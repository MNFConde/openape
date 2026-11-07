//! DAQ 采集泵：从总线连续收 DTO 帧，解码后推进样本流。
//!
//! 一次采集只有一个读者。XCP 的 CTO 响应与 DTO 数据走同一个接收端，两个任务
//! 同时 `recv` 会互相吃掉对方的帧——所以本阶段由 daemon 独占设备与总线：它既
//! 是唯一读者，也是唯一命令发起方（M2-14 的 `--via-daemon` 复用同一会话）。

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use openape_bus::{Bus, BusError, CanId};
use openape_xcp::DaqLayout;
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
    capacity: usize,
) -> DaqCapture {
    let stream = Arc::new(DaqStream::new(capacity));
    let paused = Arc::new(AtomicBool::new(false));
    let worker = Arc::clone(&stream);
    let gate = Arc::clone(&paused);
    let task = tokio::spawn(async move { pump(bus, response_id, layout, worker, gate).await });
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
    stream: Arc<DaqStream>,
    paused: Arc<AtomicBool>,
) {
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
                let Some((&pid, payload)) = frame.data.split_first() else {
                    continue;
                };
                // 不认识的 ODT 编号不猜：多半是上一轮采集的残留帧。
                if layout.decode(pid, payload).is_none() {
                    continue;
                }
                stream.push_at(
                    pid,
                    frame.timestamp.unwrap_or_else(Instant::now),
                    payload.to_vec(),
                );
            }
            Err(BusError::Timeout { .. }) => continue,
            // 总线没了，泵也就没有输入了。留下计数与已入队的样本供上层收尾。
            Err(_) => break,
        }
    }
}

/// 采集时序参数。目前只有容量，留出名目以便日后调。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CaptureConfig {
    /// 缓冲容量（样本数）。
    pub capacity: usize,
}

impl Default for CaptureConfig {
    fn default() -> Self {
        Self {
            capacity: DEFAULT_CAPACITY,
        }
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
            8,
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
            8,
        );
        let mut reader = capture.stream().subscribe();
        let sample = tokio::time::timeout(Duration::from_secs(2), reader.recv())
            .await
            .expect("不该超时")
            .expect("应有一条样本");
        assert_eq!(sample.payload, vec![0xAA]);
        assert_eq!(capture.stats().produced, 1, "被跳过的两帧不该计数");
    }
}
