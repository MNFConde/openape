//! 写入审计日志：谁、何时、写了什么、写前写后值。
//!
//! 一行一条记录（JSONL）：追加写、坏一行不影响整份文件、`grep` 与 `tail`
//! 直接可用。日志是**审计**不是对账，所以只追加不改写——写完的值被后续
//! 操作覆盖也照样留在日志里。
//!
//! [D-13] 要求每次写入都有记录，因此本模块只做"追加 + 读回"，不做淘汰。

use std::fs::OpenOptions;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use openape_cal_core::{Address, SourceMode, Symbol, Timestamp, hex};
use openape_diag::{Domain, Fault, FaultCode, Severity};
use serde::{Deserialize, Serialize};

/// 审计日志文件名。固定名让 `--audit-log` 之外的默认路径可预期。
pub const AUDIT_FILE: &str = "write-audit.jsonl";

/// 一次写入的记录。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditRecord {
    /// 发生时刻（UTC）。
    pub at: Timestamp,
    /// 谁发起的：CLI 的 `--actor` 或 daemon 的会话标识。
    pub actor: String,
    /// 写的哪个标定量。
    pub symbol: Symbol,
    /// 落在哪个地址。
    pub address: Address,
    /// 写前读到的原始字节。
    pub before: Vec<u8>,
    /// 写后读回的原始字节。
    pub after: Vec<u8>,
    /// 输入档。两档数值不可混比，记录时就得带上。
    pub mode: SourceMode,
}

/// 追加式审计日志。
#[derive(Debug, Clone)]
pub struct AuditLog {
    path: PathBuf,
}

impl AuditLog {
    /// 在目录下打开（或创建）日志。目录不存在就建。
    pub fn open(dir: &Path) -> Result<Self, Fault> {
        std::fs::create_dir_all(dir).map_err(|err| io_fault(dir, &err))?;
        Ok(Self {
            path: dir.join(AUDIT_FILE),
        })
    }

