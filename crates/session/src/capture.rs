//! DAQ 采集泵：从总线连续收 DTO 帧，解码后推进样本流。
//!
//! 一次采集只有一个读者。XCP 的 CTO 响应与 DTO 数据走同一个接收端，两个任务
//! 同时 `recv` 会互相吃掉对方的帧——所以本阶段由 daemon 独占设备与总线：它既
//! 是唯一读者，也是唯一命令发起方（M2-14 的 `--via-daemon` 复用同一会话）。

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use openape_bus::{Bus, BusError, CanId};
use openape_diag::Metrics;
use openape_xcp::DaqLayout;
use openape_xcp_proto::dto_profile::{self, DtoProfile};
use tokio::task::JoinHandle;

use crate::daq_stream::{DEFAULT_CAPACITY, DaqStats, DaqStream};

/// 每次等待接收的时长。短一些让采集泵对任务中止更敏感。
const RECV_SLICE: Duration = Duration::from_millis(50);

/// 一次运行中的采集。丢弃即停泵，样本流随之关闭。
#[derive(Debug)]
pub struct DaqCapture {
    stream: Arc<DaqStream>,
    paused: Arc<AtomicBool>,
    task: JoinHandle<()>,
}

impl DaqCapture {
    /// 样本流。多个消费者各自 `subscribe`。
    pub fn stream(&self) -> &DaqStream {
        &self.stream
    }

    /// 采集侧的统计。
    pub fn stats(&self) -> DaqStats {
        self.stream.stats()
    }

    /// 暂停或恢复。
    ///
    /// 暂停期间泵不读总线，命令事务才有机会收到自己的响应帧——两者共用一个
    /// 接收端，谁都不能一边抢帧一边等回应。代价是暂停期间的 DTO 直接丢掉，
    /// 不会记进缓冲，所以别把它当常态。
    pub fn set_paused(&self, paused: bool) {
        self.paused.store(paused, Ordering::Relaxed);
    }
}

impl Drop for DaqCapture {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// 起泵。`layout` 来自 `configure_daq`，用它判断哪些 DTO 编号属于本次采集。
pub(crate) fn spawn(
    bus: Arc<dyn Bus>,
    response_id: CanId,
    layout: DaqLayout,
    config: &CaptureConfig,
) -> DaqCapture {
    let capacity = config.capacity;
    let metrics = config.metrics.clone();
    let stream = Arc::new(match metrics {
        Some(metrics) => DaqStream::with_metrics(capacity, metrics),
        None => DaqStream::new(capacity),
    });
    let paused = Arc::new(AtomicBool::new(false));
    let worker = Arc::clone(&stream);
    let gate = Arc::clone(&paused);
    let dto = config.dto;
    let task = tokio::spawn(async move { pump(bus, response_id, layout, dto, worker, gate).await });
    DaqCapture {
        stream,
        paused,
        task,
    }
}

async fn pump(
    bus: Arc<dyn Bus>,
    response_id: CanId,
    layout: DaqLayout,
    dto: &'static DtoProfile,
    stream: Arc<DaqStream>,
    paused: Arc<AtomicBool>,
) {
    let mut warned = false;
    loop {
        if paused.load(Ordering::Relaxed) {
            // 让出接收端给命令事务。
            tokio::time::sleep(Duration::from_millis(5)).await;
            continue;
        }
        match bus.recv(RECV_SLICE).await {
            Ok(frame) => {
                // 命令响应与 DAQ 数据共用接收端：只认响应标识符上的帧，别把
                // CTO 当成 DTO。
                if frame.id != response_id {
                    continue;
                }
                // 按对端档案剥头（D-27）。档案不符或帧被截断就当它不是 DTO
                // 跳过——头长猜错比丢一帧糟得多：头字节混进载荷会让曲线看着
                // 有数、实际全错。
                let Ok(decoded) = dto_profile::split(dto, &frame.data) else {
                    stream.record_rejected();
                    // 只喊一次：一万帧里每帧喊一声只会把日志刷满，而这通常
                    // 是「档案配错」这一个确定性错误，不是一个持续故障。
                    if !warned {
                        warned = true;
                        tracing::warn!(
                            profile = dto.name,
                            "有帧按 DTO 档案解不出来（已计入 rejected）：若整段采集一条样本都没有，多半是 --dto-header 与从站不符"
                        );
                    }
                    continue;
                };
                // 不认识的 ODT 编号不猜：多半是上一轮采集的残留帧。
                if layout.decode(decoded.pid, decoded.payload).is_none() {
                    continue;
                }
                stream.push_at(
                    decoded.pid,
                    frame.timestamp.unwrap_or_else(Instant::now),
                    decoded.payload.to_vec(),
                );
            }
            Err(BusError::Timeout { .. }) => continue,
            // 总线没了，泵也就没有输入了。留下计数与已入队的样本供上层收尾。
            Err(_) => break,
        }
    }
}

/// 采集参数。
#[derive(Debug, Clone)]
pub struct CaptureConfig {
    /// 缓冲容量（样本数）。
    pub capacity: usize,
    /// 指标出口。给了它，`push` 才会更新 `daq_frames` 与 `queue_depth`——
    /// 不挂的话指标恒为零，看起来像"没在采集"。
    pub metrics: Option<Arc<Metrics>>,
    /// 对端的 DTO 头布局档案（D-27）。默认是标准 `[PID][数据]`；连私有头
    /// 的从站（如 XCPlite）必须换成对应档案，否则头字节会被当成载荷。
    pub dto: &'static DtoProfile,
}

impl Default for CaptureConfig {
    fn default() -> Self {
        Self {
            capacity: DEFAULT_CAPACITY,
            metrics: None,
            dto: &dto_profile::STANDARD,
        }
    }
}

impl CaptureConfig {
    /// 顺带更新指标出口。
    pub fn with_metrics(mut self, metrics: Arc<Metrics>) -> Self {
        self.metrics = Some(metrics);
        self
    }

