//! DAQ 采集泵：从总线连续收 DTO 帧，解码后推进样本流。
//!
//! 一次采集只有一个读者。XCP 的 CTO 响应与 DTO 数据走同一个接收端，两个任务
//! 同时 `recv` 会互相吃掉对方的帧——所以本阶段由 daemon 独占设备与总线：它既
//! 是唯一读者，也是唯一命令发起方（M2-14 的 `--via-daemon` 复用同一会话）。

use std::sync::Arc;
use std::time::{Duration, Instant};

use openape_bus::{Bus, BusError, CanId};
use openape_diag::Metrics;
use openape_xcp::DaqLayout;
use openape_xcp_proto::dto_profile::{self, DtoProfile, PidKind};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

use crate::daq_stream::{DEFAULT_CAPACITY, DaqStats, DaqStream};

/// 每次等待接收的时长。暂停握手的确认窗口也由它推出来。
const RECV_SLICE: Duration = Duration::from_millis(50);

/// 等泵自己收工的宽限期。
const SHUTDOWN_GRACE: Duration = Duration::from_secs(1);

/// 命令通道容量。命令本身只有暂停/恢复/收工三种，留几个位足够。
const CMD_QUEUE: usize = 8;

/// 采集泵的控制命令。
///
/// 为什么用命令通道而不是共享标志位：暂停要**可确认**。置一个 `AtomicBool`
/// 之后泵若正卡在 `bus.recv(50ms)` 里，它仍会吃掉命令事务正在等的响应帧——
/// 那是个真竞态，只是窗口小到平时看不见。
enum PumpCmd {
    /// 暂停并回执：泵收到后先回 ack，再在命令通道上等下一步。
    Pause(oneshot::Sender<()>),
    /// 恢复。不需要回执：晚一个 recv 周期恢复不影响正确性。
    Resume,
    /// 正常收工：泵在两次 recv 之间自己退出。
    Shutdown,
}

/// 一次运行中的采集。丢弃即停泵，样本流随之关闭。
#[derive(Debug)]
pub struct DaqCapture {
    stream: Arc<DaqStream>,
    cmd: mpsc::Sender<PumpCmd>,
    task: Option<JoinHandle<()>>,
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

    /// 暂停采集并等泵确认。
    ///
    /// 回执之后泵才真的不再 `recv`，命令事务这时拿得到自己的响应帧。暂停期间
    /// 的 DTO 仍会留在总线接收队列里，恢复后按序处理——不是丢弃。
    ///
    /// 超时只 warn 不报错：泵可能已经因为总线没了而退出，这不该让写路径失败，
    /// 但必须留痕——否则"暂停没生效"会以偶发错帧的形式出现在很久以后。
    pub async fn pause(&self) {
        let (ack_tx, ack_rx) = oneshot::channel();
        if self.cmd.send(PumpCmd::Pause(ack_tx)).await.is_err() {
            tracing::warn!("采集泵已退出，暂停命令没有送达");
            return;
        }
        // 窗口取两个 recv 周期：泵最多在途一个 recv，留一倍余量给调度。
        let window = RECV_SLICE * 2;
        match tokio::time::timeout(window, ack_rx).await {
            Ok(Ok(())) => {}
            Ok(Err(_)) => tracing::warn!("采集泵在确认前退出，暂停只到了半路"),
            Err(_) => tracing::warn!(
                window_ms = window.as_millis(),
                "采集泵没有在窗口内确认暂停：命令响应帧可能仍被泵吃掉"
            ),
        }
    }

    /// 恢复采集。不需要确认，队列满或泵已退出都只留痕。
    pub fn resume(&self) {
        if self.cmd.try_send(PumpCmd::Resume).is_err() {
            tracing::warn!("采集泵已退出或命令队列已满，恢复命令没有送达");
        }
    }

