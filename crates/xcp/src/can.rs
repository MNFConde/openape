//! XCP-on-CAN master。帧编解码与从 `bus` 收发之间的胶水。

use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use openape_bus::{Bus, BusError, CanFrame, CanId};
use openape_cal_core::{Address, ByteLen};
use openape_diag::{Fault, Severity};
use openape_xcp_proto::{
    Command, DecodeError, ErrorCode, MAX_CTO, OdtIds, Response, SHORT_IO_HEADER, SeedKey,
};

use crate::connection::{self, ConnectInfo, DaqEvent, SessionStatus, XcpConnection, seq};
use crate::daq::{self, DaqLayout, DaqListSpec};

/// SHORT_UPLOAD 单次可读的最大字节数：响应帧 `PID + 命令码 + 数据` 共 8 字节。
/// 单帧能带回的最大字节数：整帧减掉响应 PID，再没有别的开销。
pub const MAX_SHORT_UPLOAD_LEN: u16 = (MAX_CTO - 1) as u16;

/// CAN-FD 的单帧上限。
pub const MAX_FD_FRAME: usize = 64;

/// 会话参数。
#[derive(Debug, Clone)]
pub struct XcpConfig {
    /// 命令帧标识符（CRO）。
    pub command_id: CanId,
    /// 响应帧标识符（DTO/响应）。
    pub response_id: CanId,
    /// 单次等待响应的时限。
    pub timeout: Duration,
    /// 超时后的重发次数。
    pub retries: u8,
    /// 解锁用的种子密钥算法。真实 ECU 的算法各不相同，默认是示例算法——
    /// 对着不认识它的 ECU 必然一直锁着，这是预期行为而不是缺陷。
    pub seed_key: std::sync::Arc<dyn SeedKey>,
    /// 解锁的资源位。默认标定页（写入被它保护）。
    pub unlock_resource: u8,
}

impl XcpConfig {
    /// 常用默认值：`0x600` / `0x601` 与 M0-2 spike 一致，超时 200 ms，重试 2 次。
    pub fn new(command_id: CanId, response_id: CanId) -> Self {
        Self {
            command_id,
            response_id,
            timeout: Duration::from_millis(200),
            retries: 2,
            seed_key: std::sync::Arc::new(openape_xcp_proto::ExampleSeedKey),
            unlock_resource: openape_xcp_proto::resource::CAL_PAG,
        }
    }
}

/// 基于 CAN 的 XCP master。
pub struct CanXcpMaster {
    bus: Arc<dyn Bus>,
    config: XcpConfig,
    /// 建链后从站报的会话参数。存下来是因为 DAQ 的 ODT 容量要读 `max_dto`，
    /// 而它只在 CONNECT 响应里出现一次。
    session: Option<ConnectInfo>,
}

impl CanXcpMaster {
    /// 新建。建链前 `read` 会被拒绝。
    pub fn new(bus: Arc<dyn Bus>, config: XcpConfig) -> Self {
        Self {
            bus,
            config,
            session: None,
        }
    }

    /// 当前配置。
    pub fn config(&self) -> &XcpConfig {
        &self.config
    }

    /// 已建立的会话；没有就报错。
    fn require_session(&self) -> Result<&ConnectInfo, Fault> {
        self.session.as_ref().ok_or_else(|| {
            connection::fault(
                seq::NOT_CONNECTED,
                Severity::Error,
                "会话未建立，从站不会响应命令",
            )
            .with_hint("先 connect 再发命令")
        })
    }

    /// 发一条命令、等一条响应，返回响应参数（已剥 PID 与命令码）。
    ///
    /// 等待期间收到其它标识符的帧（例如 DAQ 的 DTO）直接丢弃——采集与
    /// 命令在同一条总线上并存，不能因为来了数据帧就判命令失败。
    async fn transact(&self, command: &Command) -> Result<Vec<u8>, Fault> {
        let code = command.code() as u8;
        let frame = self.command_frame(command)?;
        for _ in 0..=self.config.retries {
            self.bus
                .send(frame.clone())
                .await
                .map_err(|err| bus_fault(&err))?;
            if let Some(payload) = self.await_answer(code).await? {
                return Ok(payload);
            }
        }
        Err(self.timeout_fault(code))
    }