    /// 按对端档案切 DTO。档案来自 `openape_xcp_proto::dto_profile`。
    pub fn with_dto(mut self, dto: &'static DtoProfile) -> Self {
        self.dto = dto;
        self
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::Mutex;

    use async_trait::async_trait;
    use openape_bus::{BusCapabilities, BusError, CanFrame};
    use openape_cal_core::ByteLen;
    use openape_xcp::DaqLayout;

    use super::*;

    /// 按顺序吐出预先塞进去的帧，吐完就超时。
    struct ScriptedBus {
        frames: Mutex<VecDeque<CanFrame>>,
    }

    impl ScriptedBus {
        fn new(frames: Vec<CanFrame>) -> Self {
            Self {
                frames: Mutex::new(frames.into()),
            }
        }
    }

    #[async_trait]
    impl Bus for ScriptedBus {
        fn backend(&self) -> &'static str {
            "scripted"
        }

        fn channel(&self) -> &str {
            "mock0"
        }

        fn capabilities(&self) -> BusCapabilities {
            BusCapabilities::CLASSIC_SOFT_TIMESTAMP
        }

        async fn send(&self, _frame: CanFrame) -> Result<(), BusError> {
            Ok(())
        }

        async fn recv(&self, timeout: Duration) -> Result<CanFrame, BusError> {
            let frame = self.frames.lock().expect("锁未中毒").pop_front();
            match frame {
                Some(frame) => Ok(frame),
                // 夹具必须遵守契约：期限内等待再报超时。立刻返回超时会让采集
                // 泵空转（真实总线不是这样），测试会挂死而不是失败。
                None => {
                    tokio::time::sleep(timeout).await;
                    Err(BusError::Timeout {
                        waited_ms: u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX),
                    })
                }
            }
        }
    }

    fn dto(pid: u8, values: &[u8]) -> CanFrame {
        let mut data = vec![pid];
        data.extend_from_slice(values);
        CanFrame {
            id: CanId::standard(0x601).expect("合法"),
            data,
            timestamp: None,
        }
    }

    /// 整帧原样送出（帧首不强行当 PID：私有头的从站首字节不一定是我们认的编号）。
    fn raw(data: &[u8]) -> CanFrame {
        CanFrame {
            id: CanId::standard(0x601).expect("合法"),
            data: data.to_vec(),
            timestamp: None,
        }
    }

