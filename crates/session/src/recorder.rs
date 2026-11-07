//! 常时录制：样本流 → MF4 + CSV 双落盘，慢消费者丢自己的份并记账。
//!
//! 与触发录制（[crate::trigger]）互补：触发只留命中窗口，这里负责"整场
//! 都留底"。两条硬口径：
//!
//! 1. **录制不回压采集**。写盘走有界队列，队列满丢的是录制自己的份，
//!    计数公开——丢帧不可怕，不可见才可怕。这与 DAQ 环形缓冲（丢最旧、
//!    只保"现在的值"）口径不同：录制要的是"落盘样本数 = 收到数 − 丢的数"
//!    这条恒等式，丢最旧会破坏它。
//! 2. **两份文件讲同一套数**。MF4 存通道数值（离线重开读这个），CSV 是
//!    同一批行的文本视图（肉眼与 pandas 直接看），行数必须相等，收尾时
//!    由 [`RecorderStats`] 证明。
//!
//! 按**单 ODT** 设计：所有通道必须属于同一个 pid。多 ODT 的行是分批到达
//! 的，拼不出完整记录行——那是要等采集链路支持多 ODT 之后（M5-09/12 之
//! 后）另做的口径，配置不一致在 [`Recorder::start`] 就拒绝，不留到录制
//! 一半才爆。

use openape_cal_core::{ByteOrder, ElemType, value};
use openape_diag::{Domain, Fault, FaultCode, Severity};
use openape_format_mf4::Writer as Mf4Writer;
use openape_xcp::DaqLayout;
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc as std_mpsc;

use crate::daq_stream::DaqSample;
use crate::seq;

/// 队列缺省容量（行数）。20 帧/秒下约 50 秒的余量，与 DAQ 缓冲同量级。
pub const DEFAULT_QUEUE_CAPACITY: usize = 1024;

/// 一条要录的通道：落在哪个 ODT 的第几个条目上，解值用哪个类型与字节序。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecorderChannel {
    /// ODT 编号。
    pub pid: u8,
    /// 该 ODT 内的条目下标（顺序与 DAQ 配置一致）。
    pub entry: usize,
    /// 通道名，MF4 与 CSV 共用。
    pub name: String,
    /// 条目元素类型。
    pub elem: ElemType,
    /// 字节序。
    pub order: ByteOrder,
}

impl RecorderChannel {
    /// 一条通道定义。
    pub fn new(pid: u8, entry: usize, name: &str, elem: ElemType, order: ByteOrder) -> Self {
        Self {
            pid,
            entry,
            name: name.to_string(),
            elem,
            order,
        }
    }
}

/// 录制配置。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecorderConfig {
    /// 要录的通道，行内值按这里定义的顺序排列。
    pub channels: Vec<RecorderChannel>,
    /// MF4 输出路径。
    pub mf4: PathBuf,
    /// CSV 输出路径。
    pub csv: PathBuf,
    /// 写出队列容量（行数），0 取 [`DEFAULT_QUEUE_CAPACITY`]。
    pub queue_capacity: usize,
    /// 附加注释行，每行带 `# ` 前缀写进 CSV 头。触发窗口借这里带上判据
    /// 与命中位置；常时录制留空。MF4 无对应落点（writer 无注释路径），
    /// 元数据以 CSV 为准。
    pub notes: Vec<String>,
}

/// 一次录制的统计。`written + dropped_backpressure == received` 恒成立。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RecorderStats {
    /// 解出行并交给队列的样本数。
    pub received: u64,
    /// 已写进两份文件的行数。
    pub written: u64,
    /// 因队列满（或写出任务已死）丢掉的行数。
    pub dropped_backpressure: u64,
    /// 解不出值（pid/条目对不上、长度不够）的样本数。
    pub decode_errors: u64,
}

/// 队列里的一行。
struct RecordRow {
    timestamp_us: u64,
    values: Vec<f64>,
}

