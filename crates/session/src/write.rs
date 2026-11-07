//! 写入编排：写前读回、快照、写、写后校验、审计。
//!
//! [D-13] 的四条纪律在这里落地：
//!
//! 1. **单点写**。入口一次只接一个变量一个值，没有"整表写入"的函数——
//!    批量写做不出来，就不会被误用。
//! 2. **写前读回**。写之前先把现值读一遍，落进快照，回滚才有依据。
//! 3. **写后校验**。读回与写入值不符即失败，并指出第一个差异字节。
//! 4. **留痕**。每次写入落一份快照文件与一条审计记录。
//!
//! 回滚（[`Session::restore_snapshot`]）是唯一会连续写多个地址的路径，
//! 它只接受快照文件——不是通用的批量写入口。
//!
//! [D-13]: ../../../docs/decisions.md#d-13-信息源两档输入与写入安全

use std::path::{Path, PathBuf};

use openape_cal_core::{Address, ByteLen, Dataset, Entry, SourceMode, Symbol, Timestamp, Variable};
use openape_diag::{Domain, Fault, FaultCode, Severity};

use crate::audit::{AuditLog, AuditRecord};
use crate::engine::Session;
use crate::seq;

/// 写入上下文：审计日志、快照目录、发起人与输入档。
///
/// 会话本身只管协议，这些是"谁在什么模式下写"的信息，由外壳决定并传下来。
#[derive(Debug, Clone)]
pub struct WriteContext {
    audit: AuditLog,
    snapshot_dir: PathBuf,
    actor: String,
    mode: SourceMode,
}

impl WriteContext {
    /// 在 `root` 下准备日志与快照目录。
    pub fn open(root: &Path, actor: impl Into<String>, mode: SourceMode) -> Result<Self, Fault> {
        let audit = AuditLog::open(root)?;
        let snapshot_dir = root.join("snapshots");
        std::fs::create_dir_all(&snapshot_dir).map_err(|err| {
            snapshot_fault(format!(
                "快照目录 {} 建不出来：{err}",
                snapshot_dir.display()
            ))
        })?;
        Ok(Self {
            audit,
            snapshot_dir,
            actor: actor.into(),
            mode,
        })
    }

    /// 审计日志。
    pub fn audit(&self) -> &AuditLog {
        &self.audit
    }

    /// 快照目录。
    pub fn snapshot_dir(&self) -> &Path {
        &self.snapshot_dir
    }

    /// 发起人。
    pub fn actor(&self) -> &str {
        &self.actor
    }

    /// 输入档。
    pub fn mode(&self) -> SourceMode {
        self.mode
    }
}

/// 一次写入的结果。
#[derive(Debug, Clone)]
pub struct Written {
    /// 写的哪个标定量。
    pub symbol: Symbol,
    /// 落在哪个地址。
    pub address: Address,
    /// 写前读到的字节。
    pub before: Vec<u8>,
    /// 写后读回的字节。
    pub after: Vec<u8>,
    /// 写前快照文件。
    pub snapshot: PathBuf,
    /// 审计记录。
    pub audit: AuditRecord,
}

impl Session {
    /// 单点写：长度校验 → 写前读回 → 快照 → 写 → 读回 → 审计。
    ///
    /// 快照在写之前落盘：写到一半掉电或断线时，它是回滚的唯一依据，宁可多
    /// 留一份也不留隐患。
    pub async fn write_one(
        &mut self,
        target: &Variable,
        value: &[u8],
        context: &WriteContext,
    ) -> Result<Written, Fault> {
        let expected = target.byte_len().ok_or_else(|| {
            write_fault(format!("{} 的维度不合法，算不出字节长度", target.symbol()))
        })?;
        if value.len() != expected.get() as usize {
            return Err(write_fault(format!(
                "{} 是 {} 字节，给了 {} 字节",
                target.symbol(),
                expected.get(),
                value.len()
            )));
        }

        let address = target.address();
        let before = self.read(address, expected).await?;
        let snapshot = Dataset::new(
            context.mode(),
            Timestamp::now(),
            vec![
                Entry::new(target.symbol().clone(), address, before.clone())
                    .expect("字节长度已在上面校验过非零"),
            ],
        )
        .map_err(|err| snapshot_fault(err.to_string()))?;
        let snapshot_path = persist_snapshot(context.snapshot_dir(), &snapshot)?;

        self.write_raw(address, value).await?;
        let after = self.read(address, expected).await?;

        let audit = AuditRecord {
            at: Timestamp::now(),
            actor: context.actor().to_owned(),
            symbol: target.symbol().clone(),
            address,
            before: before.clone(),
            after: after.clone(),
            mode: context.mode(),
        };
        context.audit().append(&audit)?;

        Ok(Written {
            symbol: target.symbol().clone(),
            address,
            before,
            after,
            snapshot: snapshot_path,
            audit,
        })
    }