    /// 把命令编成要发出去的帧。定宽载体（经典 CAN）补齐到经典上限；
    /// 不定宽的载体（UDP、回放）保持编码出的长度——对端按声明长度
    /// 收帧，多余的字节会被当成命令的一部分拒掉。
    fn command_frame(&self, command: &Command) -> Result<CanFrame, Fault> {
        let mut data = command
            .encode_max(self.frame_width())
            .map_err(|err| connection::fault(seq::CODEC, Severity::Error, err.to_string()))?;
        if self.bus.capabilities().cro_padding && data.len() < MAX_CTO {
            data.resize(MAX_CTO, 0);
        }
        CanFrame::new(self.config.command_id, data)
            .ok_or_else(|| connection::fault(seq::CODEC, Severity::Error, "命令帧长超过帧上限"))
    }

    /// 能用的单帧宽度：从站报的 `maxCto` 与总线帧形态取下限。
    ///
    /// 两侧各报各的上限：从站可能支持 FD 而当前后端只有经典 CAN，此时帧宽
    /// 由后端说了算。
    fn frame_width(&self) -> usize {
        let from_slave = self
            .session
            .as_ref()
            .map_or(MAX_CTO, |info| info.max_cto as usize);
        let from_bus = if self.bus.capabilities().can_fd() {
            MAX_FD_FRAME
        } else {
            MAX_CTO
        };
        from_slave.min(from_bus).max(1)
    }

    /// 等到一条属于本命令的响应。`None` 表示等到期限仍未收到（可重发）。
    ///
    /// 同一标识符上可能混着 DAQ 的 DTO 与事件帧，它们不是命令响应，跳过继续
    /// 等——不能因为来了数据就判命令失败。
    async fn await_answer(&self, code: u8) -> Result<Option<Vec<u8>>, Fault> {
        let deadline = Instant::now() + self.config.timeout;
        loop {
            // 剩余时间为 0 时 recv(0) 立即超时，会走下面的 Timeout 分支；
            // 不必单独判一次，少一个测不到的分支。
            let remaining = deadline.saturating_duration_since(Instant::now());
            let received = match self.bus.recv(remaining).await {
                Ok(frame) => frame,
                Err(BusError::Timeout { .. }) => return Ok(None),
                Err(err) => return Err(bus_fault(&err)),
            };
            if received.id != self.config.response_id {
                continue;
            }
            match decode_answer(&received, code)? {
                Some(payload) => return Ok(Some(payload)),
                None => continue,
            }
        }
    }

    fn timeout_fault(&self, code: u8) -> Fault {
        connection::fault(
            seq::TIMEOUT,
            Severity::Error,
            format!(
                "等待从站响应超时（{} ms × {} 次重发）",
                self.config.timeout.as_millis(),
                self.config.retries
            ),
        )
        .with_context("command", format!("{code:#04x}"))
        .with_hint("确认从站已启动、通道与两侧标识符一致")
    }
}

/// 解析一帧响应。返回 `Ok(None)` 表示这帧不是命令响应，应当跳过。
fn decode_answer(frame: &CanFrame, _code: u8) -> Result<Option<Vec<u8>>, Fault> {
    let pid = frame.data.first().copied().unwrap_or(0);
    if pid != openape_xcp_proto::PID_CMD_RES && pid != openape_xcp_proto::PID_ERR {
        return Ok(None);
    }
    match Response::decode(&frame.data) {
        Ok(Response::Ok { payload }) => Ok(Some(payload)),
        Ok(Response::Err { code }) => Err(slave_error(code)),
        Err(err) => Err(decode_fault(err)),
    }
}

