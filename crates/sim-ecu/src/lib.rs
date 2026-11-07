//! XCP 从站模拟器。
//!
//! 内核自研（[D-12](../../../docs/decisions.md#d-12-xcp-on-can-从站自研)）：上游
//! xcp-lite 没有 CAN 传输层。命令处理是纯函数，收发在外层循环里，测试因此
//! 不必碰总线。

#![forbid(unsafe_code)]

use std::sync::Arc;
use std::time::Duration;

use openape_bus::{Bus, CanFrame, CanId};
use openape_diag::{Domain, Fault, FaultCode, Metrics, Severity};
use openape_xcp_proto::{Command, ErrorCode, Response, resource};

pub mod daq;

use daq::{DaqState, OdtEntry};

/// 虚拟内存大小（64 KiB），够放标定页与观测量。
pub const MEM_SIZE: usize = 0x10000;

/// 从站参数。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SimEcuConfig {
    /// 命令帧标识符（从站监听）。
    pub command_id: CanId,
    /// 响应帧标识符（从站发送）。
    pub response_id: CanId,
    /// 事件通道周期。DAQ list 按它出帧。
    pub event_period: Duration,
    /// 出帧节拍的最大抖动：实际间隔落在 `period ± jitter`。
    pub jitter: Duration,
    /// 抖动用的伪随机种子。固定种子让"抖动"可复现，测试不靠运气。
    pub jitter_seed: u64,
}

impl SimEcuConfig {
    /// 与 M0-2 spike 一致的默认值。
    pub fn default_ids() -> Self {
        Self {
            command_id: CanId::standard(0x600).expect("0x600 在 11 位内"),
            response_id: CanId::standard(0x601).expect("0x601 在 11 位内"),
            event_period: Duration::from_millis(10),
            jitter: Duration::ZERO,
            jitter_seed: 0x5EED,
        }
    }
}

/// 一台虚拟 ECU。
#[derive(Debug)]
pub struct SimEcu {
    config: SimEcuConfig,
    memory: Vec<u8>,
    mta: u32,
    page: u8,
    running: bool,
    daq: DaqState,
}

impl SimEcu {
    /// 新建，内存清零。
    pub fn new(config: SimEcuConfig) -> Self {
        Self {
            config,
            memory: vec![0; MEM_SIZE],
            mta: 0,
            page: 0,
            running: false,
            daq: DaqState::default(),
        }
    }

    /// 配置。
    pub fn config(&self) -> &SimEcuConfig {
        &self.config
    }

    /// 预置内存内容，供测试与夹具初始化。
    pub fn poke(&mut self, address: u32, data: &[u8]) {
        let start = address as usize;
        let end = start + data.len();
        if end <= self.memory.len() {
            self.memory[start..end].copy_from_slice(data);
        }
    }

    /// 读内存，测试断言用。
    pub fn peek(&self, address: u32, len: usize) -> &[u8] {
        let start = address as usize;
        &self.memory[start..start + len]
    }

    /// 推进一个事件 tick，返回本 tick 要发的 DTO 与丢弃数。
    ///
    /// 越界的条目整帧丢弃而不是补零：补零会让 master 把 0 当成真值记进曲线，
    /// 比少一帧更难查。
    pub fn on_tick(&mut self) -> TickOutput {
        let response_id = self.config.response_id;
        let plans = self.daq.tick();
        let mut frames = Vec::with_capacity(plans.len());
        let mut dropped = 0;
        for plan in &plans {
            match self.dto_of(plan, response_id) {
                Some(frame) => frames.push(frame),
                None => dropped += 1,
            }
        }
        TickOutput { frames, dropped }
    }

    fn dto_of(&self, plan: &daq::FramePlan, id: CanId) -> Option<CanFrame> {
        let mut data = Vec::with_capacity(1 + plan.entries.len() * 4);
        data.push(plan.pid);
        for entry in &plan.entries {
            let start = entry.address as usize;
            let end = start.checked_add(usize::from(entry.size))?;
            data.extend_from_slice(self.memory.get(start..end)?);
        }
        CanFrame::new(id, data)
    }

    /// 处理一整帧。byte0 就是命令码——CRO 没有 PID 前缀。
    pub fn handle(&mut self, frame: &[u8]) -> Response {
        match Command::decode(frame) {
            Err(_) => err(ErrorCode::CmdSyntax),
            Ok(command) => self.dispatch(command),
        }
    }

