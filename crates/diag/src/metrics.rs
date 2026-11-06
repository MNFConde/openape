//! 五项一等指标：DAQ 帧率、丢帧数、命令延迟分位、背压排队深度、进程内存。
//!
//! 指标与日志共用 span，但不共用输出通道（见 [11-architecture-layers] 的可
//! 观测层节）。这里只负责记录与快照，输出由外壳决定。
//!
//! 帧率用最近几秒的秒桶算，而不是"总帧数 ÷ 总时长"——后者是历史平均，采集
//! 已经卡死了它也显示正常。
//!
//! [11-architecture-layers]: ../../docs/11-architecture-layers.md

use std::collections::VecDeque;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Serialize;

/// 帧率窗口的秒桶数量。5 秒对"现在卡不卡"足够灵敏，又不至于抖成一个数。
const FPS_BUCKETS: usize = 5;

/// 延迟样本的保留条数。分位数只需要分布形状，1K 条足够稳定。
const LATENCY_SAMPLES: usize = 1024;

/// 指标收集器。所有记录方法都是 `&self`，可在多个任务间共享。
#[derive(Debug)]
pub struct Metrics {
    daq_frames: AtomicU64,
    dropped_frames: AtomicU64,
    queue_depth: AtomicU64,
    fps: Mutex<FpsWindow>,
    latency_us: Mutex<VecDeque<u32>>,
}

impl Default for Metrics {
    fn default() -> Self {
        Self::new()
    }
}

impl Metrics {
    /// 新建收集器。
    pub fn new() -> Self {
        Self {
            daq_frames: AtomicU64::new(0),
            dropped_frames: AtomicU64::new(0),
            queue_depth: AtomicU64::new(0),
            fps: Mutex::new(FpsWindow::default()),
            latency_us: Mutex::new(VecDeque::with_capacity(LATENCY_SAMPLES)),
        }
    }

    /// 记入 `count` 个刚到的 DAQ 帧。
    pub fn record_daq_frames(&self, count: u64) {
        if count == 0 {
            return;
        }
        self.daq_frames.fetch_add(count, Ordering::Relaxed);
        let mut window = self.fps.lock().unwrap_or_else(|poison| poison.into_inner());
        window.add(count, unix_secs());
    }

    /// 记入 `count` 个丢掉的 DAQ 帧。丢帧是最先崩的地方，单独计。
    pub fn record_dropped_frames(&self, count: u64) {
        if count > 0 {
            self.dropped_frames.fetch_add(count, Ordering::Relaxed);
        }
    }

    /// 记一次命令往返延迟。
    pub fn record_command_latency(&self, elapsed: Duration) {
        let micros = u32::try_from(elapsed.as_micros()).unwrap_or(u32::MAX);
        let mut samples = self
            .latency_us
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        if samples.len() == LATENCY_SAMPLES {
            samples.pop_front();
        }
        samples.push_back(micros);
    }

    /// 更新背压排队深度（当前值，不是累计）。
    pub fn set_queue_depth(&self, depth: u64) {
        self.queue_depth.store(depth, Ordering::Relaxed);
    }

    /// 取一份快照。
    pub fn snapshot(&self) -> Snapshot {
        let now = unix_secs();
        let fps = {
            let window = self.fps.lock().unwrap_or_else(|poison| poison.into_inner());
            window.rate(now)
        };
        let (p50, p99, samples) = {
            let guard = self
                .latency_us
                .lock()
                .unwrap_or_else(|poison| poison.into_inner());
            let mut sorted: Vec<u32> = guard.iter().copied().collect();
            sorted.sort_unstable();
            (
                percentile(&sorted, 50),
                percentile(&sorted, 99),
                sorted.len() as u64,
            )
        };
        Snapshot {
            daq_fps: fps,
            daq_frames_total: self.daq_frames.load(Ordering::Relaxed),
            dropped_frames_total: self.dropped_frames.load(Ordering::Relaxed),
            latency_p50_us: p50,
            latency_p99_us: p99,
            latency_samples: samples,
            queue_depth: self.queue_depth.load(Ordering::Relaxed),
            rss_bytes: rust_resident_bytes(),
        }
    }
}

/// 一份指标快照，可直接序列化给 UI 或落盘。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Snapshot {
    /// 最近窗口的 DAQ 帧率。
    pub daq_fps: f64,
    /// 累计 DAQ 帧数。
    pub daq_frames_total: u64,
    /// 累计丢帧数。
    pub dropped_frames_total: u64,
    /// 命令延迟中位数（微秒）。没有样本时为 0。
    pub latency_p50_us: u32,
    /// 命令延迟 99 分位（微秒）。没有样本时为 0。
    pub latency_p99_us: u32,
    /// 参与分位的样本数。
    pub latency_samples: u64,
    /// 背压排队深度。
    pub queue_depth: u64,
    /// 常驻内存字节数；取不到时为 0（非 Linux）。
    pub rss_bytes: u64,
}

#[derive(Debug)]
struct FpsWindow {
    buckets: VecDeque<(u64, u64)>,
}