fn decode_fault(err: DecodeError) -> Fault {
    let message = err.to_string();
    connection::fault(seq::CODEC, Severity::Error, message)
        .with_hint("检查两侧 maxCto 是否一致：响应是 PID 加数据，不回显命令码")
}

fn slave_error(code: u8) -> Fault {
    let text = match ErrorCode::from_u8(code) {
        Some(known) => format!("从站返回错误 {code:#04x}（{known:?}）"),
        None => format!("从站返回未登记的错误码 {code:#04x}"),
    };
    let fault = connection::fault(seq::SLAVE_ERROR, Severity::Error, text);
    match ErrorCode::from_u8(code) {
        Some(ErrorCode::AccessLocked) => {
            fault.with_hint("该资源被种子密钥保护：先用 unlock 解锁，或确认密钥算法与该 ECU 匹配")
        }
        Some(ErrorCode::PageNotValid) => {
            fault.with_hint("页号不被接受：合法页号见 GET_CAL_PAGE 的读回与 A2L 的页表")
        }
        Some(ErrorCode::AccessDenied) => fault.with_hint("该地址不可访问：确认地址落在标定段内"),
        Some(ErrorCode::Sequence) => fault.with_hint("命令顺序不对：先取种子再交密钥"),
        _ => fault.with_hint("查看命令参数是否越界、目标段是否可写"),
    }
}

fn bus_fault(err: &BusError) -> Fault {
    let message = err.to_string();
    connection::fault(seq::BUS, Severity::Error, message).with_hint("先跑 `probe bus` 确认通道可用")
}

#[async_trait]
impl XcpConnection for CanXcpMaster {
    async fn connect(&mut self, mode: u8) -> Result<ConnectInfo, Fault> {
        let payload = self.transact(&Command::Connect { mode }).await?;
        let info = parse_connect_info(&payload)?;
        self.session = Some(info.clone());
        Ok(info)
    }

    async fn disconnect(&mut self) -> Result<(), Fault> {
        self.transact(&Command::Disconnect).await?;
        self.session = None;
        Ok(())
    }

    async fn status(&mut self) -> Result<SessionStatus, Fault> {
        let payload = self.transact(&Command::GetStatus).await?;
        let running = payload.first().copied().unwrap_or(0) & 0x01 != 0;
        let protection = payload.get(2).copied().unwrap_or(0);
        Ok(SessionStatus {
            running,
            protection,
        })
    }

    async fn read(&mut self, address: Address, len: ByteLen) -> Result<Vec<u8>, Fault> {
        self.require_session()?;

        let total = len.get() as usize;
        // 一帧装得下就用 SHORT_UPLOAD：一条命令、不动传输地址。
        if total <= MAX_SHORT_UPLOAD_LEN as usize {
            let payload = self
                .transact(&Command::ShortUpload {
                    len: total as u8,
                    address_extension: 0,
                    address: address.get(),
                })
                .await?;
            return take_exact(payload, total);
        }

        // 装不下就 SET_MTA 一次，再按 UPLOAD 逐帧搬；UPLOAD 自己会推进地址。
        self.transact(&Command::SetMta {
            address_extension: 0,
            address: address.get(),
        })
        .await?;

        let mut out = Vec::with_capacity(total);
        while out.len() < total {
            let remaining = total - out.len();
            let chunk = remaining.min(MAX_SHORT_UPLOAD_LEN as usize);
            let payload = self.transact(&Command::Upload { len: chunk as u8 }).await?;
            out.extend_from_slice(&take_exact(payload, chunk)?);
        }
        Ok(out)
    }