    /// 按命令分派到各自的处理。圈复杂度等于从站支持的命令条数（当前 26 条），
    /// 与 master 侧的编解码同属刻意保留的协议分派，不再拆分。
    fn dispatch(&mut self, command: Command) -> Response {
        let code = command.code() as u8;
        match command {
            Command::Connect { .. } => {
                self.running = true;
                // CAN 上 maxCto = 8：PID + 命令码之后只剩 6 字节参数。
                // 版本字段放不下，master 侧因此按"缺席"处理，不臆造默认值。
                ok(
                    code,
                    vec![resource::CAL_PAG | resource::DAQ, 0x00, 8, 0x00, 8, 0x00],
                )
            }
            Command::Disconnect => {
                self.running = false;
                ok(code, vec![])
            }
            Command::GetStatus => {
                let session = u8::from(self.running);
                ok(code, vec![session, 0x00, 0x00, 0x00, 0x00, 0x00])
            }
            Command::Synch => ok(code, vec![]),
            Command::GetCommModeInfo => ok(code, vec![0x00, 0x00, 0x00, 0x00, 0x00, 0x01]),
            Command::SetMta {
                address_extension,
                address,
            } => {
                if address_extension != 0 {
                    return err(ErrorCode::OutOfRange);
                }
                self.mta = address;
                ok(code, vec![])
            }
            Command::Upload { len } => {
                let Some(data) = self.read_memory(self.mta, len) else {
                    return err(ErrorCode::OutOfRange);
                };
                self.mta = self.mta.saturating_add(len as u32);
                ok(code, data)
            }
            Command::ShortUpload {
                len,
                address_extension,
                address,
            } => {
                if address_extension != 0 {
                    return err(ErrorCode::OutOfRange);
                }
                match self.read_memory(address, len) {
                    Some(data) => ok(code, data),
                    None => err(ErrorCode::OutOfRange),
                }
            }
            Command::Download { len: _, data } => {
                if !self.write_memory(self.mta, &data) {
                    return err(ErrorCode::OutOfRange);
                }
                self.mta = self.mta.saturating_add(data.len() as u32);
                ok(code, vec![])
            }
            Command::ShortDownload {
                len: _,
                address_extension,
                address,
                data,
            } => {
                if address_extension != 0 {
                    return err(ErrorCode::OutOfRange);
                }
                if self.write_memory(address, &data) {
                    ok(code, vec![])
                } else {
                    err(ErrorCode::OutOfRange)
                }
            }
            // pyXCP 取 response[2]（PID 与保留位之后），单字节标准响应它读不到第 3 字节
            Command::GetCalPage { .. } => ok(code, vec![0x00, 0x00, self.page]),
            Command::SetCalPage { page, segment, .. } => {
                if segment != 0 || page > 1 {
                    return err(ErrorCode::PageNotValid);
                }
                self.page = page;
                ok(code, vec![])
            }
            Command::GetDaqProcessorInfo => ok(
                code,
                // properties=预分频与动态 DAQ、最多 4 条 list、最多 1 个事件通道
                vec![0x03, 4, 0x00, 1, 0x00, 0x00],
            ),
            Command::GetDaqResolutionInfo => ok(
                code,
                // 条目粒度 1 字节、最大条目 7 字节；时间戳暂不支持（全 0）
                vec![1, 7, 0, 0, 0, 0],
            ),
            Command::FreeDaq => {
                self.daq.free();
                ok(code, vec![])
            }
            Command::AllocDaq { count } => reply(code, self.daq.alloc_lists(count)),
            Command::AllocOdt {
                daq_list,
                odt_count,
            } => reply(code, self.daq.alloc_odts(daq_list, odt_count)),
            Command::AllocOdtEntry {
                daq_list,
                odt,
                entries,
            } => reply(code, self.daq.alloc_entries(daq_list, odt, entries)),
            Command::SetDaqPtr {
                daq_list,
                odt,
                entry,
            } => reply(code, self.daq.set_ptr(daq_list, odt, entry)),
            Command::WriteDaq {
                bit_offset,
                size,
                address_extension,
                address,
            } => reply(
                code,
                self.daq.write_entry(OdtEntry {
                    bit_offset,
                    size,
                    address_extension,
                    address,
                }),
            ),
            Command::SetDaqListMode {
                mode: _,
                daq_list,
                event,
                prescaler,
                priority: _,
            } => reply(code, self.daq.set_mode(daq_list, event, prescaler)),
            Command::StartStopDaqList { mode, daq_list } => {
                match self.daq.start_stop_list(daq_list, mode == 1) {
                    // 响应唯一带值的地方：该 list 的首个绝对 ODT 编号
                    Ok(first) => ok(code, vec![first, 0x00]),
                    Err(error) => err(error),
                }
            }
            Command::StartStopSynch { mode } => {
                self.daq.start_stop_synch(mode == 1);
                ok(code, vec![])
            }
            Command::ClearDaqList { daq_list } => reply(code, self.daq.clear_list(daq_list)),
        }
    }

