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
//! 本阶段（M4）只做到「判据 + 窗口 + 导出 CSV」；接到录制器与 MF4 是 M5。

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::time::Duration;

use openape_cal_core::{ElemType, hex};
use openape_diag::{Domain, Fault, FaultCode, Severity};
use openape_xcp::DaqLayout;

use crate::daq_stream::{DaqSample, DaqStream, DaqStreamReader};
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

impl TriggerWindow {
    /// 导出为 CSV：一行一条样本，文件头写明判据与命中位置。
    ///
    /// 表头注释行以 `#` 开头，pandas/numpy 与 `grep -v '^#'` 都能直接吃。
    pub fn to_csv(&self, condition: &Condition) -> String {
        let mut out = String::new();
        out.push_str("# openape trigger window\n");
        out.push_str(&format!(
            "# condition: pid={} entry={} {} {}\n",
            condition.pid,
            condition.entry,
            condition.compare.label(),
            condition.threshold
        ));
        out.push_str(&format!("# hit_at_us: {}\n", self.hit_timestamp_us));
        out.push_str(&format!("# hit_index: {}\n", self.hit_index));
        out.push_str("timestamp_us,pid,payload_hex,hit\n");
        for (index, sample) in self.samples.iter().enumerate() {
            out.push_str(&format!(
                "{},{},{},{}\n",
                sample.timestamp_us,
                sample.pid,
                hex::encode(&sample.payload),
                if index == self.hit_index { 1 } else { 0 }
            ));
        }
        out
    }
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

/// 把窗口写到目录下，返回文件路径。
///
/// 文件名带命中时刻：同一目录里放多次触发的导出不会互相覆盖。
pub fn write_window(
    dir: &Path,
    window: &TriggerWindow,
    condition: &Condition,
) -> Result<PathBuf, Fault> {
    std::fs::create_dir_all(dir)
        .map_err(|err| config_fault(format!("触发窗口目录 {} 建不出来：{err}", dir.display())))?;
    let path = dir.join(format!("trigger-{}.csv", window.hit_timestamp_us));
    std::fs::write(&path, window.to_csv(condition))
        .map_err(|err| config_fault(format!("{} 写不进去：{err}", path.display())))?;
    Ok(path)
}

/// 挂在采集流上的触发任务。
#[derive(Debug)]
pub struct TriggerHandle {
    task: tokio::task::JoinHandle<Result<PathBuf, Fault>>,
}

impl TriggerHandle {
    /// 等窗口落盘。没命中时按超时返回故障——`Ok` 只代表真的写出了文件。
    pub async fn wait(self) -> Result<PathBuf, Fault> {
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

/// 在采集流上挂一个触发器：命中即导出窗口到 `dir`。
pub fn spawn_trigger(
    stream: &DaqStream,
    mut recorder: TriggerRecorder,
    dir: PathBuf,
    timeout: Duration,
) -> TriggerHandle {
    let mut reader: DaqStreamReader = stream.subscribe();
    let condition = recorder.config.condition.clone();
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
            if let Some(window) = recorder.push(&sample) {
                return write_window(&dir, &window, &condition);
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
    fn csv_carries_the_condition_and_marks_the_hit() {
        let mut recorder = TriggerRecorder::new(
            layout(),
            TriggerConfig {
                condition: condition(),
                pre_samples: 1,
                post_samples: 0,
            },
        )
        .expect("配置合法");
        let _ = recorder.push(&sample(0, 10));
        let window = recorder.push(&sample(1, 99)).expect("命中即导出");
        let dir = tempdir();
        let path = write_window(&dir, &window, &condition()).expect("能落盘");
        let text = std::fs::read_to_string(&path).expect("能读");
        assert!(text.contains("# condition: pid=0 entry=0 > 50"), "{text}");
        assert!(text.contains("timestamp_us,pid,payload_hex,hit"), "{text}");
        assert!(text.contains(",0,0063,1"), "命中行标 1: {text}");
        assert!(text.contains(",0,000a,0"), "命中前的样本标 0: {text}");
        assert_eq!(
            path.file_name().and_then(|name| name.to_str()),
            Some("trigger-1000.csv"),
            "文件名带命中时刻"
        );
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
        let handle = spawn_trigger(&stream, recorder(), dir.clone(), Duration::from_millis(50));
        stream.push(0, 10u16.to_be_bytes().to_vec());
        let err = handle.wait().await.expect_err("没命中就该报超时");
        assert!(err.message().contains("未落盘"), "{err}");
        assert_eq!(std::fs::read_dir(&dir).expect("目录可读").count(), 0);
    }

    #[tokio::test]
    async fn a_live_stream_hit_lands_on_disk() {
        let stream = DaqStream::new(8);
        let dir = tempdir();
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
            dir.clone(),
            Duration::from_secs(2),
        );
        stream.push(0, 10u16.to_be_bytes().to_vec());
        stream.push(0, 99u16.to_be_bytes().to_vec());
        let path = handle.wait().await.expect("命中即导出");
        assert!(path.exists());
        let text = std::fs::read_to_string(path).expect("能读");
        assert!(text.contains(",0,0063,1"), "{text}");
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