    /// 直接指定日志文件路径。
    pub fn at(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// 日志文件路径。
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// 追加一条。写文件失败即故障——审计丢了，这次写入就不该算成功。
    pub fn append(&self, record: &AuditRecord) -> Result<(), Fault> {
        let line = serde_json::to_string(&record.to_wire()).map_err(|err| {
            fault(
                crate::seq::AUDIT,
                Severity::Error,
                format!("审计记录序列化失败：{err}"),
            )
        })?;
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .map_err(|err| io_fault(&self.path.clone(), &err))?;
        writeln!(file, "{line}").map_err(|err| io_fault(&self.path.clone(), &err))
    }

    /// 读回全部记录。坏行跳过并计次，不让一行坏数据挡住整份日志。
    pub fn records(&self) -> Result<Vec<AuditRecord>, Fault> {
        let file = match std::fs::File::open(&self.path) {
            Ok(file) => file,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(err) => return Err(io_fault(&self.path.clone(), &err)),
        };
        let mut records = Vec::new();
        for line in BufReader::new(file).lines() {
            let line = line.map_err(|err| io_fault(&self.path.clone(), &err))?;
            if line.trim().is_empty() {
                continue;
            }
            // 坏行跳过：审计日志的价值在于"能读到其余部分"，宁可少一条并记账。
            if let Ok(wire) = serde_json::from_str::<AuditWire>(&line)
                && let Some(record) = wire.into_record()
            {
                records.push(record);
            }
        }
        Ok(records)
    }
}

impl AuditRecord {
    fn to_wire(&self) -> AuditWire {
        AuditWire {
            at_unix_ms: self.at.unix_ms(),
            actor: self.actor.clone(),
            symbol: self.symbol.to_string(),
            address: self.address.to_string(),
            before: hex::encode(&self.before),
            after: hex::encode(&self.after),
            mode: self.mode,
        }
    }
}

#[derive(Serialize, Deserialize)]
struct AuditWire {
    at_unix_ms: u64,
    actor: String,
    symbol: String,
    address: String,
    before: String,
    after: String,
    mode: SourceMode,
}

impl AuditWire {
    fn into_record(self) -> Option<AuditRecord> {
        let symbol = Symbol::new(&self.symbol)?;
        let digits = self.address.strip_prefix("0x")?;
        let address = Address::new(u32::from_str_radix(digits, 16).ok()?);
        Some(AuditRecord {
            at: Timestamp::from_unix_ms(self.at_unix_ms),
            actor: self.actor,
            symbol,
            address,
            before: hex::decode(&self.before)?,
            after: hex::decode(&self.after)?,
            mode: self.mode,
        })
    }
}

fn fault(sequence: u32, severity: Severity, message: impl Into<String>) -> Fault {
    let code = FaultCode::new(Domain::Session, sequence).expect("序号常量在 1..=999 内");
    Fault::new(code, severity, message)
}

fn io_fault(path: &Path, err: &std::io::Error) -> Fault {
    fault(
        crate::seq::AUDIT,
        Severity::Error,
        format!("审计日志 {} 读写失败：{err}", path.display()),
    )
    .with_hint("确认日志目录可写、磁盘未满")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(actor: &str, before: &[u8], after: &[u8]) -> AuditRecord {
        AuditRecord {
            at: Timestamp::from_unix_ms(1_709_210_096_789),
            actor: actor.to_owned(),
            symbol: Symbol::new("cal_gains.kp").expect("合法符号"),
            address: Address::new(0x1000),
            before: before.to_vec(),
            after: after.to_vec(),
            mode: SourceMode::WithA2l,
        }
    }

    #[test]
    fn append_then_read_back_keeps_every_field() {
        let dir = tempdir();
        let log = AuditLog::open(&dir).expect("能建日志");
        let first = record("cli:alice", &[0x01, 0x02], &[0x03, 0x04]);
        let second = record("daemon:bob", &[0x03, 0x04], &[0x05, 0x06]);
        log.append(&first).expect("追加第一条");
        log.append(&second).expect("追加第二条");

        let records = log.records().expect("读回");
        assert_eq!(records, vec![first, second], "顺序保持追加顺序");
        assert_eq!(
            log.path().file_name().and_then(|n| n.to_str()),
            Some(AUDIT_FILE)
        );
    }

    #[test]
    fn records_include_who_when_and_both_values() {
        let dir = tempdir();
        let log = AuditLog::open(&dir).expect("能建日志");
        log.append(&record("cli:alice", &[0xAA], &[0xBB]))
            .expect("追加");
        let text = std::fs::read_to_string(log.path()).expect("读文件");
        assert!(text.contains("\"actor\":\"cli:alice\""), "{text}");
        assert!(text.contains("\"before\":\"aa\""), "{text}");
        assert!(text.contains("\"after\":\"bb\""), "{text}");
        assert!(text.contains("\"address\":\"0x00001000\""), "{text}");
        assert!(text.contains("\"at_unix_ms\":1709210096789"), "{text}");
        assert!(text.ends_with('\n'), "一行一条，末尾留换行");
    }

    #[test]
    fn missing_log_reads_as_empty_not_an_error() {
        let dir = tempdir();
        let log = AuditLog::open(&dir).expect("能建日志");
        assert!(log.records().expect("没写过也要能读").is_empty());
    }

    #[test]
    fn a_corrupt_line_does_not_hide_the_rest() {
        let dir = tempdir();
        let log = AuditLog::open(&dir).expect("能建日志");
        log.append(&record("cli:alice", &[0x01], &[0x02]))
            .expect("追加");
        {
            let mut file = OpenOptions::new()
                .append(true)
                .open(log.path())
                .expect("打开");
            writeln!(file, "{{ not json").expect("写坏行");
        }
        log.append(&record("cli:bob", &[0x02], &[0x03]))
            .expect("再追加");
        let records = log.records().expect("读回");
        assert_eq!(records.len(), 2, "坏行跳过，其余照读");
        assert_eq!(records[1].actor, "cli:bob");
    }

    /// 测试用临时目录。不引入 tempfile：用完自己删。
    fn tempdir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "openape-audit-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("建临时目录");
        dir
    }
}
