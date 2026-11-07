//! 条件触发录制：判据 + 前后环形窗口，命中即导出窗口。
//!
//! 常时录制的问题不是存不下，而是存下来之后要在几十万行里找"那一瞬"。触发
//! 录制把这件事倒过来做：一直只留一小段环形缓冲，判据命中才把「命中前的缓冲
//! + 命中样本 + 命中后若干样本」落盘。没命中就一个字节都不写。
//!
//! 三条约定：
//!
//! 1. **命中前的缓冲必须算进窗口**。异常往往在判据成立之前就已经开始积累，
//!    只导命中之后的部分等于把起因删掉。
//! 2. **窗口按样本计，不按时间计**。按时间要猜采样周期，猜错就少一段。
//! 3. **一次采集只落一次盘**。反复命中会让文件无限长，重新起一轮比继续堆
//!    更符合"看那一瞬"的用法。
//!
//! 本阶段（M5-04）起，窗口导出走录制器的同一出口（[`TriggerWindow::export`]）：
//! 同一套通道解值、MF4+CSV 双落盘、同一时间轴基准，与常时录制逐行可比。

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::time::Duration;

use openape_cal_core::ElemType;
use openape_diag::{Domain, Fault, FaultCode, Severity};
use openape_xcp::DaqLayout;

use crate::daq_stream::{DaqSample, DaqStream, DaqStreamReader};
use crate::recorder::{Recorder, RecorderChannel, RecorderConfig, RecorderStats};
use crate::seq;

/// 比较方式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Compare {
    /// 大于阈值。
    Above,
    /// 小于阈值。
    Below,
    /// 等于阈值（浮点按相等比较，阈值应来自同一换算）。
    Equal,
}

impl Compare {
    /// 判据文本，写进导出文件头，便于事后核对"这批是怎么挑出来的"。
    pub const fn label(self) -> &'static str {
        match self {
            Self::Above => ">",
            Self::Below => "<",
            Self::Equal => "==",
        }
    }
}

/// 一条判据：某一路信号的原始值满足比较。
#[derive(Debug, Clone, PartialEq)]
pub struct Condition {
    /// 哪一路 ODT。
    pub pid: u8,
    /// 该 ODT 内的第几个条目（顺序与 DAQ 配置一致）。
    pub entry: usize,
    /// 条目元素类型。
    pub elem: ElemType,
    /// 字节序。
    pub order: openape_cal_core::ByteOrder,
    /// 比较方式。
    pub compare: Compare,
    /// 阈值（原始值，不做换算——判据越简单越不会错）。
    pub threshold: f64,
}

impl Condition {
    /// 对着一条样本求值。样本里没有这一路或长度不够时返回 `None`。
    pub fn test(&self, layout: &DaqLayout, sample: &DaqSample) -> Option<bool> {
        if sample.pid != self.pid {
            return None;
        }
        let entries = layout.decode(sample.pid, &sample.payload)?;
        let value =
            openape_cal_core::value::decode(entries.get(self.entry)?, self.elem, self.order)?;
        Some(match self.compare {
            Compare::Above => value > self.threshold,
            Compare::Below => value < self.threshold,
            Compare::Equal => value == self.threshold,
        })
    }
}

/// 触发配置。
#[derive(Debug, Clone, PartialEq)]
pub struct TriggerConfig {
    /// 判据。
    pub condition: Condition,
    /// 命中前保留多少条样本。
    pub pre_samples: usize,
    /// 命中后再收多少条样本。
    pub post_samples: usize,
}

/// 窗口里的一条样本。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowSample {
    /// 相对采集起点的微秒时间戳。
    pub timestamp_us: u64,
    /// ODT 编号。
    pub pid: u8,
    /// DTO 载荷。
    pub payload: Vec<u8>,
}