    async fn write(&mut self, address: Address, data: &[u8]) -> Result<(), Fault> {
        self.require_session()?;
        let chunk = self.download_capacity()?;
        if data.is_empty() {
            return Err(connection::fault(
                seq::WRITE,
                Severity::Error,
                format!("写入 {address} 的长度为 0"),
            )
            .with_hint("写 0 字节不会改变任何东西，检查标定量的长度来源"));
        }

        // 短写省掉 SET_MTA，但只在帧够宽时可用：经典 CAN 的固定头已占满一帧。
        let short_capacity = self.frame_width().saturating_sub(SHORT_IO_HEADER);
        if data.len() <= short_capacity {
            self.transact(&Command::ShortDownload {
                len: data.len() as u8,
                address_extension: 0,
                address: address.get(),
                data: data.to_vec(),
            })
            .await?;
        } else {
            self.transact(&Command::SetMta {
                address_extension: 0,
                address: address.get(),
            })
            .await?;
            for piece in data.chunks(chunk) {
                self.transact(&Command::Download {
                    len: piece.len() as u8,
                    data: piece.to_vec(),
                })
                .await?;
            }
        }
        self.verify(address, data).await
    }

    async fn cal_page(&mut self) -> Result<u8, Fault> {
        self.require_session()?;
        let payload = self
            .transact(&Command::GetCalPage {
                mode: 0,
                segment: 0,
            })
            .await?;
        payload.get(2).copied().ok_or_else(|| {
            connection::fault(
                seq::PAGE,
                Severity::Error,
                format!("GET_CAL_PAGE 响应只有 {} 字节，取不到页号", payload.len()),
            )
            .with_hint("页号在响应的第 3 字节（pyXCP 也读这一字节）")
        })
    }

    async fn set_cal_page(&mut self, page: u8) -> Result<(), Fault> {
        self.require_session()?;
        self.transact(&Command::SetCalPage {
            mode: 0,
            segment: 0,
            page,
        })
        .await?;
        // 从站可能"接受命令但不切"，所以读回是唯一的判据。
        let actual = self.cal_page().await?;
        if actual != page {
            return Err(connection::fault(
                seq::PAGE,
                Severity::Error,
                format!("要求切到页 {page}，读回是页 {actual}"),
            )
            .with_hint("确认从站支持该页号；页表见 A2L 的 CALIBRATION_METHOD 或 GET_PAGE_INFO"));
        }
        Ok(())
    }

    async fn unlock(&mut self) -> Result<(), Fault> {
        self.require_session()?;
        let resource = self.config.unlock_resource;
        let payload = self
            .transact(&Command::GetSeed { mode: 0, resource })
            .await?;
        let seed_len = usize::from(*payload.first().ok_or_else(|| {
            connection::fault(
                seq::UNLOCK,
                Severity::Error,
                "GET_SEED 响应为空，取不到种子长度",
            )
        })?);
        if seed_len == 0 {
            // 标准约定：资源本来就解锁，种子长度为 0。
            return Ok(());
        }
        let seed = payload.get(1..1 + seed_len).ok_or_else(|| {
            connection::fault(
                seq::UNLOCK,
                Severity::Error,
                format!(
                    "GET_SEED 声明 {seed_len} 字节种子，响应只带回 {} 字节",
                    payload.len().saturating_sub(1)
                ),
            )
            .with_hint("种子超过单帧容量时要分段取，本版本只支持一段")
        })?;
        let key = self.config.seed_key.key(seed);
        if key.is_empty() || key.len() > MAX_CTO - 2 {
            return Err(connection::fault(
                seq::UNLOCK,
                Severity::Error,
                format!(
                    "密钥长度 {} 字节，单帧只能承载 {} 字节",
                    key.len(),
                    MAX_CTO - 2
                ),
            )
            .with_hint("种子密钥算法要产出不超过 6 字节的密钥"));
        }
        let answer = self
            .transact(&Command::Unlock {
                len: key.len() as u8,
                key,
            })
            .await?;
        let status = answer.first().copied().unwrap_or(0);
        if status & resource == 0 {
            return Err(connection::fault(
                seq::UNLOCK,
                Severity::Error,
                format!("UNLOCK 之后资源位 {resource:#04x} 仍未解锁（状态 {status:#04x}）"),
            )
            .with_hint("确认密钥算法与该 ECU 匹配：默认示例算法只对自研从站有效"));
        }
        Ok(())
    }