    fn read_memory(&self, address: u32, len: u8) -> Option<Vec<u8>> {
        let start = address as usize;
        let end = start.checked_add(len as usize)?;
        (end <= self.memory.len()).then(|| self.memory[start..end].to_vec())
    }

    /// 写内存。越界整笔拒绝，不写半截——半截写入会让 master 的写后读回
    /// 对不上，却查不出是哪一半没写进去。
    fn write_memory(&mut self, address: u32, data: &[u8]) -> bool {
        let start = address as usize;
        let Some(end) = start.checked_add(data.len()) else {
            return false;
        };
        if end > self.memory.len() {
            return false;
        }
        self.memory[start..end].copy_from_slice(data);
        true
    }
}

fn ok(_code: u8, payload: Vec<u8>) -> Response {
    // 响应不回显命令码，PID 之后直接是数据
    Response::Ok { payload }
}

/// 把状态机的结果翻成响应：成功回空正响应，失败回错误码。
fn reply(code: u8, result: Result<(), ErrorCode>) -> Response {
    match result {
        Ok(()) => ok(code, vec![]),
        Err(error) => err(error),
    }
}

fn err(code: ErrorCode) -> Response {
    Response::Err { code: code as u8 }
}

/// 一个事件 tick 的产出。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TickOutput {
    /// 要发到总线上的 DTO。
    pub frames: Vec<CanFrame>,
    /// 因为地址越界被丢掉的帧数。
    pub dropped: usize,
}

/// 空闲时从站多久轮询一次。没有命令不是错误，继续等。
const IDLE_POLL: Duration = Duration::from_millis(200);

/// 在总线上跑从站，直到任务被取消。
pub async fn serve(ecu: SimEcu, bus: Arc<dyn Bus>) -> Result<(), Fault> {
    serve_with_metrics(ecu, bus, Arc::new(Metrics::new())).await
}

/// 同上，但由调用方提供指标收集器——测试要读丢帧数。
pub async fn serve_with_metrics(
    mut ecu: SimEcu,
    bus: Arc<dyn Bus>,
    metrics: Arc<Metrics>,
) -> Result<(), Fault> {
    let config = ecu.config.clone();
    let mut rng = Lcg::new(config.jitter_seed);
    let mut next_tick = tokio::time::Instant::now() + next_delay(&config, &mut rng);

    loop {
        tokio::select! {
            biased;
            received = bus.recv(IDLE_POLL) => {
                let frame = match received {
                    Ok(frame) => frame,
                    // 空闲轮询到期不是错误，接着等
                    Err(openape_bus::BusError::Timeout { .. }) => continue,
                    Err(err) => {
                        let fault = bus_fault(&err);
                        tracing::error!(target: "openape::sim", %fault, "从站收帧失败，退出");
                        return Err(fault);
                    }
                };
                let Some(outbound) = respond(&mut ecu, &config, &frame) else {
                    continue;
                };
                if let Err(err) = bus.send(outbound).await {
                    let fault = bus_fault(&err);
                    tracing::error!(target: "openape::sim", %fault, "从站发响应失败，退出");
                    return Err(fault);
                }
            }
            _ = tokio::time::sleep_until(next_tick) => {
                emit_daq(&mut ecu, &bus, &metrics).await;
                next_tick = tokio::time::Instant::now() + next_delay(&config, &mut rng);
            }
        }
    }
}