/// 计数块。写出线程与喂入侧共享，读者只拿得到 [`RecorderStats`]。
#[derive(Debug, Default)]
struct Counters {
    received: AtomicU64,
    written: AtomicU64,
    dropped: AtomicU64,
    decode_errors: AtomicU64,
}

impl Counters {
    fn snapshot(&self) -> RecorderStats {
        RecorderStats {
            received: self.received.load(Ordering::Relaxed),
            written: self.written.load(Ordering::Relaxed),
            dropped_backpressure: self.dropped.load(Ordering::Relaxed),
            decode_errors: self.decode_errors.load(Ordering::Relaxed),
        }
    }
}

/// 一次常时录制。
///
/// 用法：[`Recorder::start`] → 循环 [`Recorder::feed_sample`] →
/// [`Recorder::finish`] 等落盘收尾。收尾之后句柄就没了，写盘线程随之
/// 结束，不存在半开的文件。
///
/// 写出在独立 OS 线程里做：`mf4-rs` 的写出器不是 `Send`（内部是
/// `Write + Seek` 的 trait 对象），文件 IO 又是阻塞的——按线程设计最
/// 短的路。喂入侧的 [`Recorder::feed_sample`] 只做解值与 `try_send`，
/// 在 async 上下文里也不会阻塞采集泵；[`Recorder::finish`] 的 join 是
/// 阻塞的，daemon 侧接入时（M5-17）包一层 `spawn_blocking` 即可。
pub struct Recorder {
    tx: std_mpsc::SyncSender<RecordRow>,
    channels: Arc<[RecorderChannel]>,
    counters: Arc<Counters>,
    handle: Option<std::thread::JoinHandle<Result<(), Fault>>>,
}

impl std::fmt::Debug for Recorder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Recorder")
            .field("stats", &self.stats())
            .finish_non_exhaustive()
    }
}

fn recorder_fault(message: String) -> Fault {
    let code = FaultCode::new(Domain::Session, seq::RECORDER).expect("序号常量在 1..=999 内");
    Fault::new(code, Severity::Error, message)
}

impl Recorder {
    /// 起一次录制：建两份文件（CSV 先写表头），spawn 写出任务。
    ///
    /// 通道的 pid 不一致、条目越界这类配置错误在这里就报——录制跑到一半
    /// 才发现配置不对，是最坏的时间点。
    pub fn start(config: RecorderConfig, layout: &DaqLayout) -> Result<Self, Fault> {
        if config.channels.is_empty() {
            return Err(recorder_fault("没有要录的通道".to_string()));
        }
        let pid = config.channels[0].pid;
        if let Some(offender) = config.channels.iter().find(|channel| channel.pid != pid) {
            return Err(recorder_fault(format!(
                "录制按单 ODT 设计：通道 {} 属于 pid {}，其余属于 pid {}",
                offender.name, offender.pid, pid
            )));
        }
        let signal_count = layout.signal_count(pid).unwrap_or(0);
        for channel in &config.channels {
            if channel.entry >= signal_count {
                return Err(recorder_fault(format!(
                    "通道 {} 的条目 {} 越界：pid {} 只有 {signal_count} 个条目",
                    channel.name, channel.entry, pid
                )));
            }
        }

        let capacity = if config.queue_capacity == 0 {
            DEFAULT_QUEUE_CAPACITY
        } else {
            config.queue_capacity
        };

        // MF4 的写出器不能跨线程 move（内部是 Write+Seek 的 trait 对象），
        // 真文件由写出线程建；这里只做"路径此刻可建"的预检，让配置错误在
        // 起录制时报，而不是等收尾才发现。预检留下一个空文件，写出线程
        // 随即重建——中途崩溃的残件本来就该按残件处理。
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&config.mf4)
            .map_err(|err| recorder_fault(format!("{} 建不出来：{err}", config.mf4.display())))?;