    async fn daq_events(&mut self) -> Result<Vec<DaqEvent>, Fault> {
        self.require_session()?;
        // 事件总数在 GET_DAQ_PROCESSOR_INFO 里；为 0 表示从站没有打开
        // 事件查询，不硬扫。
        let payload = self.transact(&Command::GetDaqProcessorInfo).await?;
        let count = payload
            .get(3..5)
            .and_then(|raw| <[u8; 2]>::try_from(raw).ok())
            .map(u16::from_le_bytes)
            .ok_or_else(|| {
                connection::fault(
                    seq::DAQ,
                    Severity::Error,
                    format!(
                        "GET_DAQ_PROCESSOR_INFO 响应只有 {} 字节，读不到事件总数",
                        payload.len()
                    ),
                )
            })?;
        let unsupported = || {
            connection::fault(
                seq::DAQ,
                Severity::Error,
                "从站不支持 GET_DAQ_EVENT_INFO（可选命令）",
            )
            .with_hint("事件号从 ECU 手册或 A2L 里找；也可以直接给事件号配 DAQ")
        };
        let mut events = Vec::new();
        for index in 0..count {
            let payload = self
                .transact(&Command::GetDaqEventInfo { event: index })
                .await
                .map_err(|err| {
                    if err.message().contains("命令码") {
                        unsupported()
                    } else {
                        err
                    }
                })?;
            // 响应有两种实测布局：properties(1) max_daq_lists(1)
            // name_length(1) time_cycle(1) time_unit(1) priority(1)，
            // 或参考实现 V2.3 的精简版——只有前两个域，且名字以 C 字
            // 符串形式挂在 MTA 上，长度要按 0 结尾截。域缺席按 0 计。
            let properties = payload.first().copied().unwrap_or(0);
            let max_daq_lists = payload.get(1).copied().unwrap_or(0);
            let name = match payload.get(2).copied() {
                Some(name_len) if name_len > 0 => {
                    let bytes = self.transact(&Command::Upload { len: name_len }).await?;
                    String::from_utf8_lossy(&bytes).into_owned()
                }
                _ => {
                    // 参考实现的 PTR 段 UPLOAD 对大块读取会拒绝
                    // （实测 64 字节 0x24，8 字节通过）；它的名字缓冲
                    // 上限 16 字节（XCP_MAX_EVENT_NAME=15 + 0 终止符）。
                    let cap = self
                        .session
                        .as_ref()
                        .map_or(MAX_CTO - 1, |info| info.max_cto as usize)
                        .saturating_sub(1)
                        .min(16);
                    let bytes = self.transact(&Command::Upload { len: cap as u8 }).await?;
                    let ended = bytes.split(|byte| *byte == 0).next().unwrap_or(&[]);
                    String::from_utf8_lossy(ended).into_owned()
                }
            };
            events.push(DaqEvent {
                index,
                properties,
                max_daq_lists,
                time_cycle: payload.get(3).copied().unwrap_or(0),
                time_unit: payload.get(4).copied().unwrap_or(0),
                name,
            });
        }
        Ok(events)
    }

    async fn configure_daq(&mut self, lists: &[DaqListSpec]) -> Result<DaqLayout, Fault> {
        let session = self.require_session()?;
        if session.resource & openape_xcp_proto::resource::DAQ == 0 {
            return Err(connection::fault(
                seq::DAQ,
                Severity::Error,
                "从站 CONNECT 响应里没有声明 DAQ 能力",
            )
            .with_hint("该 ECU 只支持标定读写；换用 resource 位含 DAQ 的从站"));
        }
        // ODT 的载荷上限：从站报的 maxDto 减去 PID，封顶 255——ODT 条目
        // 大小本身是单字节域（参考实现也报 255），更大的值不会兑现。
        let limit = session.max_dto.saturating_sub(1).min(255) as u8;

        // 先把分组算完：任何一条放不下都在动从站之前报出来，不留半配置状态。
        let odts_per_list = lists
            .iter()
            .map(|spec| daq::plan_odts(&spec.signals, limit))
            .collect::<Result<Vec<_>, _>>()?;

        self.transact(&Command::FreeDaq).await?;
        self.transact(&Command::AllocDaq {
            count: lists.len() as u16,
        })
        .await?;
        self.allocate_odts(&odts_per_list).await?;
        self.write_entries(lists, &odts_per_list).await?;
        self.start_and_map(&odts_per_list).await
    }
}