/// 发一轮 DAQ。单帧发不出去只计丢帧继续跑——DAQ 少一帧不致命，
/// 命令通道挂了才致命，两者不能同一种处理。
async fn emit_daq(ecu: &mut SimEcu, bus: &Arc<dyn Bus>, metrics: &Metrics) {
    let output = ecu.on_tick();
    if output.dropped > 0 {
        metrics.record_dropped_frames(output.dropped as u64);
        tracing::warn!(target: "openape::sim", dropped = output.dropped, "地址越界，丢掉 DAQ 帧");
    }
    for frame in output.frames {
        match bus.send(frame).await {
            Ok(()) => metrics.record_daq_frames(1),
            Err(err) => {
                metrics.record_dropped_frames(1);
                tracing::warn!(target: "openape::sim", %err, "DAQ 帧发不出去");
            }
        }
    }
}

/// 下一次出帧的间隔：`period ± jitter`。没有抖动时就是固定周期。
fn next_delay(config: &SimEcuConfig, rng: &mut Lcg) -> Duration {
    if config.jitter.is_zero() {
        return config.event_period;
    }
    let span = u64::try_from(config.jitter.as_nanos()).unwrap_or(0) * 2;
    let offset = if span == 0 {
        0
    } else {
        rng.next() % (span + 1)
    };
    config
        .event_period
        .saturating_sub(config.jitter)
        .saturating_add(Duration::from_nanos(offset))
}

/// 线性同余伪随机。只要"抖动可复现"，不需要统计质量。
struct Lcg(u64);

impl Lcg {
    fn new(seed: u64) -> Self {
        Self(seed | 1)
    }

    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0 >> 33
    }
}

/// 给定一帧，决定要不要回、回什么。不是命令帧就返回 `None`。
///
/// 从站不处理别人的流量，也不该把命令标识符上的 DAQ DTO 当命令——那样会
/// 回环自激。
fn respond(ecu: &mut SimEcu, config: &SimEcuConfig, frame: &CanFrame) -> Option<CanFrame> {
    if frame.id != config.command_id {
        return None;
    }
    if frame.data.is_empty() {
        return None;
    }
    let response = ecu.handle(&frame.data);
    CanFrame::new(config.response_id, response.encode())
}

fn bus_fault(err: &openape_bus::BusError) -> Fault {
    let code = FaultCode::new(Domain::Session, 2).expect("序号 2 合法");
    Fault::new(code, Severity::Error, format!("从站总线出错：{err}"))
        .with_hint("确认通道仍然存在（`probe bus`）")
}

#[cfg(test)]
mod tests {
    use super::*;
    use openape_xcp_proto::{CommandCode, MAX_CTO};

    fn ecu() -> SimEcu {
        SimEcu::new(SimEcuConfig::default_ids())
    }

    fn send(ecu: &mut SimEcu, command: Command) -> Response {
        let frame = command.encode().expect("测试命令都能编码");
        ecu.handle(&frame)
    }

    /// 期望正响应。`_command` 留作调用处可读性的标签：响应不再回显命令码，
    /// 这里无法校验"这是哪条命令的答复"。
    fn expect_ok(response: Response, _command: CommandCode) -> Vec<u8> {
        match response {
            Response::Ok { payload } => payload,
            other => panic!("期望正响应，实际 {other:?}"),
        }
    }

    #[test]
    fn next_delay_spans_the_whole_jitter_band() {
        let config = SimEcuConfig {
            event_period: Duration::from_millis(20),
            jitter: Duration::from_millis(5),
            jitter_seed: 7,
            ..SimEcuConfig::default_ids()
        };
        let mut rng = Lcg::new(config.jitter_seed);
        let mut low = u128::MAX;
        let mut high = 0u128;
        for _ in 0..64 {
            let delay = next_delay(&config, &mut rng).as_nanos();
            low = low.min(delay);
            high = high.max(delay);
        }
        assert!(
            low >= Duration::from_millis(15).as_nanos(),
            "下界是 period - jitter"
        );
        assert!(
            high > Duration::from_millis(22).as_nanos(),
            "上界要摸到 period + jitter 附近"
        );
        assert!(
            high <= Duration::from_millis(25).as_nanos(),
            "不该越过 period + jitter"
        );
    }

