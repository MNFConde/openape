//! DAQ 样本的环形缓冲与扇出：采集侧推一条，多个消费者各取一条。
//!
//! 缓冲是环形队列，满时丢最旧。这不是省事而是选择——实时曲线要的是"现在
//! 的值"，卡住的消费者拿三秒前的数据只会让画面更假。但丢掉的量必须可见：
//! 慢消费者若只表现为"曲线不动"，排查方向会指向总线，而实际原因在消费者。
//!
//! 三个计数互不替代：
//!
//! - `produced`：写进缓冲的样本数
//! - `delivered`：被消费者取走的份数（两个消费者时每条样本计两次）
//! - `dropped_slow_consumer`：因消费者落后被跳过的份数
//!
//! 与 [`openape_diag::Metrics::record_dropped_frames`] 是两个口径：那是帧
//! 没到总线侧就丢了，这里是帧到了但没人及时取。

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use openape_diag::Metrics;
use tokio::sync::broadcast;

/// 默认容量（样本数）。20 帧/秒下约 50 秒，够 UI 断线重连一次。
pub const DEFAULT_CAPACITY: usize = 1024;

/// 一条 DAQ 样本：一帧 DTO 的载荷，按 ODT 编号归属。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DaqSample {
    /// 绝对 ODT 编号。消费者用它查 [`openape_xcp::DaqLayout`] 切出各信号。
    pub pid: u8,
    /// 相对采集起点的单调时间戳（微秒）。不取墙钟，避免校时跳变把曲线拧断。
    pub timestamp_us: u64,
    /// DTO 载荷，不含帧首的 ODT 编号字节。
    pub payload: Vec<u8>,
}

/// 一次统计快照。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DaqStats {
    /// 写进缓冲的样本总数。
    pub produced: u64,
    /// 被消费者取走的份数。
    pub delivered: u64,
    /// 因消费者落后被跳过的份数。
    pub dropped_slow_consumer: u64,
    /// 当前仍在缓冲里、尚未被取走的样本数——这就是背压的可观测量。
    pub backlog: usize,
}

/// 采集侧的出口。
///
/// 不实现 `Clone`：持有它的那一处关掉，消费者就该收到结束信号。要分发给多
/// 个写入方，由调用方套 `Arc<DaqStream>`，别在这里隐式共享发送端。
#[derive(Debug)]
pub struct DaqStream {
    tx: broadcast::Sender<DaqSample>,
    epoch: Instant,
    counters: Arc<Counters>,
    metrics: Option<Arc<Metrics>>,
}

/// 三个计数。消费者也要用，所以单独成块——让读者只拿到计数，不拿到发送端，
/// 否则读者自己就把流维持活着，采集侧关不干净。
#[derive(Debug, Default)]
struct Counters {
    produced: AtomicU64,
    delivered: AtomicU64,
    dropped: AtomicU64,
}

impl DaqStream {
    /// 建一条流。`capacity` 为 0 时用 [`DEFAULT_CAPACITY`]。
    pub fn new(capacity: usize) -> Self {
        Self::build(capacity, None)
    }

    /// 建一条流，并让每次推入顺带更新指标出口。
    ///
    /// 由本类型记 `daq_frames` 而不是让采集侧另外记一笔：一帧 DTO 进缓冲就是
    /// 一条样本，两处都记会把这个数翻倍。
    pub fn with_metrics(capacity: usize, metrics: Arc<Metrics>) -> Self {
        Self::build(capacity, Some(metrics))
    }

    fn build(capacity: usize, metrics: Option<Arc<Metrics>>) -> Self {
        let capacity = if capacity == 0 {
            DEFAULT_CAPACITY
        } else {
            capacity
        };
        let (tx, _) = broadcast::channel(capacity);
        Self {
            tx,
            epoch: Instant::now(),
            counters: Arc::new(Counters::default()),
            metrics,
        }
    }

    /// 推入一条样本，时间戳取当前时刻。
    ///
    /// 返回是否有消费者接走；`false` 只表示当下无人订阅，不是错误。
    pub fn push(&self, pid: u8, payload: Vec<u8>) -> bool {
        self.push_at(pid, Instant::now(), payload)
    }

    /// 带帧时间戳推入。时间戳早于本流起点时按 0 计，不倒推出负值。
    ///
    /// 本方法不阻塞：缓冲满时丢最旧，慢消费者只丢自己那份，不回压采集侧。
    pub fn push_at(&self, pid: u8, at: Instant, payload: Vec<u8>) -> bool {
        let sample = DaqSample {
            pid,
            timestamp_us: u64::try_from(at.saturating_duration_since(self.epoch).as_micros())
                .unwrap_or(u64::MAX),
            payload,
        };
        self.counters.produced.fetch_add(1, Ordering::Relaxed);
        let taken = self.tx.send(sample).is_ok();
        if let Some(metrics) = self.metrics.as_ref() {
            metrics.record_daq_frames(1);
            metrics.set_queue_depth(self.tx.len() as u64);
        }
        taken
    }

    /// 新开一个消费者。只看到订阅之后的样本，不补历史。
    pub fn subscribe(&self) -> DaqStreamReader {
        DaqStreamReader {
            rx: self.tx.subscribe(),
            counters: Arc::clone(&self.counters),
        }
    }