impl CanXcpMaster {
    /// DOWNLOAD 一帧能带几字节：整帧减去命令码与长度字节。
    fn download_capacity(&self) -> Result<usize, Fault> {
        let width = self.frame_width();
        let chunk = width.saturating_sub(2);
        if chunk == 0 {
            return Err(connection::fault(
                seq::CODEC,
                Severity::Error,
                format!("单帧只有 {width} 字节，放不下写入数据"),
            ));
        }
        Ok(chunk)
    }

    /// 写后读回。不一致时指出第一个差异字节的位置——"不一致"三个字不足以
    /// 定位问题，位置才是。
    async fn verify(&mut self, address: Address, expected: &[u8]) -> Result<(), Fault> {
        let len = u16::try_from(expected.len())
            .ok()
            .and_then(ByteLen::new)
            .ok_or_else(|| {
                connection::fault(
                    seq::WRITE,
                    Severity::Error,
                    format!("写入长度 {} 字节，超出单次可读范围", expected.len()),
                )
            })?;
        let read_back = self.read(address, len).await?;
        if read_back == expected {
            return Ok(());
        }
        let at = (0..expected.len())
            .find(|index| read_back.get(*index) != Some(&expected[*index]))
            .unwrap_or(0);
        let actual = read_back
            .get(at)
            .map_or_else(|| "响应提前结束".to_string(), |byte| format!("{byte:#04x}"));
        Err(connection::fault(
            seq::VERIFY,
            Severity::Error,
            format!(
                "写后读回不一致：{address} 起第 {at} 字节应为 {:#04x}，读回 {actual}",
                expected[at]
            ),
        )
        .with_hint("确认该地址没有被 ECU 镜像或量程裁剪，且资源已解锁"))
    }

    /// 按 list 逐个分配 ODT 与条目。
    async fn allocate_odts(
        &mut self,
        odts_per_list: &[Vec<Vec<daq::DaqSignal>>],
    ) -> Result<(), Fault> {
        for (index, groups) in odts_per_list.iter().enumerate() {
            let daq_list = index as u16;
            self.transact(&Command::AllocOdt {
                daq_list,
                odt_count: groups.len() as u8,
            })
            .await?;
            for (odt, group) in groups.iter().enumerate() {
                self.transact(&Command::AllocOdtEntry {
                    daq_list,
                    odt: odt as u8,
                    entries: group.len() as u8,
                })
                .await?;
            }
        }
        Ok(())
    }

    /// 写条目并设模式。每个 ODT 只发一次 SET_DAQ_PTR：从站写后自增指针，
    /// 这是 pyXCP 依赖的行为，标准也这么规定。
    async fn write_entries(
        &mut self,
        lists: &[DaqListSpec],
        odts_per_list: &[Vec<Vec<daq::DaqSignal>>],
    ) -> Result<(), Fault> {
        for (index, (spec, groups)) in lists.iter().zip(odts_per_list).enumerate() {
            let daq_list = index as u16;
            for (odt, group) in groups.iter().enumerate() {
                self.transact(&Command::SetDaqPtr {
                    daq_list,
                    odt: odt as u8,
                    entry: 0,
                })
                .await?;
                for signal in group {
                    self.transact(&Command::WriteDaq {
                        bit_offset: 0,
                        size: signal.len.get() as u8,
                        address_extension: spec.address_extension,
                        address: signal.address.get(),
                    })
                    .await?;
                }
            }
            // timestamp 位按调用方声明透传：参考实现（XCPlite）强制要
            // 这一位，自研从站忽略它——DTO 是否真带时间戳由对端定。
            let mut mode = 0;
            if spec.timestamp {
                mode |= 0x10;
            }
            self.transact(&Command::SetDaqListMode {
                mode,
                daq_list,
                event: spec.event,
                prescaler: spec.prescaler,
                priority: 0,
            })
            .await?;
        }
        Ok(())
    }