    #[test]
    fn next_delay_without_jitter_is_constant() {
        let config = SimEcuConfig {
            event_period: Duration::from_millis(12),
            jitter: Duration::ZERO,
            ..SimEcuConfig::default_ids()
        };
        let mut rng = Lcg::new(1);
        for _ in 0..4 {
            assert_eq!(next_delay(&config, &mut rng), Duration::from_millis(12));
        }
    }

    #[test]
    fn daq_capability_reports_match_the_documented_shape() {
        let mut ecu = ecu();
        let processor = expect_ok(
            send(&mut ecu, Command::GetDaqProcessorInfo),
            CommandCode::GetDaqProcessorInfo,
        );
        assert_eq!(processor, vec![0x03, 4, 0x00, 1, 0x00, 0x00]);

        let resolution = expect_ok(
            send(&mut ecu, Command::GetDaqResolutionInfo),
            CommandCode::GetDaqResolutionInfo,
        );
        assert_eq!(
            resolution,
            vec![1, 7, 0, 0, 0, 0],
            "条目粒度 1 字节、上限 7 字节"
        );
    }

    /// 配一条 list：一个 ODT、一个两字节条目，供 DAQ 命令用例复用。
    fn daily_configure(ecu: &mut SimEcu) {
        expect_ok(
            send(ecu, Command::AllocDaq { count: 1 }),
            CommandCode::AllocDaq,
        );
        expect_ok(
            send(
                ecu,
                Command::AllocOdt {
                    daq_list: 0,
                    odt_count: 1,
                },
            ),
            CommandCode::AllocOdt,
        );
        expect_ok(
            send(
                ecu,
                Command::AllocOdtEntry {
                    daq_list: 0,
                    odt: 0,
                    entries: 1,
                },
            ),
            CommandCode::AllocOdtEntry,
        );
        expect_ok(
            send(
                ecu,
                Command::SetDaqPtr {
                    daq_list: 0,
                    odt: 0,
                    entry: 0,
                },
            ),
            CommandCode::SetDaqPtr,
        );
        expect_ok(
            send(
                ecu,
                Command::WriteDaq {
                    bit_offset: 0,
                    size: 2,
                    address_extension: 0,
                    address: 0x1000,
                },
            ),
            CommandCode::WriteDaq,
        );
    }

    #[test]
    fn start_stop_synch_drives_every_list() {
        let mut ecu = ecu();
        daily_configure(&mut ecu);
        expect_ok(
            send(&mut ecu, Command::StartStopSynch { mode: 1 }),
            CommandCode::StartStopSynch,
        );
        assert!(!ecu.on_tick().frames.is_empty(), "同步启动后应出帧");
        expect_ok(
            send(&mut ecu, Command::StartStopSynch { mode: 0 }),
            CommandCode::StartStopSynch,
        );
        assert!(ecu.on_tick().frames.is_empty(), "同步停止后不再出帧");
    }

    #[test]
    fn download_may_fill_the_last_byte_but_not_pass_it() {
        let mut ecu = ecu();
        let last = (MEM_SIZE - 1) as u32;
        expect_ok(
            send(
                &mut ecu,
                Command::SetMta {
                    address_extension: 0,
                    address: last,
                },
            ),
            CommandCode::SetMta,
        );
        expect_ok(
            send(
                &mut ecu,
                Command::Download {
                    len: 1,
                    data: vec![0x5A],
                },
            ),
            CommandCode::Download,
        );
        assert_eq!(ecu.peek(last, 1), &[0x5A], "正好写满末尾要允许");

        expect_ok(
            send(
                &mut ecu,
                Command::SetMta {
                    address_extension: 0,
                    address: last,
                },
            ),
            CommandCode::SetMta,
        );
        let response = send(
            &mut ecu,
            Command::Download {
                len: 2,
                data: vec![0x11, 0x22],
            },
        );
        match response {
            Response::Err { code } => assert_eq!(code, ErrorCode::OutOfRange as u8),
            other => panic!("越过末尾要拒绝，实际 {other:?}"),
        }
    }

    #[test]
    fn short_upload_may_end_exactly_at_the_last_byte() {
        let mut ecu = ecu();
        let payload = expect_ok(
            send(
                &mut ecu,
                Command::ShortUpload {
                    len: 2,
                    address_extension: 0,
                    address: (MEM_SIZE - 2) as u32,
                },
            ),
            CommandCode::ShortUpload,
        );
        assert_eq!(payload.len(), 2, "读到内存末尾正好要允许");
    }