    /// 让泵自己收工。
    ///
    /// 为什么不是 `CancellationToken`：泵不持锁、不写文件，abort 不会破坏任何
    /// 不变量，所以 [`Drop`] 里的 abort 可以继续当兜底。但正常路径走命令通道，
    /// 泵在两次 recv 之间自己退出，样本流的关闭与计数收尾都还能走到。
    pub async fn shutdown(mut self) {
        let _ = self.cmd.send(PumpCmd::Shutdown).await;
        if let Some(task) = self.task.take()
            && tokio::time::timeout(SHUTDOWN_GRACE, task).await.is_err()
        {
            tracing::warn!("采集泵没有在宽限期内退出，交给 Drop 里的 abort");
        }
    }
}

impl Drop for DaqCapture {
    fn drop(&mut self) {
        // 兜底：正常路径已由 `shutdown` 收工，走到这里说明调用方直接丢了句柄。
        if let Some(task) = self.task.take() {
            task.abort();
        }
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
    let (cmd, cmd_rx) = mpsc::channel(CMD_QUEUE);
    let worker = Arc::clone(&stream);
    let dto = config.dto;
    let task =
        tokio::spawn(async move { pump(bus, response_id, layout, dto, worker, cmd_rx).await });
    DaqCapture {
        stream,
        cmd,
        task: Some(task),
    }
}

async fn pump(
    bus: Arc<dyn Bus>,
    response_id: CanId,
    layout: DaqLayout,
    dto: &'static DtoProfile,
    stream: Arc<DaqStream>,
    mut cmd_rx: mpsc::Receiver<PumpCmd>,
) {
    let mut warned = false;
    loop {
        // 命令优先于收帧：暂停必须在下一次 recv 之前生效，否则它会吃掉命令
        // 事务正在等的响应帧。
        match cmd_rx.try_recv() {
            Ok(PumpCmd::Pause(ack)) => {
                let _ = ack.send(());
                // 暂停就是等：直到收到恢复或收工。等待期间不持锁、不写文件，
                // 没有东西需要收尾。
                loop {
                    match cmd_rx.recv().await {
                        Some(PumpCmd::Resume) => break,
                        // 重复暂停直接补回执，别让第二个调用方等满窗口。
                        Some(PumpCmd::Pause(ack)) => {
                            let _ = ack.send(());
                        }
                        Some(PumpCmd::Shutdown) | None => return,
                    }
                }
                continue;
            }
            Ok(PumpCmd::Resume) => continue,
            Ok(PumpCmd::Shutdown) => return,
            Err(mpsc::error::TryRecvError::Empty) => {}
            // 发送端没了说明句柄已丢弃，泵没有存在的理由。
            Err(mpsc::error::TryRecvError::Disconnected) => return,
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
                // 编号语义由档案声明（D-27）：绝对号直接用；list 内相对号
                // 要用「该 list 的首个绝对编号」还原——拿相对号当绝对号用
                // 能解出数，但解的是别的信号，比报错难查得多。
                let pid = match dto.pid {
                    PidKind::Absolute => Some(decoded.pid),
                    PidKind::RelativeToDaqList => decoded
                        .daq_list
                        .and_then(|list| layout.absolute_pid(list, decoded.pid)),
                };
                let Some(pid) = pid else {
                    stream.record_rejected();
                    continue;
                };
                // 不认识的 ODT 编号不猜：多半是上一轮采集的残留帧。
                if layout.decode(pid, decoded.payload).is_none() {
                    continue;
                }
                stream.push_at(
                    pid,
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
    async fn a_relative_pid_becomes_the_absolute_odt_number() {
        // 私有头带 list 号与 list 内相对号：绝对号 = 该 list 首号 + 相对号。
        // 采样流里的 pid 必须是绝对号，否则查 DaqLayout 一定错位。
        let frame = raw(&[0x01, 0xAA, 0x00, 0x00, 0x03, 0x02, 0x01, 0x00, 0x44, 0x55]);
        let bus = Arc::new(ScriptedBus::new(vec![frame]));
        let mut layout = DaqLayout::default();
        layout.insert_list(0, 4);
        layout.insert_odt(5, &[ByteLen::new(2).expect("非零")]);
        let config = CaptureConfig::default().with_dto(&openape_xcp_proto::dto_profile::XCPLITE);
        let capture = spawn(bus, CanId::standard(0x601).expect("合法"), layout, &config);
        let mut reader = capture.stream().subscribe();
        let sample = tokio::time::timeout(Duration::from_secs(2), reader.recv())
            .await
            .expect("不该超时")
            .expect("应有一条样本");
        assert_eq!(sample.pid, 5, "相对 1 + 首号 4");
        assert_eq!(sample.payload, vec![0x44, 0x55]);
    }

    #[tokio::test]
    async fn a_relative_pid_with_no_known_list_is_rejected_not_guessed() {
        // 档案说编号是相对的，帧里却没有可用 list 首号：跳过并计数，
        // 绝不把相对号当绝对号用——那样能解出数，但解的是别人的信号。
        let frame = raw(&[0x00, 0xAA, 0x07, 0x00, 0x03, 0x02, 0x01, 0x00, 0x11, 0x22]);
        let bus = Arc::new(ScriptedBus::new(vec![frame]));
        let config = CaptureConfig::default().with_dto(&openape_xcp_proto::dto_profile::XCPLITE);
        let capture = spawn(
            bus,
            CanId::standard(0x601).expect("合法"),
            layout_with(0, 2),
            &config,
        );
        let mut reader = capture.stream().subscribe();
        assert!(
            tokio::time::timeout(Duration::from_millis(300), reader.recv())
                .await
                .is_err(),
            "没有 list 首号就不该出样本"
        );
        assert_eq!(capture.stats().produced, 0);
        assert_eq!(capture.stats().rejected, 1);
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
        // list 0 的首号是 0：相对 0 就还原成绝对 0，与本例登记的 ODT 对上。
        let mut layout = layout_with(0, 4);
        layout.insert_list(0, 0);
        let capture = spawn(bus, CanId::standard(0x601).expect("合法"), layout, &config);
        let mut reader = capture.stream().subscribe();
        let sample = tokio::time::timeout(Duration::from_secs(2), reader.recv())
            .await
            .expect("不该超时")
            .expect("应有一条样本");
        assert_eq!(sample.pid, 0, "相对 0 + 首号 0 还原成绝对 0");
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

    /// 可以中途喂帧的总线：推一帧才收一帧，空闲时按契约等满超时再报。
    ///
    /// 与 [`ScriptedBus`] 的区别是帧的到达时机可控——验证"暂停期间没有样本"
    /// 需要在泵停下来之后再放帧进去。
    struct ChannelBus {
        frames: tokio::sync::Mutex<tokio::sync::mpsc::Receiver<CanFrame>>,
    }

    impl ChannelBus {
        fn pair() -> (Self, tokio::sync::mpsc::Sender<CanFrame>) {
            let (tx, rx) = tokio::sync::mpsc::channel(8);
            (
                Self {
                    frames: tokio::sync::Mutex::new(rx),
                },
                tx,
            )
        }
    }

    #[async_trait]
    impl Bus for ChannelBus {
        fn backend(&self) -> &'static str {
            "channel"
        }

        fn channel(&self) -> &str {
            "mock1"
        }

        fn capabilities(&self) -> BusCapabilities {
            BusCapabilities::CLASSIC_SOFT_TIMESTAMP
        }

        async fn send(&self, _frame: CanFrame) -> Result<(), BusError> {
            Ok(())
        }

        async fn recv(&self, timeout: Duration) -> Result<CanFrame, BusError> {
            let mut frames = self.frames.lock().await;
            match tokio::time::timeout(timeout, frames.recv()).await {
                Ok(Some(frame)) => Ok(frame),
                Ok(None) => Err(BusError::Closed),
                Err(_) => Err(BusError::Timeout {
                    waited_ms: u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX),
                }),
            }
        }
    }

    fn response_id() -> CanId {
        CanId::standard(0x601).expect("合法")
    }

    #[tokio::test(start_paused = true)]
    async fn a_paused_pump_produces_no_samples() {
        let (bus, frames) = ChannelBus::pair();
        let capture = spawn(
            Arc::new(bus),
            response_id(),
            layout_with(0, 1),
            &CaptureConfig::default(),
        );
        let mut reader = capture.stream().subscribe();

        frames.send(dto(0, &[0x11])).await.expect("喂帧");
        let first = tokio::time::timeout(Duration::from_secs(2), reader.recv())
            .await
            .expect("不该超时")
            .expect("应有一条样本");
        assert_eq!(first.payload, vec![0x11]);

        capture.pause().await;
        frames.send(dto(0, &[0x22])).await.expect("喂帧");
        assert!(
            tokio::time::timeout(Duration::from_millis(500), reader.recv())
                .await
                .is_err(),
            "暂停期间不该出样本"
        );
        assert_eq!(capture.stats().produced, 1, "暂停期间的帧一帧都不该计");
    }

    #[tokio::test(start_paused = true)]
    async fn pause_hands_the_receiver_back_to_the_caller() {
        // 暂停的意义就在这里：回执之后泵真的不再 recv，命令事务才拿得到自己的
        // 响应帧。旧实现只置标志位，泵卡在 recv 里仍会吃掉它。
        let (bus, frames) = ChannelBus::pair();
        let bus: Arc<dyn Bus> = Arc::new(bus);
        let capture = spawn(
            Arc::clone(&bus),
            response_id(),
            layout_with(0, 1),
            &CaptureConfig::default(),
        );

        capture.pause().await;
        frames.send(dto(0, &[0x99])).await.expect("喂帧");
        let frame = tokio::time::timeout(Duration::from_millis(500), bus.recv(RECV_SLICE))
            .await
            .expect("暂停后调用方要能直接收到帧")
            .expect("帧在队列里");
        assert_eq!(frame.data, vec![0, 0x99]);
    }

    #[tokio::test(start_paused = true)]
    async fn resuming_after_a_pause_keeps_the_stream_going() {
        let (bus, frames) = ChannelBus::pair();
        let capture = spawn(
            Arc::new(bus),
            response_id(),
            layout_with(0, 1),
            &CaptureConfig::default(),
        );
        let mut reader = capture.stream().subscribe();

        capture.pause().await;
        // 暂停期间放进去的帧留在信道里，恢复后按序出来。
        frames.send(dto(0, &[0x22])).await.expect("喂帧");
        capture.resume();

        let next = tokio::time::timeout(Duration::from_secs(2), reader.recv())
            .await
            .expect("恢复后不该超时")
            .expect("应有一条样本");
        assert_eq!(next.payload, vec![0x22], "暂停期间排着的帧要接着出");

        frames.send(dto(0, &[0x33])).await.expect("喂帧");
        let after = tokio::time::timeout(Duration::from_secs(2), reader.recv())
            .await
            .expect("恢复后不该超时")
            .expect("应有一条样本");
        assert_eq!(after.payload, vec![0x33], "后续流要照常推进");
    }

    #[tokio::test(start_paused = true)]
    async fn shutdown_lets_the_pump_finish_on_its_own() {
        let (bus, _frames) = ChannelBus::pair();
        let capture = spawn(
            Arc::new(bus),
            response_id(),
            layout_with(0, 1),
            &CaptureConfig::default(),
        );
        let mut reader = capture.stream().subscribe();
        capture.shutdown().await;
        // 泵退出即停泵，样本流随之关闭：订阅者拿到"流已结束"而不是一直悬着。
        assert!(
            tokio::time::timeout(Duration::from_millis(100), reader.recv())
                .await
                .is_ok(),
            "泵退出后流要关掉"
        );
        assert!(reader.recv().await.is_none(), "流结束应给出 None");
    }
}