    /// 取一份统计。
    pub fn stats(&self) -> DaqStats {
        DaqStats {
            produced: self.counters.produced.load(Ordering::Relaxed),
            delivered: self.counters.delivered.load(Ordering::Relaxed),
            dropped_slow_consumer: self.counters.dropped.load(Ordering::Relaxed),
            backlog: self.tx.len(),
        }
    }
}

/// 一个消费者的取数端。丢弃即退订。
#[derive(Debug)]
pub struct DaqStreamReader {
    rx: broadcast::Receiver<DaqSample>,
    counters: Arc<Counters>,
}

impl DaqStreamReader {
    /// 取一条样本。
    ///
    /// 落后超过缓冲容量时跳过被覆盖的最旧样本、记入丢帧，然后返回最新可取的
    /// 一条——跳过是静默的，但计数不静默。采集侧已全部关闭时返回 `None`。
    pub async fn recv(&mut self) -> Option<DaqSample> {
        loop {
            match self.rx.recv().await {
                Ok(sample) => {
                    self.counters.delivered.fetch_add(1, Ordering::Relaxed);
                    return Some(sample);
                }
                Err(broadcast::error::RecvError::Lagged(skipped)) => {
                    self.counters.dropped.fetch_add(skipped, Ordering::Relaxed);
                }
                Err(broadcast::error::RecvError::Closed) => return None,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn a_pushed_sample_comes_back_unchanged() {
        let stream = DaqStream::new(8);
        let mut reader = stream.subscribe();
        assert!(stream.push(3, vec![0xAA, 0xBB]), "有订阅者");
        let sample = reader.recv().await.expect("刚推过");
        assert_eq!(sample.pid, 3);
        assert_eq!(sample.payload, vec![0xAA, 0xBB]);
        assert_eq!(stream.stats().delivered, 1);
    }

    #[tokio::test]
    async fn a_slow_consumer_keeps_the_newest_and_counts_the_skipped() {
        let stream = DaqStream::new(4);
        let mut reader = stream.subscribe();
        for n in 0..10u8 {
            stream.push(0, vec![n]);
        }
        // 容量 4：前 6 条被挤出，留下第 6..10 条
        let mut seen = Vec::new();
        for _ in 0..4 {
            seen.push(reader.recv().await.expect("缓冲里还有").payload[0]);
        }
        assert_eq!(seen, vec![6, 7, 8, 9]);
        let stats = stream.stats();
        assert_eq!(stats.dropped_slow_consumer, 6);
        assert_eq!(stats.delivered, 4);
        assert_eq!(stats.produced, 10);
    }

    #[test]
    fn pushing_without_consumers_neither_blocks_nor_counts_as_drop() {
        let stream = DaqStream::new(4);
        for n in 0..1000u32 {
            assert!(!stream.push(0, vec![n as u8]), "没人订阅");
        }
        let stats = stream.stats();
        assert_eq!(stats.produced, 1000);
        assert_eq!(stats.delivered, 0);
        assert_eq!(stats.dropped_slow_consumer, 0, "没人订阅不算慢消费者");
    }

    #[tokio::test]
    async fn every_consumer_sees_the_same_sample() {
        let stream = DaqStream::new(8);
        let mut first = stream.subscribe();
        let mut second = stream.subscribe();
        stream.push(5, vec![0x11]);
        let a = first.recv().await.expect("有数据");
        let b = second.recv().await.expect("有数据");
        assert_eq!(a, b);
        assert_eq!(stream.stats().delivered, 2, "一份样本按消费者各计一次");
    }

    #[tokio::test]
    async fn dropping_the_stream_closes_readers() {
        let stream = DaqStream::new(4);
        let mut reader = stream.subscribe();
        drop(stream);
        assert!(reader.recv().await.is_none(), "采集侧没了就该结束");
    }

    #[tokio::test]
    async fn timestamps_are_relative_to_the_stream_start() {
        let stream = DaqStream::new(4);
        let mut reader = stream.subscribe();
        stream.push_at(1, Instant::now() + Duration::from_millis(25), vec![7]);
        let sample = reader.recv().await.expect("有数据");
        assert!(sample.timestamp_us >= 25_000, "{}", sample.timestamp_us);
    }

    #[tokio::test]
    async fn backlog_shows_how_far_a_consumer_fell_behind() {
        let stream = DaqStream::new(8);
        let _reader = stream.subscribe();
        for n in 0..3u8 {
            stream.push(n, vec![n]);
        }
        assert_eq!(stream.stats().backlog, 3);
    }

    #[tokio::test]
    async fn zero_capacity_falls_back_to_the_default() {
        let stream = DaqStream::new(0);
        let mut reader = stream.subscribe();
        for _ in 0..=DEFAULT_CAPACITY {
            stream.push(0, vec![0]);
        }
        let _ = reader.recv().await.expect("有数据");
        assert_eq!(stream.stats().dropped_slow_consumer, 1, "只挤出最早一条");
    }

    #[test]
    fn pushes_feed_the_diag_metrics_without_touching_the_bus_drop_count() {
        let metrics = Arc::new(Metrics::new());
        let stream = DaqStream::with_metrics(4, Arc::clone(&metrics));
        stream.push(0, vec![1, 2]);
        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.daq_frames_total, 1);
        assert_eq!(
            snapshot.dropped_frames_total, 0,
            "消费者丢帧不混进总线丢帧口径"
        );
    }
}