impl From<&DaqSample> for WindowSample {
    fn from(sample: &DaqSample) -> Self {
        Self {
            timestamp_us: sample.timestamp_us,
            pid: sample.pid,
            payload: sample.payload.clone(),
        }
    }
}

/// 一次命中导出的窗口。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TriggerWindow {
    /// 命中样本的时间戳。
    pub hit_timestamp_us: u64,
    /// 命中样本在 `samples` 里的下标。
    pub hit_index: usize,
    /// 窗口内样本，按时间顺序。
    pub samples: Vec<WindowSample>,
}

/// 触发录制器。喂样本，命中一次就吐出窗口。
#[derive(Debug)]
pub struct TriggerRecorder {
    layout: DaqLayout,
    config: TriggerConfig,
    pre: VecDeque<WindowSample>,
    post: Vec<WindowSample>,
    /// 命中那条样本。命中后置样本单独收，`post_samples` 也不把命中本身算进去。
    hit: Option<WindowSample>,
    fired: bool,
}

impl TriggerRecorder {
    /// 建录制器。命中前样本数为 0 时导出仍然可用，但要显式给 0——不留默认值，
    /// 因为"要不要命中前缓冲"是个取舍而不是参数细节。
    pub fn new(layout: DaqLayout, config: TriggerConfig) -> Result<Self, Fault> {
        if config.pre_samples == 0 {
            return Err(
                config_fault("命中前样本数为 0：窗口会缺少起因那一段".to_owned())
                    .with_hint("给一个正整数；异常往往在判据成立前就开始积累"),
            );
        }
        Ok(Self {
            layout,
            config,
            pre: VecDeque::new(),
            post: Vec::new(),
            hit: None,
            fired: false,
        })
    }

    /// 判据取值用的布局。挂在流上时借它去建窗口导出，不必另传一份。
    pub fn layout(&self) -> &DaqLayout {
        &self.layout
    }

    /// 喂一条样本。首次命中且命中后样本已收齐时返回窗口。
    pub fn push(&mut self, sample: &DaqSample) -> Option<TriggerWindow> {
        if self.fired {
            return None;
        }
        // 已在收后置样本：收满就导出。
        if let Some(hit) = self.hit.clone() {
            self.post.push(WindowSample::from(sample));
            if self.post.len() < self.config.post_samples {
                return None;
            }
            return Some(self.finish(&hit));
        }
        let hit_now = self
            .config
            .condition
            .test(&self.layout, sample)
            .unwrap_or(false);
        if hit_now {
            let hit = WindowSample::from(sample);
            if self.config.post_samples == 0 {
                return Some(self.finish(&hit));
            }
            self.hit = Some(hit);
            return None;
        }
        self.pre.push_back(WindowSample::from(sample));
        while self.pre.len() > self.config.pre_samples {
            self.pre.pop_front();
        }
        None
    }

    fn finish(&mut self, hit: &WindowSample) -> TriggerWindow {
        self.fired = true;
        let mut samples: Vec<WindowSample> = self.pre.drain(..).collect();
        let hit_index = samples.len();
        samples.push(hit.clone());
        samples.append(&mut self.post);
        TriggerWindow {
            hit_timestamp_us: hit.timestamp_us,
            hit_index,
            samples,
        }
    }
}

/// 一次命中经统一出口落盘的产物。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowExport {
    /// MF4 路径（离线重开读这份）。
    pub mf4: PathBuf,
    /// CSV 路径（同一批行的文本视图，注释头带判据与命中位置）。
    pub csv: PathBuf,
    /// 录制统计——行数守恒恒等式同样约束窗口导出。
    pub stats: RecorderStats,
}