    #[test]
    fn connect_reports_resources_and_frame_limits() {
        let mut ecu = ecu();
        let payload = expect_ok(
            send(&mut ecu, Command::Connect { mode: 0 }),
            CommandCode::Connect,
        );
        assert_eq!(payload[0] & resource::DAQ, resource::DAQ);
        assert_eq!(payload[0] & resource::CAL_PAG, resource::CAL_PAG);
        assert_eq!(u16::from_le_bytes([payload[2], payload[3]]), 8, "maxCto");
        assert_eq!(u16::from_le_bytes([payload[4], payload[5]]), 8, "maxDto");
    }

    #[test]
    fn status_reflects_session_state() {
        let mut ecu = ecu();
        assert_eq!(
            expect_ok(send(&mut ecu, Command::GetStatus), CommandCode::GetStatus)[0],
            0
        );
        let _ = send(&mut ecu, Command::Connect { mode: 0 });
        assert_eq!(
            expect_ok(send(&mut ecu, Command::GetStatus), CommandCode::GetStatus)[0],
            1
        );
    }

    #[test]
    fn short_upload_reads_preseeded_memory() {
        let mut ecu = ecu();
        ecu.poke(0x1000, &[0xDE, 0xAD, 0xBE, 0xEF]);
        let payload = expect_ok(
            send(
                &mut ecu,
                Command::ShortUpload {
                    len: 4,
                    address_extension: 0,
                    address: 0x1000,
                },
            ),
            CommandCode::ShortUpload,
        );
        assert_eq!(payload, vec![0xDE, 0xAD, 0xBE, 0xEF]);
    }

    #[test]
    fn upload_advances_the_transfer_address() {
        let mut ecu = ecu();
        ecu.poke(0x1000, &[1, 2, 3, 4, 5, 6, 7, 8]);
        let _ = send(
            &mut ecu,
            Command::SetMta {
                address_extension: 0,
                address: 0x1000,
            },
        );
        let first = expect_ok(
            send(&mut ecu, Command::Upload { len: 6 }),
            CommandCode::Upload,
        );
        let second = expect_ok(
            send(&mut ecu, Command::Upload { len: 2 }),
            CommandCode::Upload,
        );
        assert_eq!([first, second].concat(), vec![1, 2, 3, 4, 5, 6, 7, 8]);
    }

    #[test]
    fn download_writes_memory_and_advances() {
        let mut ecu = ecu();
        let _ = send(
            &mut ecu,
            Command::SetMta {
                address_extension: 0,
                address: 0x2000,
            },
        );
        let _ = send(
            &mut ecu,
            Command::Download {
                len: 3,
                data: vec![0xAA, 0xBB, 0xCC],
            },
        );
        assert_eq!(ecu.peek(0x2000, 3), &[0xAA, 0xBB, 0xCC]);
        // 地址已自增到 0x2003。
        let _ = send(
            &mut ecu,
            Command::Download {
                len: 1,
                data: vec![0xDD],
            },
        );
        assert_eq!(ecu.peek(0x2003, 1), &[0xDD]);
    }

    #[test]
    fn out_of_range_read_is_refused_not_truncated() {
        let mut ecu = ecu();
        let response = send(
            &mut ecu,
            Command::ShortUpload {
                len: 4,
                address_extension: 0,
                address: (MEM_SIZE - 2) as u32,
            },
        );
        match response {
            Response::Err { code } => assert_eq!(code, ErrorCode::OutOfRange as u8),
            other => panic!("期望越界错误，实际 {other:?}"),
        }
    }

    #[test]
    fn calibration_page_switch_round_trips_and_rejects_bad_page() {
        let mut ecu = ecu();
        expect_ok(
            send(
                &mut ecu,
                Command::SetCalPage {
                    mode: 0,
                    segment: 0,
                    page: 1,
                },
            ),
            CommandCode::SetCalPage,
        );
        let payload = expect_ok(
            send(
                &mut ecu,
                Command::GetCalPage {
                    mode: 0,
                    segment: 0,
                },
            ),
            CommandCode::GetCalPage,
        );
        assert_eq!(
            payload,
            vec![0x00, 0x00, 1],
            "GET_CAL_PAGE 回 [保留, 保留, 页号]：pyXCP 取 response[2]"
        );

        let response = send(
            &mut ecu,
            Command::SetCalPage {
                mode: 0,
                segment: 0,
                page: 7,
            },
        );
        match response {
            Response::Err { code } => assert_eq!(code, ErrorCode::PageNotValid as u8),
            other => panic!("期望页号错误，实际 {other:?}"),
        }
    }