impl Default for FpsWindow {
    fn default() -> Self {
        Self {
            buckets: VecDeque::with_capacity(FPS_BUCKETS),
        }
    }
}

impl FpsWindow {
    fn add(&mut self, count: u64, second: u64) {
        match self.buckets.back_mut() {
            Some((bucket_second, frames)) if *bucket_second == second => *frames += count,
            _ => {
                if self.buckets.len() == FPS_BUCKETS {
                    self.buckets.pop_front();
                }
                self.buckets.push_back((second, count));
            }
        }
    }

    /// 最近窗口的帧率。窗口跨度取实际桶跨度，避免刚开始采集时把帧率算高。
    fn rate(&self, now: u64) -> f64 {
        let fresh: u64 = self
            .buckets
            .iter()
            .filter(|(second, _)| now.saturating_sub(*second) < FPS_BUCKETS as u64)
            .map(|(_, frames)| *frames)
            .sum();
        let oldest = self
            .buckets
            .front()
            .map(|(second, _)| *second)
            .unwrap_or(now);
        let span = now.saturating_sub(oldest).max(1);
        fresh as f64 / span as f64
    }
}

/// 分位数（最近秩法）。空样本返回 0。
fn percentile(sorted: &[u32], percent: usize) -> u32 {
    if sorted.is_empty() {
        return 0;
    }
    let rank = (sorted.len() * percent).div_ceil(100).max(1);
    sorted[rank.min(sorted.len()) - 1]
}

fn unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
}

/// 进程常驻内存。Linux 读 `/proc/self/status` 的 `VmRSS`，其它平台返回 0。
#[cfg(target_os = "linux")]
fn rust_resident_bytes() -> u64 {
    let Ok(status) = std::fs::read_to_string("/proc/self/status") else {
        return 0;
    };
    status
        .lines()
        .find_map(|line| line.strip_prefix("VmRSS:"))
        .and_then(|rest| rest.split_whitespace().next())
        .and_then(|kilobytes| kilobytes.parse::<u64>().ok())
        .map(|kilobytes| kilobytes * 1024)
        .unwrap_or(0)
}

#[cfg(not(target_os = "linux"))]
fn rust_resident_bytes() -> u64 {
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_accumulate_and_rate_is_positive() {
        let metrics = Metrics::new();
        metrics.record_daq_frames(0);
        metrics.record_daq_frames(100);
        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.daq_frames_total, 100);
        assert!(
            snapshot.daq_fps >= 100.0,
            "首秒窗口内帧率不该低于帧数：{snapshot:?}"
        );
    }

    #[test]
    fn dropped_frames_are_counted_separately() {
        let metrics = Metrics::new();
        metrics.record_dropped_frames(0);
        metrics.record_dropped_frames(7);
        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.dropped_frames_total, 7);
        assert_eq!(snapshot.daq_frames_total, 0, "丢帧不是收帧");
    }

    #[test]
    fn latency_percentiles_track_samples() {
        let metrics = Metrics::new();
        for millis in 1..=100u64 {
            metrics.record_command_latency(Duration::from_micros(millis * 10));
        }
        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.latency_samples, 100);
        assert_eq!(snapshot.latency_p50_us, 500, "中位数取第 50 个样本");
        assert!(snapshot.latency_p99_us >= 990, "{snapshot:?}");
    }

    #[test]
    fn empty_latency_reports_zero() {
        let snapshot = Metrics::new().snapshot();
        assert_eq!(snapshot.latency_samples, 0);
        assert_eq!((snapshot.latency_p50_us, snapshot.latency_p99_us), (0, 0));
    }

    #[test]
    fn latency_samples_are_bounded() {
        let metrics = Metrics::new();
        for _ in 0..(LATENCY_SAMPLES + 10) {
            metrics.record_command_latency(Duration::from_micros(1));
        }
        assert_eq!(metrics.snapshot().latency_samples, LATENCY_SAMPLES as u64);
    }

    #[test]
    fn queue_depth_is_a_gauge_not_a_counter() {
        let metrics = Metrics::new();
        metrics.set_queue_depth(5);
        metrics.set_queue_depth(2);
        assert_eq!(metrics.snapshot().queue_depth, 2);
    }

    #[test]
    fn snapshot_serializes_for_ui_and_disk() {
        let metrics = Metrics::new();
        metrics.record_daq_frames(10);
        let json = serde_json::to_value(metrics.snapshot()).expect("可序列化");
        for key in [
            "daq_fps",
            "dropped_frames_total",
            "latency_p99_us",
            "queue_depth",
            "rss_bytes",
        ] {
            assert!(json.get(key).is_some(), "缺指标 {key}");
        }
    }

    #[test]
    fn percentile_handles_boundaries() {
        assert_eq!(percentile(&[], 99), 0);
        assert_eq!(percentile(&[7], 99), 7);
        assert_eq!(percentile(&[1, 2, 3, 4], 50), 2);
        assert_eq!(percentile(&[1, 2, 3, 4], 100), 4);
    }
}