impl TriggerWindow {
    /// 窗口经录制器的同一出口落盘：同一套通道解值、MF4+CSV 双落盘、
    /// 同一时间轴基准（样本的 `timestamp_us` 原样进两份文件，与常时
    /// 录制逐行可比）。判据与命中位置进 CSV 注释头；没命中就不会调
    /// 到这里，目录里零文件。
    ///
    /// 文件名带命中时刻，同目录多次触发互不覆盖。
    pub fn export(
        &self,
        dir: &Path,
        condition: &Condition,
        channels: &[RecorderChannel],
        layout: &DaqLayout,
    ) -> Result<WindowExport, Fault> {
        let mf4 = dir.join(format!("trigger-{}.mf4", self.hit_timestamp_us));
        let csv = dir.join(format!("trigger-{}.csv", self.hit_timestamp_us));
        std::fs::create_dir_all(dir).map_err(|err| {
            config_fault(format!("触发窗口目录 {} 建不出来：{err}", dir.display()))
        })?;
        let config = RecorderConfig {
            channels: channels.to_vec(),
            mf4: mf4.clone(),
            csv: csv.clone(),
            queue_capacity: 0,
            notes: vec![
                format!(
                    "condition: pid={} entry={} {} {}",
                    condition.pid,
                    condition.entry,
                    condition.compare.label(),
                    condition.threshold
                ),
                format!("hit_at_us: {}", self.hit_timestamp_us),
                format!("hit_index: {}", self.hit_index),
            ],
        };
        let recorder = Recorder::start(config, layout)?;
        for sample in &self.samples {
            recorder.feed_sample(
                layout,
                &DaqSample {
                    pid: sample.pid,
                    timestamp_us: sample.timestamp_us,
                    payload: sample.payload.clone(),
                },
            );
        }
        let stats = recorder.finish()?;
        Ok(WindowExport { mf4, csv, stats })
    }
}

/// 挂在采集流上的触发任务。
#[derive(Debug)]
pub struct TriggerHandle {
    task: tokio::task::JoinHandle<Result<WindowExport, Fault>>,
}

impl TriggerHandle {
    /// 等窗口落盘。没命中时按超时返回故障——`Ok` 只代表真的写出了两份文件。
    pub async fn wait(self) -> Result<WindowExport, Fault> {
        match self.task.await {
            Ok(result) => result,
            Err(err) => Err(config_fault(format!("触发任务被中断：{err}"))),
        }
    }

    /// 不等结果，直接停。
    pub fn abort(self) {
        self.task.abort();
    }
}

/// 在采集流上挂一个触发器：命中即经录制器出口把窗口落成 MF4+CSV。
pub fn spawn_trigger(
    stream: &DaqStream,
    mut trigger: TriggerRecorder,
    channels: Vec<RecorderChannel>,
    dir: PathBuf,
    timeout: Duration,
) -> TriggerHandle {
    let mut reader: DaqStreamReader = stream.subscribe();
    let condition = trigger.config.condition.clone();
    let layout = trigger.layout().clone();
    let task = tokio::spawn(async move {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                return Err(config_fault(format!(
                    "{} ms 内没有命中判据，未落盘",
                    timeout.as_millis()
                ))
                .with_hint("放宽判据阈值，或确认该信号确实进了采集"));
            }
            let sample = match tokio::time::timeout(remaining, reader.recv()).await {
                Ok(Some(sample)) => sample,
                Ok(None) => {
                    return Err(config_fault("采集流已结束，窗口没收齐，未落盘".to_owned())
                        .with_hint("命中后还要收满后置样本，别提前停采集"));
                }
                Err(_) => continue,
            };
            if let Some(window) = trigger.push(&sample) {
                return window.export(&dir, &condition, &channels, &layout);
            }
        }
    });
    TriggerHandle { task }
}