        // CSV 在本线程建好、写完表头再交给写出线程：BufWriter<File> 可跨
        // 线程 move，表头错误也能同步报。
        let csv_file = std::fs::File::create(&config.csv)
            .map_err(|err| recorder_fault(format!("{} 建不出来：{err}", config.csv.display())))?;
        let mut csv = BufWriter::new(csv_file);
        write_csv_header(&mut csv, &config.channels, &config.notes)
            .map_err(|err| recorder_fault(format!("{} 写表头失败：{err}", config.csv.display())))?;

        let (tx, rx) = std_mpsc::sync_channel(capacity);
        let counters = Arc::new(Counters::default());
        let loop_counters = Arc::clone(&counters);
        let specs = channels_to_specs(&config.channels);
        let mf4_path = config.mf4.clone();
        let csv_path = config.csv.clone();
        let handle =
            std::thread::spawn(move || run(rx, mf4_path, specs, csv, csv_path, loop_counters));
        Ok(Self {
            tx,
            channels: config.channels.into(),
            counters,
            handle: Some(handle),
        })
    }

    /// 喂一条样本。解值在调用侧完成：解不出的记 [`RecorderStats::decode_errors`],
    /// 不阻塞、不报错——采集与录制是两条独立的退路，坏的样本不该拖垮采集。
    pub fn feed_sample(&self, layout: &DaqLayout, sample: &DaqSample) {
        let Some(values) = decode_row(&self.channels, layout, sample) else {
            self.counters.decode_errors.fetch_add(1, Ordering::Relaxed);
            return;
        };
        self.counters.received.fetch_add(1, Ordering::Relaxed);
        let row = RecordRow {
            timestamp_us: sample.timestamp_us,
            values,
        };
        // 队列满或写出任务已死都算丢：前者是慢盘，后者会在 finish 时以
        // 故障暴露——两种情形这批数都到不了盘上，计数不掩盖。
        if self.tx.try_send(row).is_err() {
            self.counters.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// 取一份统计快照。
    pub fn stats(&self) -> RecorderStats {
        self.counters.snapshot()
    }

    /// 收尾：关队列、等写出线程把两份文件落干净，返回最终统计。
    ///
    /// join 是阻塞的，见类型文档：async 上下文请包 `spawn_blocking`。
    pub fn finish(mut self) -> Result<RecorderStats, Fault> {
        drop(self.tx);
        let Some(handle) = self.handle.take() else {
            return Ok(self.counters.snapshot());
        };
        handle
            .join()
            .map_err(|_| recorder_fault("录制线程已崩溃，无错误信息可取".to_string()))??;
        Ok(self.counters.snapshot())
    }
}

/// 写出线程：先建 MF4（本线程独占），再从队列取行写两份文件。
fn run(
    rx: std_mpsc::Receiver<RecordRow>,
    mf4_path: PathBuf,
    specs: Vec<openape_format_mf4::ChannelSpec>,
    mut csv: BufWriter<std::fs::File>,
    csv_path: PathBuf,
    counters: Arc<Counters>,
) -> Result<(), Fault> {
    let mut mf4 = Mf4Writer::create(&mf4_path, &specs)
        .map_err(|err| recorder_fault(format!("{} 建不出来：{err}", mf4_path.display())))?;
    for row in rx.iter() {
        let time_s = row.timestamp_us as f64 / 1e6;
        mf4.write_row(time_s, &row.values)
            .map_err(|err| recorder_fault(format!("MF4 写失败：{err}")))?;
        let mut line = row.timestamp_us.to_string();
        for value in &row.values {
            line.push(',');
            line.push_str(&format_value(*value));
        }
        line.push('\n');
        csv.write_all(line.as_bytes())
            .map_err(|err| recorder_fault(format!("{} 写失败：{err}", csv_path.display())))?;
        counters.written.fetch_add(1, Ordering::Relaxed);
    }
    mf4.finish()
        .map_err(|err| recorder_fault(format!("MF4 收尾失败：{err}")))?;
    csv.flush()
        .map_err(|err| recorder_fault(format!("{} 收尾失败：{err}", csv_path.display())))?;
    Ok(())
}

/// CSV 数值格式：整数不带小数点，浮点走最短表示。肉眼可读优先。
fn format_value(value: f64) -> String {
    if value.fract() == 0.0 && value.abs() < 1e15 {
        format!("{}", value as i64)
    } else {
        format!("{value}")
    }
}

fn write_csv_header(
    csv: &mut BufWriter<std::fs::File>,
    channels: &[RecorderChannel],
    notes: &[String],
) -> std::io::Result<()> {
    csv.write_all(b"# openape recording\n")?;
    let names: Vec<&str> = channels
        .iter()
        .map(|channel| channel.name.as_str())
        .collect();
    writeln!(csv, "# channels: {}", names.join(","))?;
    write!(csv, "timestamp_us")?;
    for name in &names {
        write!(csv, ",{name}")?;
    }
    writeln!(csv)?;
    for note in notes {
        writeln!(csv, "# {note}")?;
    }
    Ok(())
}

fn channels_to_specs(channels: &[RecorderChannel]) -> Vec<openape_format_mf4::ChannelSpec> {
    channels
        .iter()
        .map(|channel| openape_format_mf4::ChannelSpec::new(&channel.name))
        .collect()
}

fn decode_row(
    channels: &[RecorderChannel],
    layout: &DaqLayout,
    sample: &DaqSample,
) -> Option<Vec<f64>> {
    let entries = layout.decode(sample.pid, &sample.payload)?;
    channels
        .iter()
        .map(|channel| {
            let raw = entries.get(channel.entry)?;
            value::decode(raw, channel.elem, channel.order)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    use openape_cal_core::ByteLen;

    fn tmp(name: &str) -> PathBuf {
        // 幂等：同一个测试进程里重复取目录不重建，别把已落盘的文件清掉。
        let dir =
            std::env::temp_dir().join(format!("openape-recorder-{name}-{}", std::process::id()));
        if !dir.exists() {
            std::fs::create_dir_all(&dir).expect("临时目录必须可建");
        }
        dir
    }

    /// 单 ODT（pid 0）单条目 U16 小端：与 UI 演示信号同一布局。
    fn layout() -> DaqLayout {
        let mut layout = DaqLayout::default();
        layout.insert_odt(0, &[ByteLen::TWO]);
        layout
    }

    fn config(name: &str, capacity: usize) -> RecorderConfig {
        let dir = tmp(name);
        RecorderConfig {
            channels: vec![RecorderChannel::new(
                0,
                0,
                "speed",
                ElemType::U16,
                ByteOrder::Little,
            )],
            mf4: dir.join("rec.mf4"),
            csv: dir.join("rec.csv"),
            queue_capacity: capacity,
            notes: Vec::new(),
        }
    }

    fn push_sample(stream: &crate::daq_stream::DaqStream, us: u64, value: u16) {
        let payload = value.to_le_bytes().to_vec();
        let at = std::time::Instant::now() + std::time::Duration::from_micros(us);
        stream.push_at(0, at, payload);
    }

    #[tokio::test]
    async fn a_recording_lands_both_files_with_matching_counts() {
        let stream = crate::daq_stream::DaqStream::new(64);
        let mut reader = stream.subscribe();
        let layout = layout();
        let cfg = config("counts", 0);
        let recorder = Recorder::start(cfg.clone(), &layout).expect("录制必须能起");
        for i in 0..10u16 {
            push_sample(&stream, u64::from(i) * 50_000, i * 100);
        }
        drop(stream); // 同上：排空缓冲后 recv 返回 None
        // 走同一条消费通路：从 DAQ 流里收，再喂录制——与 daemon 的接线一致。
        let mut fed = 0;
        while let Some(sample) = reader.recv().await {
            recorder.feed_sample(&layout, &sample);
            fed += 1;
            if fed == 10 {
                break;
            }
        }
        assert_eq!(fed, 10, "推了 10 条就要收到 10 条");
        let stats = recorder.finish().expect("收尾必须成功");
        assert_eq!(stats.received, 10);
        assert_eq!(stats.written, 10, "两份文件都要有全部行");
        assert_eq!(stats.dropped_backpressure, 0);
        assert_eq!(stats.decode_errors, 0);

        let csv = std::fs::read_to_string(&cfg.csv).expect("CSV 必须落盘");
        let data_lines: Vec<_> = csv
            .lines()
            .skip_while(|line| line.starts_with('#'))
            .skip(1)
            .collect();
        assert_eq!(data_lines.len(), 10, "CSV 数据行数要等于写出数");
        assert!(
            csv.lines()
                .next()
                .is_some_and(|head| head.starts_with("# openape")),
            "注释头要表明这是 openape 的录制文件"
        );

        let mdf = mf4_rs::api::mdf::MDF::from_file(cfg.mf4.to_str().expect("路径是 UTF-8"))
            .expect("MF4 必须能被读回");
        let values = mdf
            .channel("speed")
            .expect("通道按名可查")
            .values_as_f64()
            .expect("取值必须可读");
        let expected: Vec<f64> = (0..10u16).map(|i| f64::from(i * 100)).collect();
        assert_eq!(values, expected, "MF4 里的值要逐条等于喂入的值");
    }

    #[tokio::test]
    async fn backpressure_drops_are_counted_not_hidden() {
        // current_thread 运行时：喂入循环不让出，写出线程来不及消费，
        // 队列必满——丢帧进计数，恒等式守住。
        let stream = crate::daq_stream::DaqStream::new(256);
        let mut reader = stream.subscribe();
        let layout = layout();
        let recorder = Recorder::start(config("backpressure", 4), &layout).expect("录制必须能起");
        for i in 0..200u16 {
            push_sample(&stream, u64::from(i) * 50_000, i);
        }
        drop(stream); // 关采集侧：recv 把缓冲排空后返回 None，循环才有终点
        while let Some(sample) = reader.recv().await {
            recorder.feed_sample(&layout, &sample);
        }
        let stats = recorder.finish().expect("收尾必须成功");
        assert_eq!(
            stats.written + stats.dropped_backpressure,
            stats.received,
            "落盘 + 丢帧 必须等于收到"
        );
        assert!(
            stats.dropped_backpressure > 0,
            "容量 4 喂 200 条，必然有背压丢帧：{stats:?}"
        );
    }

    #[test]
    fn undecodable_samples_are_counted_never_written() {
        let layout = layout();
        let recorder = Recorder::start(config("undecodable", 0), &layout).expect("录制必须能起");
        recorder.feed_sample(
            &layout,
            &DaqSample {
                pid: 9,
                timestamp_us: 1,
                payload: vec![0xAA],
            },
        );
        recorder.feed_sample(
            &layout,
            &DaqSample {
                pid: 0,
                timestamp_us: 2,
                payload: vec![0xAA],
            },
        );
        let stats = recorder.stats();
        assert_eq!(stats.decode_errors, 2, "pid 对不上与长度不够都算解值失败");
        assert_eq!(stats.received, 0, "解不出的行不进队列");
        let stats = recorder.finish().expect("收尾必须成功");
        assert_eq!(stats.written, 0);
    }

    #[test]
    fn inconsistent_pids_are_rejected_before_any_file_is_created() {
        let layout = layout();
        let config = RecorderConfig {
            channels: vec![
                RecorderChannel::new(0, 0, "a", ElemType::U16, ByteOrder::Little),
                RecorderChannel::new(1, 0, "b", ElemType::U16, ByteOrder::Little),
            ],
            ..config("mixed", 0)
        };
        let err = Recorder::start(config, &layout).expect_err("跨 ODT 配置必须在起时就拒绝");
        assert!(err.to_string().contains("单 ODT"), "{err}");
    }

    #[test]
    fn empty_channel_list_is_rejected() {
        let layout = layout();
        let config = RecorderConfig {
            channels: Vec::new(),
            ..config("empty", 0)
        };
        let err = Recorder::start(config, &layout).expect_err("空通道必须拒绝");
        assert!(err.to_string().contains("通道"), "{err}");
    }
}