    fn layout_with(pid: u8, len: u16) -> DaqLayout {
        let mut layout = DaqLayout::default();
        layout.insert_odt(pid, &[ByteLen::new(len).expect("测试长度非零")]);
        layout
    }

    #[tokio::test]
    async fn a_dto_frame_becomes_one_sample() {
        let bus = Arc::new(ScriptedBus::new(vec![dto(0, &[0x11, 0x22, 0x33])]));
        let capture = spawn(
            bus,
            CanId::standard(0x601).expect("合法"),
            layout_with(0, 3),
            &CaptureConfig::default(),
        );
        let mut reader = capture.stream().subscribe();
        let sample = tokio::time::timeout(Duration::from_secs(2), reader.recv())
            .await
            .expect("不该超时")
            .expect("应有一条样本");
        assert_eq!(sample.pid, 0);
        assert_eq!(sample.payload, vec![0x11, 0x22, 0x33]);
    }

    #[tokio::test]
    async fn frames_on_other_ids_and_unknown_pids_are_ignored() {
        let mut wrong_id = dto(0, &[1]);
        wrong_id.id = CanId::standard(0x600).expect("合法");
        let bus = Arc::new(ScriptedBus::new(vec![
            wrong_id,
            dto(9, &[7]),
            dto(0, &[0xAA]),
        ]));
        let capture = spawn(
            bus,
            CanId::standard(0x601).expect("合法"),
            layout_with(0, 1),
            &CaptureConfig::default(),
        );
        let mut reader = capture.stream().subscribe();
        let sample = tokio::time::timeout(Duration::from_secs(2), reader.recv())
            .await
            .expect("不该超时")
            .expect("应有一条样本");
        assert_eq!(sample.payload, vec![0xAA]);
        assert_eq!(capture.stats().produced, 1, "被跳过的两帧不该计数");
    }

    #[tokio::test]
    async fn a_private_header_dto_is_split_by_the_profile() {
        // XCPlite 的私有头（D-27）：相对 ODT、0xAA 标记、DAQ list、时间戳，
        // 之后才是载荷——头字节绝不能进载荷。
        let frame = raw(&[
            0x00, 0xAA, 0x00, 0x00, 0x03, 0x02, 0x01, 0x00, 0x11, 0x22, 0x33, 0x44,
        ]);
        let bus = Arc::new(ScriptedBus::new(vec![frame]));
        let config = CaptureConfig::default().with_dto(&openape_xcp_proto::dto_profile::XCPLITE);
        let capture = spawn(
            bus,
            CanId::standard(0x601).expect("合法"),
            layout_with(0, 4),
            &config,
        );
        let mut reader = capture.stream().subscribe();
        let sample = tokio::time::timeout(Duration::from_secs(2), reader.recv())
            .await
            .expect("不该超时")
            .expect("应有一条样本");
        assert_eq!(sample.pid, 0, "相对编号就是 PID 字节");
        assert_eq!(sample.payload, vec![0x11, 0x22, 0x33, 0x44]);
    }

    #[tokio::test]
    async fn a_frame_shorter_than_the_profile_header_is_skipped() {
        // 标准档案下这条私有头帧能"解出"11 字节载荷，属于档案配错的成本；
        // 但真正截断的帧（不够头长）必须跳过，不能靠补零凑出来。
        let bus = Arc::new(ScriptedBus::new(vec![
            raw(&[0x00, 0xAA]),
            raw(&[0x01, 0x11]),
        ]));
        let config = CaptureConfig::default().with_dto(&openape_xcp_proto::dto_profile::XCPLITE);
        let capture = spawn(
            bus,
            CanId::standard(0x601).expect("合法"),
            layout_with(0, 1),
            &config,
        );
        let mut reader = capture.stream().subscribe();
        assert!(
            tokio::time::timeout(Duration::from_millis(300), reader.recv())
                .await
                .is_err(),
            "头长不足的帧不该变成样本"
        );
        assert_eq!(capture.stats().produced, 0);
        assert_eq!(
            capture.stats().rejected,
            2,
            "两帧都按档案不符计数——配错档案时这个数会一直涨"
        );
    }
}