    #[test]
    fn every_response_fits_in_one_can_frame() {
        // 经典 CAN 一帧 8 字节；从站回超长响应会被总线层拒绝，
        // 现象是 master 一直等到超时。这里把每个命令的响应长度钉住。
        let commands = [
            Command::Connect { mode: 0 },
            Command::Disconnect,
            Command::GetStatus,
            Command::Synch,
            Command::GetCommModeInfo,
            Command::SetMta {
                address_extension: 0,
                address: 0x1000,
            },
            Command::ShortUpload {
                len: 6,
                address_extension: 0,
                address: 0x1000,
            },
            Command::Upload { len: 6 },
            Command::Download {
                len: 5,
                data: vec![0; 5],
            },
            Command::GetCalPage {
                mode: 0,
                segment: 0,
            },
            Command::SetCalPage {
                mode: 0,
                segment: 0,
                page: 1,
            },
        ];
        let mut ecu = ecu();
        ecu.poke(0x1000, &[0; 8]);
        for command in commands {
            let response = send(&mut ecu, command.clone());
            let frame = response.encode();
            assert!(
                frame.len() <= MAX_CTO,
                "{command:?} 的响应 {} 字节，超过一帧上限 {MAX_CTO}",
                frame.len()
            );
        }
    }

    #[test]
    fn short_download_writes_without_moving_the_transfer_address() {
        // 经典 CAN 一帧 8 字节装不下带数据的短写，所以这里手搓加宽帧——从站
        // 按 `len` 取数据，帧有多宽与它无关（CAN FD 上是常态）。
        let mut ecu = ecu();
        let _ = send(
            &mut ecu,
            Command::SetMta {
                address_extension: 0,
                address: 0x3000,
            },
        );
        expect_ok(
            ecu.handle(&[0xED, 2, 0, 0, 0x00, 0x20, 0x00, 0x00, 0x12, 0x34]),
            CommandCode::ShortDownload,
        );
        assert_eq!(ecu.peek(0x2000, 2), &[0x12, 0x34], "写到了短写地址");
        // 传输地址没动：接着 DOWNLOAD 应当落在 0x3000。
        let _ = send(
            &mut ecu,
            Command::Download {
                len: 1,
                data: vec![0x77],
            },
        );
        assert_eq!(ecu.peek(0x3000, 1), &[0x77], "短写不该动 MTA");
    }

    #[test]
    fn short_download_past_the_end_is_refused() {
        let mut ecu = ecu();
        // 地址 0xFFFE + 2 字节正好写满，再加一字节就出界。
        let last_two = [0xED, 2, 0, 0, 0xFE, 0xFF, 0x00, 0x00, 0x11, 0x22];
        expect_ok(ecu.handle(&last_two), CommandCode::ShortDownload);

        let past_end = [0xED, 2, 0, 0, 0xFF, 0xFF, 0x00, 0x00, 0x33, 0x44];
        match ecu.handle(&past_end) {
            Response::Err { code } => assert_eq!(code, ErrorCode::OutOfRange as u8),
            other => panic!("期望越界错误，实际 {other:?}"),
        }
        assert_eq!(ecu.peek((MEM_SIZE - 1) as u32, 1), &[0x22], "越界不写半截");
    }

    #[test]
    fn unknown_command_byte_is_a_syntax_error() {
        let mut ecu = ecu();
        match ecu.handle(&[0x01, 0x02, 0x00]) {
            Response::Err { code } => assert_eq!(code, ErrorCode::CmdSyntax as u8),
            other => panic!("期望语法错误，实际 {other:?}"),
        }
    }

    #[test]
    fn disconnect_stops_the_session() {
        let mut ecu = ecu();
        let _ = send(&mut ecu, Command::Connect { mode: 0 });
        let _ = send(&mut ecu, Command::Disconnect);
        assert_eq!(
            expect_ok(send(&mut ecu, Command::GetStatus), CommandCode::GetStatus)[0],
            0
        );
    }
}