    /// 逐条启动 DAQ list，并用从站报的首个 PID 建布局。
    ///
    /// 从站的编号是权威：master 按"跨 list 连续"算一遍，两者不一致说明对
    /// 绝对 ODT 编号的理解有分歧，此时收到的 DTO 必然切错，宁可现在就失败。
    ///
    /// 对端不支持逐 list 启动时（参考实现 XCPlite 的测试构建回 0x27）
    /// 回退到 SELECT + START_STOP_SYNCH：此时拿不到从站的 firstPid（它
    /// 的 DTO 头也不是 PID 字节），布局按 master 的连续分配，核对豁免
    /// 并留下日志。
    async fn start_and_map(
        &mut self,
        odts_per_list: &[Vec<Vec<daq::DaqSignal>>],
    ) -> Result<DaqLayout, Fault> {
        let mut layout = DaqLayout::default();
        let mut next_pids = OdtIds::new();
        let mut synch_fallback = false;
        for (index, groups) in odts_per_list.iter().enumerate() {
            let expected = next_pids.allocate(groups.len() as u8).ok_or_else(|| {
                connection::fault(
                    seq::DAQ,
                    Severity::Error,
                    "DAQ list 多到绝对 ODT 编号用尽（上限 0xFB）",
                )
            })?;
            let reported = match self.start_list(index as u16).await {
                Ok(pid) => pid,
                Err(fault) => {
                    if !fault.message().contains("0x27") {
                        return Err(fault);
                    }
                    synch_fallback = true;
                    self.transact(&Command::StartStopDaqList {
                        mode: 2,
                        daq_list: index as u16,
                    })
                    .await?;
                    tracing::warn!(
                        list = index,
                        "从站不支持逐 list 启动：改用 SELECT+SYNCH，跳过从站 firstPid 核对"
                    );
                    expected
                }
            };
            if reported != expected {
                return Err(connection::fault(
                    seq::DAQ,
                    Severity::Error,
                    format!(
                        "从站把 list {index} 的首个 ODT 编成 {reported}，master 按连续分配算是 {expected}"
                    ),
                )
                .with_hint("两侧对绝对 ODT 编号的约定不一致，DAQ 帧无法还原成信号"));
            }
            for (odt, group) in groups.iter().enumerate() {
                layout.insert(expected + odt as u8, group);
            }
        }
        if synch_fallback {
            self.transact(&Command::StartStopSynch { mode: 1 }).await?;
        }
        Ok(layout)
    }

    /// 启动一条 list，返回从站报的首个绝对 ODT 编号。
    async fn start_list(&mut self, daq_list: u16) -> Result<u8, Fault> {
        let payload = self
            .transact(&Command::StartStopDaqList { mode: 1, daq_list })
            .await?;
        first_pid(&payload)
    }
}

/// 解析 `START_STOP_DAQ_LIST` 响应里的首个 PID（u16 小端，只用低字节）。
fn first_pid(payload: &[u8]) -> Result<u8, Fault> {
    let low = payload.first().copied().ok_or_else(|| {
        connection::fault(
            seq::DAQ,
            Severity::Error,
            "START_STOP_DAQ_LIST 响应为空，取不到 firstPid",
        )
    })?;
    if low > openape_xcp_proto::PID_DAQ_MAX {
        return Err(connection::fault(
            seq::DAQ,
            Severity::Error,
            format!("从站报的 firstPid {low:#04x} 落在命令 PID 区间"),
        ));
    }
    Ok(low)
}

