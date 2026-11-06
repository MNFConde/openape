//! 黑匣子：常驻环形缓冲，故障触发时把最近一段窗口落盘。
//!
//! 针对现场偶发问题——ECU 侧现象事后难以复现，故障瞬间把前一段时间的帧与
//! 命令序列一并留下，比事后追加日志开关有效得多。
//!
//! 容量按条数而不是按时间：DAQ 帧率基本恒定，条数就是时间；按时间做反而
//! 需要一个后台定时器，多一个会漏数据的部件。

use std::collections::VecDeque;
use std::io::Write;
use std::path::Path;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;

use crate::fault::Fault;
use crate::log::Direction;

/// 一条黑匣子记录。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Entry {
    /// 自缓冲建立起的序号，用来发现空洞。
    seq: u64,
    /// 墙上时间（Unix 毫秒）。
    at_ms: u64,
    #[serde(flatten)]
    body: EntryBody,
}

/// 记录内容。
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EntryBody {
    /// 一帧收发。
    Frame {
        /// `tx` 或 `rx`。
        dir: &'static str,
        /// 通道名。
        channel: String,
        /// CAN 标识符。
        id: u32,
        /// 载荷，十六进制。
        data: String,
    },
    /// 一条命令或状态迁移。
    Note {
        /// 事由，如 `xcp_connect`。
        name: String,
        /// 细节。
        detail: String,
    },
}

/// 环形缓冲。多线程共享一个实例，写入是短临界区。
#[derive(Debug)]
pub struct BlackBox {
    capacity: usize,
    inner: Mutex<Inner>,
}

#[derive(Debug)]
struct Inner {
    entries: VecDeque<Entry>,
    next_seq: u64,
}

impl BlackBox {
    /// 建一个最多保留 `capacity` 条记录的缓冲。`capacity` 为 0 按 1 处理。
    pub fn new(capacity: usize) -> Self {
        let capacity = capacity.max(1);
        Self {
            capacity,
            inner: Mutex::new(Inner {
                entries: VecDeque::with_capacity(capacity),
                next_seq: 0,
            }),
        }
    }

    /// 容量。
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// 记一帧。
    pub fn record_frame(&self, direction: Direction, channel: &str, id: u32, data: &[u8]) {
        self.push(EntryBody::Frame {
            dir: match direction {
                Direction::Tx => "tx",
                Direction::Rx => "rx",
            },
            channel: channel.to_owned(),
            id,
            data: hex(data),
        });
    }

    /// 记一条命令或状态迁移。
    pub fn record_note(&self, name: &str, detail: impl Into<String>) {
        self.push(EntryBody::Note {
            name: name.to_owned(),
            detail: detail.into(),
        });
    }

    fn push(&self, body: EntryBody) {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let seq = inner.next_seq;
        inner.next_seq += 1;
        if inner.entries.len() == self.capacity {
            inner.entries.pop_front();
        }
        inner.entries.push_back(Entry {
            seq,
            at_ms: unix_ms(),
            body,
        });
    }

    /// 当前窗口内的全部记录，按时间顺序。
    pub fn snapshot(&self) -> Vec<Entry> {
        let inner = self
            .inner
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        inner.entries.iter().cloned().collect()
    }

    /// 当前记录条数。
    pub fn len(&self) -> usize {
        self.inner
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .entries
            .len()
    }

    /// 是否一条都还没有。
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// 故障触发：把当前窗口写成 JSONL，第一行是故障本身。
    ///
    /// 返回写入的记录条数。写文件失败直接返回错误——黑匣子的意义就是落盘，
    /// 静默失败比不写更糟。
    pub fn dump_to(&self, path: &Path, fault: &Fault) -> std::io::Result<usize> {
        let entries = self.snapshot();
        let file = std::fs::File::create(path)?;
        let mut out = std::io::BufWriter::new(file);

        let header = serde_json::json!({
            "kind": "dump",
            "at_ms": unix_ms(),
            "capacity": self.capacity,
            "entries": entries.len(),
            "fault": fault,
        });
        writeln!(out, "{header}")?;
        for entry in &entries {
            writeln!(out, "{}", serde_json::to_string(entry)?)?;
        }
        out.flush()?;
        Ok(entries.len())
    }
}

fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0)
}

fn hex(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len() * 3);
    for (index, byte) in data.iter().enumerate() {
        if index > 0 {
            out.push(' ');
        }
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fault::{Domain, FaultCode, Severity};

    fn note(name: &str) -> BlackBox {
        let black_box = BlackBox::new(4);
        black_box.record_note(name, "detail");
        black_box
    }

    #[test]
    fn window_keeps_only_the_latest_entries() {
        let black_box = BlackBox::new(3);
        for index in 0..5 {
            black_box.record_note("tick", index.to_string());
        }
        let entries = black_box.snapshot();
        assert_eq!(entries.len(), 3, "只留最近三条");
        assert_eq!(black_box.len(), 3);
        assert_eq!(entries[0].seq, 2, "最早的两条被挤掉");
        assert_eq!(entries[2].seq, 4);
    }

    #[test]
    fn capacity_zero_is_treated_as_one() {
        let black_box = BlackBox::new(0);
        assert_eq!(black_box.capacity(), 1);
        black_box.record_note("a", "b");
        black_box.record_note("c", "d");
        assert_eq!(black_box.len(), 1);
    }

    #[test]
    fn frames_render_as_hex_and_are_ordered() {
        let black_box = BlackBox::new(4);
        black_box.record_frame(Direction::Tx, "vcan0", 0x201, &[0xff, 0x00]);
        black_box.record_frame(Direction::Rx, "vcan0", 0x5a1, &[0xfe, 0x10]);
        let entries = black_box.snapshot();
        assert_eq!(
            entries[0].body,
            EntryBody::Frame {
                dir: "tx",
                channel: "vcan0".to_owned(),
                id: 0x201,
                data: "ff 00".to_owned(),
            }
        );
        assert!(entries[1].at_ms > 0);
    }

    #[test]
    fn empty_box_reports_empty() {
        let black_box = BlackBox::new(2);
        assert!(black_box.is_empty());
        assert!(black_box.snapshot().is_empty());
    }

    #[test]
    fn dump_writes_fault_header_then_window() {
        let dir = std::env::temp_dir().join(format!("openape-blackbox-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("建临时目录");
        let path = dir.join("dump.jsonl");

        let black_box = note("xcp_connect");
        black_box.record_frame(Direction::Rx, "vcan0", 0x5a1, &[0xff, 0xff, 0x00]);
        let fault = Fault::new(
            FaultCode::new(Domain::Xcp, 11).expect("编号合法"),
            Severity::Error,
            "从站无响应",
        )
        .with_hint("确认从站已启动");

        let written = black_box.dump_to(&path, &fault).expect("要能落盘");
        assert_eq!(written, 2);

        let text = std::fs::read_to_string(&path).expect("读回");
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 3, "一行故障头 + 两条记录");
        assert!(lines[0].contains("\"kind\":\"dump\""), "{}", lines[0]);
        assert!(lines[0].contains("2011"), "故障编号要进头行：{}", lines[0]);
        assert!(lines[1].contains("\"kind\":\"note\""), "{}", lines[1]);
        assert!(lines[2].contains("ff ff 00"), "帧载荷要可读：{}", lines[2]);

        std::fs::remove_file(&path).ok();
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn dump_failure_is_reported() {
        let black_box = note("x");
        let fault = Fault::new(
            FaultCode::new(Domain::Internal, 1).expect("编号合法"),
            Severity::Error,
            "boom",
        );
        let bad = Path::new("/nonexistent-dir-openape/dump.jsonl");
        assert!(black_box.dump_to(bad, &fault).is_err(), "写不了要报错");
    }
}