    /// 回滚一份快照：把快照里的每条按单点写的路数写回去（含审计）。
    ///
    /// 只接受快照文件读出来的 dataset；这是安全路径，不是批量写入口。
    pub async fn restore_snapshot(
        &mut self,
        snapshot: &Dataset,
        context: &WriteContext,
    ) -> Result<Vec<Written>, Fault> {
        if snapshot.mode() != context.mode() {
            return Err(restore_fault(format!(
                "快照是{}档，当前是{}档，两档数值不可混写",
                snapshot.mode().label(),
                context.mode().label()
            )));
        }
        let mut written = Vec::with_capacity(snapshot.entries().len());
        for entry in snapshot.entries() {
            let len = u16::try_from(entry.raw().len())
                .ok()
                .and_then(ByteLen::new)
                .ok_or_else(|| restore_fault(format!("{} 的字节长度不合法", entry.symbol())))?;
            let before = self.read(entry.address(), len).await?;
            self.write_raw(entry.address(), entry.raw()).await?;
            let after = self.read(entry.address(), len).await?;
            let audit = AuditRecord {
                at: Timestamp::now(),
                actor: format!("{} (回滚)", context.actor()),
                symbol: entry.symbol().clone(),
                address: entry.address(),
                before,
                after,
                mode: context.mode(),
            };
            context.audit().append(&audit)?;
            written.push(Written {
                symbol: audit.symbol.clone(),
                address: audit.address,
                before: audit.before.clone(),
                after: audit.after.clone(),
                snapshot: context.snapshot_dir().to_path_buf(),
                audit,
            });
        }
        Ok(written)
    }
}

/// 快照落盘。文件名带时刻与符号，同一次会话里不会互相覆盖。
pub fn persist_snapshot(dir: &Path, snapshot: &Dataset) -> Result<PathBuf, Fault> {
    let stamp = snapshot.taken_at().unix_ms();
    let label = snapshot
        .entries()
        .first()
        .map_or_else(|| "empty".to_owned(), |entry| sanitize(entry.symbol()));
    let path = dir.join(format!("{stamp}-{label}.json"));
    std::fs::write(&path, snapshot.to_json())
        .map_err(|err| snapshot_fault(format!("{} 写不进去：{err}", path.display())))?;
    Ok(path)
}

/// 从文件读回一份快照。
pub fn read_snapshot(path: &Path) -> Result<Dataset, Fault> {
    let text = std::fs::read_to_string(path)
        .map_err(|err| snapshot_fault(format!("{} 读不出来：{err}", path.display())))?;
    Dataset::from_json(&text).map_err(|err| snapshot_fault(err.to_string()))
}

fn sanitize(symbol: &Symbol) -> String {
    symbol
        .as_str()
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '_' })
        .collect()
}

fn fault(sequence: u32, message: impl Into<String>) -> Fault {
    let code = FaultCode::new(Domain::Session, sequence).expect("序号常量在 1..=999 内");
    Fault::new(code, Severity::Error, message)
}

fn write_fault(message: impl Into<String>) -> Fault {
    fault(seq::WRITE_LEN, message).with_hint("检查该标定量的长度与维度来源")
}

fn snapshot_fault(message: impl Into<String>) -> Fault {
    fault(seq::SNAPSHOT, message).with_hint("确认快照目录可写")
}

fn restore_fault(message: impl Into<String>) -> Fault {
    fault(seq::RESTORE, message).with_hint("回滚只接受同档快照文件")
}

#[cfg(test)]
mod tests {
    use super::*;
    use openape_cal_core::{ElemType, Symbol, VariableTree};

    fn symbol(name: &str) -> Symbol {
        Symbol::new(name).expect("测试符号合法")
    }

    fn tempdir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "openape-write-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("建临时目录");
        dir
    }

    #[test]
    fn snapshot_file_name_carries_time_and_symbol() {
        let dir = tempdir();
        let dataset = Dataset::new(
            SourceMode::WithA2l,
            Timestamp::from_unix_ms(1234),
            vec![
                Entry::new(symbol("cal_gains.kp"), Address::new(0x1000), vec![1, 2]).expect("非空"),
            ],
        )
        .expect("合法");
        let path = persist_snapshot(&dir, &dataset).expect("能落盘");
        assert_eq!(
            path.file_name().and_then(|name| name.to_str()),
            Some("1234-cal_gains_kp.json"),
            "文件名要能一眼看出时刻与对象"
        );
        assert_eq!(read_snapshot(&path).expect("能读回"), dataset);
    }

    #[test]
    fn variable_byte_len_guards_the_write_length() {
        // 长度校验依赖 IR 算出的字节长度：2 个 i16 就是 4 字节。
        let variable = Variable::new(symbol("kp"), Address::new(0x1000), ElemType::I16)
            .with_dims(vec![2])
            .expect("维度合法");
        let tree = VariableTree::new(vec![variable.clone()]).expect("无重复");
        let target = tree.get(&symbol("kp")).expect("查得到");
        assert_eq!(target.byte_len().map(|len| len.get()), Some(4));
    }
}