/// 取前 `wanted` 字节；不足即报协议错，不拿填充字节冒充数据。
fn take_exact(payload: Vec<u8>, wanted: usize) -> Result<Vec<u8>, Fault> {
    if payload.len() < wanted {
        return Err(connection::fault(
            seq::PROTOCOL,
            Severity::Error,
            format!("要求 {wanted} 字节，从站只回了 {} 字节", payload.len()),
        ));
    }
    Ok(payload[..wanted].to_vec())
}

/// 解析 CONNECT 响应。CAN 上参数放不全时版本字段缺席，不臆造。
fn parse_connect_info(payload: &[u8]) -> Result<ConnectInfo, Fault> {
    // 标准 CONNECT 响应布局：resource(1) commMode(1) maxCto(byte)
    // maxDto(LE word) protocol 版本 transport 版本，共 7 字节参数。
    // CAN 8 字节帧恰好装下（PID + 7）；UDP 上从站也按标准回。
    if payload.len() < 5 {
        return Err(connection::fault(
            seq::PROTOCOL,
            Severity::Error,
            format!("CONNECT 响应只有 {} 字节，至少需要 5 字节", payload.len()),
        ));
    }
    let max_cto = u16::from(payload[2]);
    let max_dto = u16::from_le_bytes([payload[3], payload[4]]);
    if max_cto == 0 {
        return Err(connection::fault(
            seq::PROTOCOL,
            Severity::Error,
            "CONNECT 响应的 maxCto 为 0，说明响应被截断或格式不符",
        )
        .with_hint("确认从站按标准回的响应是 PID + 命令码 + 参数"));
    }
    // 版本域只有 major；minor 由 GET_VERSION 给，那里没实现就归零。
    // 用 then 而不是 then_some：后者急切求值，短响应会越界。
    let protocol_version = (payload.len() >= 6).then(|| (payload[5], 0));
    let transport_version = (payload.len() >= 7).then(|| (payload[6], 0));
    Ok(ConnectInfo {
        resource: payload[0],
        comm_mode_basic: payload[1],
        max_cto,
        max_dto,
        protocol_version,
        transport_version,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use openape_xcp_proto::PID_DAQ_MAX;

    #[test]
    fn first_pid_accepts_the_last_daq_number_and_rejects_the_command_range() {
        assert_eq!(first_pid(&[0x00]).ok(), Some(0));
        assert_eq!(first_pid(&[PID_DAQ_MAX]).ok(), Some(PID_DAQ_MAX));
        assert!(first_pid(&[PID_DAQ_MAX + 1]).is_err(), "0xFC 起是命令 PID");
        assert!(first_pid(&[]).is_err(), "空响应取不到编号");
    }

    #[test]
    fn connect_info_reads_versions_only_when_the_frame_holds_them() {
        // 标准布局：resource、comm、maxCto(byte)、maxDto(LE word)、
        // protocol major、transport major。6 字节只有协议版本，
        // 7 字节两者都有。
        let full = [0x05, 0x00, 8, 8, 0, 1];
        let info = parse_connect_info(&full).expect("6 字节合法");
        assert_eq!(info.resource, 0x05);
        assert_eq!(info.comm_mode_basic, 0x00, "取第 1 字节，不是第 0 字节");
        assert_eq!(info.max_cto, 8);
        assert_eq!(info.max_dto, 8);
        assert_eq!(info.protocol_version, Some((1, 0)));
        assert_eq!(info.transport_version, None);

        let full = [0x05, 0x00, 8, 8, 0, 1, 1];
        let info = parse_connect_info(&full).expect("7 字节合法");
        assert_eq!(info.transport_version, Some((1, 0)));
    }

    #[test]
    fn connect_info_rejects_short_and_zero_max_cto() {
        assert!(
            parse_connect_info(&[0x05, 0x00, 8, 8]).is_err(),
            "不足 5 字节"
        );
        assert!(
            parse_connect_info(&[0x05, 0x00, 0, 8, 0]).is_err(),
            "maxCto 为 0 说明响应被截断"
        );
    }
}