fn config_fault(message: String) -> Fault {
    let code = FaultCode::new(Domain::Session, seq::TRIGGER).expect("序号常量在 1..=999 内");
    Fault::new(code, Severity::Error, message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use openape_cal_core::{Address, ByteLen, ByteOrder};

    fn layout() -> DaqLayout {
        let mut layout = DaqLayout::default();
        layout.insert_odt(0, &[ByteLen::new(2).expect("非零")]);
        layout
    }

    fn condition() -> Condition {
        Condition {
            pid: 0,
            entry: 0,
            elem: ElemType::U16,
            order: ByteOrder::Big,
            compare: Compare::Above,
            threshold: 50.0,
        }
    }

    fn recorder() -> TriggerRecorder {
        TriggerRecorder::new(
            layout(),
            TriggerConfig {
                condition: condition(),
                pre_samples: 3,
                post_samples: 2,
            },
        )
        .expect("配置合法")
    }

    fn sample(n: u64, value: u16) -> DaqSample {
        DaqSample {
            pid: 0,
            timestamp_us: n * 1000,
            payload: value.to_be_bytes().to_vec(),
        }
    }

    #[test]
    fn a_hit_in_the_middle_keeps_the_buffer_before_it() {
        let mut recorder = recorder();
        for (index, value) in [10u16, 20, 30, 60, 70, 80].into_iter().enumerate() {
            let window = recorder.push(&sample(index as u64, value));
            if index < 5 {
                assert!(window.is_none(), "命中后还要收满 2 条才导出");
            } else {
                let window = window.expect("第 6 条时窗口收齐");
                assert_eq!(window.hit_timestamp_us, 3000);
                assert_eq!(window.hit_index, 3, "命中前有 3 条缓冲");
                let values: Vec<u16> = window
                    .samples
                    .iter()
                    .map(|sample| u16::from_be_bytes([sample.payload[0], sample.payload[1]]))
                    .collect();
                assert_eq!(values, vec![10, 20, 30, 60, 70, 80], "含命中前缓冲");
            }
        }
    }

    #[test]
    fn a_hit_at_the_start_exports_what_it_has() {
        let mut recorder = TriggerRecorder::new(
            layout(),
            TriggerConfig {
                condition: condition(),
                pre_samples: 3,
                post_samples: 1,
            },
        )
        .expect("配置合法");
        assert!(recorder.push(&sample(0, 99)).is_none());
        let window = recorder.push(&sample(1, 10)).expect("收满后置样本");
        assert_eq!(window.hit_index, 0, "命中之前没有样本");
        assert_eq!(window.samples.len(), 2);
    }

    #[test]
    fn only_the_first_hit_is_exported() {
        let mut recorder = TriggerRecorder::new(
            layout(),
            TriggerConfig {
                condition: condition(),
                pre_samples: 1,
                post_samples: 0,
            },
        )
        .expect("配置合法");
        assert!(recorder.push(&sample(0, 99)).is_some(), "首次命中即导出");
        assert!(
            recorder.push(&sample(1, 99)).is_none(),
            "一次采集只落一次盘"
        );
    }

    #[test]
    fn no_hit_means_no_window_and_no_file() {
        let dir = tempdir();
        let mut recorder = recorder();
        for (index, value) in [1u16, 2, 3, 4].into_iter().enumerate() {
            assert!(recorder.push(&sample(index as u64, value)).is_none());
        }
        assert_eq!(
            std::fs::read_dir(&dir).expect("目录可读").count(),
            0,
            "未命中不落盘"
        );
    }

    #[test]
    fn the_export_writes_decoded_values_through_the_recorder_exit() {
        let mut trigger_recorder = TriggerRecorder::new(
            layout(),
            TriggerConfig {
                condition: condition(),
                pre_samples: 1,
                post_samples: 0,
            },
        )
        .expect("配置合法");
        let _ = trigger_recorder.push(&sample(0, 10));
        let window = trigger_recorder.push(&sample(1, 99)).expect("命中即导出");
        let dir = tempdir();
        let channels = vec![RecorderChannel::new(
            0,
            0,
            "speed",
            ElemType::U16,
            ByteOrder::Big,
        )];
        let export = window
            .export(&dir, &condition(), &channels, trigger_recorder.layout())
            .expect("能落盘");
        assert_eq!(export.stats.received, 2, "窗口两条都解出值");
        assert_eq!(export.stats.written, 2);
        assert_eq!(export.stats.decode_errors, 0);
        assert_eq!(
            export.mf4.file_name().and_then(|name| name.to_str()),
            Some("trigger-1000.mf4"),
            "文件名带命中时刻"
        );
        let csv = std::fs::read_to_string(&export.csv).expect("能读");
        assert!(csv.contains("# condition: pid=0 entry=0 > 50"), "{csv}");
        assert!(csv.contains("# hit_at_us: 1000"), "{csv}");
        assert!(csv.contains("# hit_index: 1"), "{csv}");
        assert!(csv.contains("timestamp_us,speed"), "{csv}");
        assert!(csv.lines().any(|line| line == "1000,99"), "{csv}");

        let mdf = mf4_rs::api::mdf::MDF::from_file(export.mf4.to_str().expect("路径是 UTF-8"))
            .expect("MF4 必须能被读回");
        let values = mdf
            .channel("speed")
            .expect("通道按名可查")
            .values_as_f64()
            .expect("取值必须可读");
        assert_eq!(values, vec![10.0, 99.0], "窗口值按通道定义解出");
    }

    #[test]
    fn zero_pre_samples_is_refused_at_construction() {
        let err = TriggerRecorder::new(
            layout(),
            TriggerConfig {
                condition: condition(),
                pre_samples: 0,
                post_samples: 1,
            },
        )
        .expect_err("没有命中前缓冲的窗口缺少起因");
        assert!(err.hint().is_some());
    }

    #[test]
    fn samples_of_another_pid_are_kept_in_the_window_but_do_not_trigger() {
        let mut recorder = TriggerRecorder::new(
            layout(),
            TriggerConfig {
                condition: condition(),
                pre_samples: 2,
                post_samples: 0,
            },
        )
        .expect("配置合法");
        let other = DaqSample {
            pid: 7,
            timestamp_us: 500,
            payload: vec![0xFF, 0xFF],
        };
        assert!(recorder.push(&other).is_none(), "别的 ODT 不触发");
        let window = recorder.push(&sample(1, 99)).expect("命中即导出");
        assert_eq!(window.hit_index, 1, "别的 ODT 仍留在窗口里");
        assert_eq!(window.samples[0].pid, 7);
    }

    #[tokio::test]
    async fn a_live_stream_that_never_hits_writes_nothing() {
        let stream = DaqStream::new(8);
        let dir = tempdir();
        let channels = vec![RecorderChannel::new(
            0,
            0,
            "speed",
            ElemType::U16,
            ByteOrder::Big,
        )];
        let handle = spawn_trigger(
            &stream,
            recorder(),
            channels,
            dir.clone(),
            Duration::from_millis(50),
        );
        stream.push(0, 10u16.to_be_bytes().to_vec());
        let err = handle.wait().await.expect_err("没命中就该报超时");
        assert!(err.message().contains("未落盘"), "{err}");
        assert_eq!(
            std::fs::read_dir(&dir).expect("目录可读").count(),
            0,
            "未命中零落盘"
        );
    }

    #[tokio::test]
    async fn a_live_stream_hit_lands_on_disk() {
        let stream = DaqStream::new(8);
        let dir = tempdir();
        let channels = vec![RecorderChannel::new(
            0,
            0,
            "speed",
            ElemType::U16,
            ByteOrder::Big,
        )];
        let handle = spawn_trigger(
            &stream,
            TriggerRecorder::new(
                layout(),
                TriggerConfig {
                    condition: condition(),
                    pre_samples: 1,
                    post_samples: 0,
                },
            )
            .expect("配置合法"),
            channels,
            dir.clone(),
            Duration::from_secs(2),
        );
        stream.push(0, 10u16.to_be_bytes().to_vec());
        stream.push(0, 99u16.to_be_bytes().to_vec());
        let export = handle.wait().await.expect("命中即导出");
        assert!(export.mf4.exists(), "MF4 要落盘");
        assert!(export.csv.exists(), "CSV 要落盘");
        assert_eq!(export.stats.written, 2);
    }

    /// 验收线：命中窗口与常时录制的时间轴逐行可比——窗口的 4 行必须是
    /// 常时文件里按时间戳定位到的连续子序列，值也逐条相等。
    #[tokio::test]
    async fn the_window_time_axis_aligns_with_the_continuous_recording() {
        let stream = DaqStream::new(64);
        let mut reader = stream.subscribe();
        let layout = layout();
        let rec_dir = tempdir();
        let continuous = Recorder::start(
            RecorderConfig {
                channels: vec![RecorderChannel::new(
                    0,
                    0,
                    "speed",
                    ElemType::U16,
                    ByteOrder::Big,
                )],
                mf4: rec_dir.join("cont.mf4"),
                csv: rec_dir.join("cont.csv"),
                queue_capacity: 0,
                notes: Vec::new(),
            },
            &layout,
        )
        .expect("常时录制必须能起");
        let trig_dir = tempdir();
        let channels = vec![RecorderChannel::new(
            0,
            0,
            "speed",
            ElemType::U16,
            ByteOrder::Big,
        )];
        let handle = spawn_trigger(
            &stream,
            TriggerRecorder::new(
                layout.clone(),
                TriggerConfig {
                    condition: condition(),
                    pre_samples: 2,
                    post_samples: 1,
                },
            )
            .expect("配置合法"),
            channels,
            trig_dir.clone(),
            Duration::from_secs(2),
        );
        for value in [10u16, 20, 30, 99, 40, 50] {
            stream.push(0, value.to_be_bytes().to_vec());
        }
        drop(stream); // 排空缓冲后 recv 返回 None，两条消费路径都有终点
        let mut fed = 0;
        while let Some(sample) = reader.recv().await {
            continuous.feed_sample(&layout, &sample);
            fed += 1;
        }
        assert_eq!(fed, 6, "推 6 条两条消费路径都要见全");
        assert_eq!(
            continuous.finish().expect("收尾必须成功").written,
            6,
            "常时录制整场留底"
        );
        let export = handle.wait().await.expect("命中即导出");
        assert_eq!(export.stats.written, 4, "窗口 = 命中前 2 + 命中 + 命中后 1");

        let open = |path: &std::path::Path| {
            mf4_rs::api::mdf::MDF::from_file(path.to_str().expect("路径是 UTF-8"))
                .expect("MF4 必须能被读回")
        };
        let continuous_mdf = open(&rec_dir.join("cont.mf4"));
        let window_mdf = open(&export.mf4);
        let channel_values = |mdf: &mf4_rs::api::mdf::MDF, name: &str| {
            mdf.channel(name)
                .unwrap_or_else(|| panic!("通道 {name} 必须可查"))
                .values_as_f64()
                .expect("取值必须可读")
        };
        let cont_time = channel_values(&continuous_mdf, "time");
        let win_time = channel_values(&window_mdf, "time");
        let cont_speed = channel_values(&continuous_mdf, "speed");
        let win_speed = channel_values(&window_mdf, "speed");
        let pos = cont_time
            .windows(4)
            .position(|span| span == win_time.as_slice())
            .expect("窗口时间戳必须是常时录制的连续子序列");
        assert_eq!(
            &cont_speed[pos..pos + 4],
            win_speed.as_slice(),
            "定位到的时间段里值也要逐条相等"
        );
    }

    fn tempdir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "openape-trigger-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("建临时目录");
        dir
    }

    /// 判据定义里用到的地址只是为了可读性：触发判据按 ODT 条目取，不按地址。
    #[allow(dead_code)]
    fn address() -> Address {
        Address::new(0x1000)
    }
}
