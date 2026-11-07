//! dataset：一次快照的标定量取值集合。
//!
//! 三件事都由它承担：写前快照（可回滚）、页间 diff（只列差异地址）、跨次
//! 对比（dataset 与 dataset）。三者共用一种文件格式，避免"快照存一份、
//! 对比存另一份"。
//!
//! 两条硬约束来自 [D-13]：
//!
//! 1. **带模式标记**。仅 ELF 与带 A2L 的数值不可混比——前者没有换算，同一
//!    批字节在两者里的含义不同，混比出来的差异没有意义。
//! 2. **顺序稳定**。条目按（地址, 符号）排好再落盘，于是同一批取值的文件
//!    字节一致，diff 也能按顺序合并，不必再排一次。
//!
//! [D-13]: ../../../docs/decisions.md#d-13-信息源两档输入与写入安全

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::ir::Symbol;
use crate::time::Timestamp;
use crate::types::Address;

/// 本软件支持的 dataset 版本。读比它新的版本即拒绝，不猜字段含义。
pub const DATASET_SCHEMA_VERSION: u32 = 1;

/// 取值来自哪一档输入。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceMode {
    /// 只有 ELF：无语义层，数值按裸字节理解。
    ElfOnly,
    /// ELF 加 A2L：带单位、量程与换算。
    WithA2l,
}

impl SourceMode {
    /// 中文名，供界面与日志直接显示。
    pub const fn label(self) -> &'static str {
        match self {
            Self::ElfOnly => "仅 ELF",
            Self::WithA2l => "带 A2L",
        }
    }
}

/// 一条取值：符号、地址与 ECU 里的原始字节。
///
/// 存原始字节而不是物理值：回滚要能原样写回，换算过的数值写回去会引入
/// 一次往返误差。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    symbol: Symbol,
    address: Address,
    raw: Vec<u8>,
}

impl Entry {
    /// 构造。空字节没有意义，返回 `None`。
    pub fn new(symbol: Symbol, address: Address, raw: Vec<u8>) -> Option<Self> {
        (!raw.is_empty()).then_some(Self {
            symbol,
            address,
            raw,
        })
    }

    /// 符号。
    pub fn symbol(&self) -> &Symbol {
        &self.symbol
    }

    /// 地址。
    pub fn address(&self) -> Address {
        self.address
    }

    /// 原始字节。
    pub fn raw(&self) -> &[u8] {
        &self.raw
    }
}

/// 一份 dataset。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dataset {
    schema_version: u32,
    mode: SourceMode,
    taken_at: Timestamp,
    entries: Vec<Entry>,
}

impl Dataset {
    /// 组装。条目按（地址, 符号）排序；符号重复直接拒绝——同一个符号两个值
    /// 说明取数过程出了问题，静默留一个会让回滚写错地址。
    pub fn new(
        mode: SourceMode,
        taken_at: Timestamp,
        entries: Vec<Entry>,
    ) -> Result<Self, DatasetError> {
        let mut sorted = entries;
        sorted.sort_by(|left, right| {
            left.address
                .cmp(&right.address)
                .then_with(|| left.symbol.cmp(&right.symbol))
        });
        for pair in sorted.windows(2) {
            if pair[0].symbol == pair[1].symbol {
                return Err(DatasetError::DuplicateSymbol {
                    symbol: pair[0].symbol.clone(),
                });
            }
        }
        Ok(Self {
            schema_version: DATASET_SCHEMA_VERSION,
            mode,
            taken_at,
            entries: sorted,
        })
    }

    /// 版本。
    pub fn schema_version(&self) -> u32 {
        self.schema_version
    }

    /// 模式。
    pub fn mode(&self) -> SourceMode {
        self.mode
    }

    /// 快照时刻。
    pub fn taken_at(&self) -> Timestamp {
        self.taken_at
    }

    /// 条目，按（地址, 符号）有序。
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// 按符号查一条。
    pub fn get(&self, symbol: &Symbol) -> Option<&Entry> {
        self.entries.iter().find(|entry| &entry.symbol == symbol)
    }

    /// 序列化为 JSON 文本。字段顺序固定，同一批取值得到同一份字节。
    pub fn to_json(&self) -> String {
        let wire = DatasetWire {
            schema_version: self.schema_version,
            mode: self.mode,
            taken_at_unix_ms: self.taken_at.unix_ms(),
            entries: self
                .entries
                .iter()
                .map(|entry| EntryWire {
                    symbol: entry.symbol.to_string(),
                    address: entry.address.to_string(),
                    raw: hex_encode(&entry.raw),
                })
                .collect(),
        };
        serde_json::to_string_pretty(&wire).unwrap_or_else(|_| String::from("{}"))
    }

    /// 从 JSON 文本读入。版本比本软件新即拒绝，不猜字段含义。
    pub fn from_json(text: &str) -> Result<Self, DatasetError> {
        let wire: DatasetWire = serde_json::from_str(text).map_err(|err| DatasetError::Syntax {
            message: err.to_string(),
        })?;
        if wire.schema_version > DATASET_SCHEMA_VERSION {
            return Err(DatasetError::VersionTooNew {
                found: wire.schema_version,
                supported: DATASET_SCHEMA_VERSION,
            });
        }
        let entries = wire
            .entries
            .into_iter()
            .map(EntryWire::into_entry)
            .collect::<Result<Vec<_>, _>>()?;
        // 读进来的版本原样保留：旧文件回写时不该被悄悄升级。
        let mut dataset = Self::new(
            wire.mode,
            Timestamp::from_unix_ms(wire.taken_at_unix_ms),
            entries,
        )?;
        dataset.schema_version = wire.schema_version;
        Ok(dataset)
    }
}

/// 一条差异。`left`/`right` 为 `None` 表示该侧没有这个符号。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Difference {
    symbol: Symbol,
    address: Address,
    left: Option<Vec<u8>>,
    right: Option<Vec<u8>>,
}

impl Difference {
    /// 符号。
    pub fn symbol(&self) -> &Symbol {
        &self.symbol
    }

    /// 地址（取存在的那一侧；两侧都在时取左侧）。
    pub fn address(&self) -> Address {
        self.address
    }

    /// 左侧字节；`None` 表示左侧没有这条。
    pub fn left(&self) -> Option<&[u8]> {
        self.left.as_deref()
    }

    /// 右侧字节；`None` 表示右侧没有这条。
    pub fn right(&self) -> Option<&[u8]> {
        self.right.as_deref()
    }
}

/// 比较两份 dataset，只列有差异的条目。
///
/// 两档混比直接拒绝：模式不同的数值即使字节相同也不可比。
pub fn diff(left: &Dataset, right: &Dataset) -> Result<Vec<Difference>, DatasetError> {
    if left.mode != right.mode {
        return Err(DatasetError::ModeMismatch {
            left: left.mode,
            right: right.mode,
        });
    }
    let mut differences = Vec::new();
    for entry in &left.entries {
        match right.get(&entry.symbol) {
            Some(other) if other.raw == entry.raw => continue,
            Some(other) => differences.push(Difference {
                symbol: entry.symbol.clone(),
                address: entry.address,
                left: Some(entry.raw.clone()),
                right: Some(other.raw.clone()),
            }),
            None => differences.push(Difference {
                symbol: entry.symbol.clone(),
                address: entry.address,
                left: Some(entry.raw.clone()),
                right: None,
            }),
        }
    }
    for entry in &right.entries {
        if left.get(&entry.symbol).is_none() {
            differences.push(Difference {
                symbol: entry.symbol.clone(),
                address: entry.address,
                left: None,
                right: Some(entry.raw.clone()),
            });
        }
    }
    // 两侧都按地址有序，合并后按地址（再符号）排一次，输出顺序与输入文件
    // 的顺序无关。
    differences.sort_by(|a, b| {
        a.address
            .cmp(&b.address)
            .then_with(|| a.symbol.cmp(&b.symbol))
    });
    Ok(differences)
}

/// dataset 读写的失败原因。每条都带足以定位问题的信息。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DatasetError {
    /// JSON 语法错。
    Syntax {
        /// 原始错误信息。
        message: String,
    },
    /// 版本比本软件新。
    VersionTooNew {
        /// 文件里的版本。
        found: u32,
        /// 本软件支持的版本。
        supported: u32,
    },
    /// 同一符号出现多次。
    DuplicateSymbol {
        /// 重复的符号。
        symbol: Symbol,
    },
    /// 符号或地址不合法。
    BadField {
        /// 字段名。
        field: &'static str,
        /// 原始文本。
        value: String,
    },
    /// 两档数值混比。
    ModeMismatch {
        /// 左侧模式。
        left: SourceMode,
        /// 右侧模式。
        right: SourceMode,
    },
}

impl fmt::Display for DatasetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Syntax { message } => write!(f, "dataset 不是合法 JSON：{message}"),
            Self::VersionTooNew { found, supported } => write!(
                f,
                "dataset 版本 {found} 比本软件支持的 {supported} 新，请升级后再读"
            ),
            Self::DuplicateSymbol { symbol } => {
                write!(f, "dataset 里符号 {symbol} 出现了多次")
            }
            Self::BadField { field, value } => {
                write!(f, "dataset 字段 {field} 的值 {value:?} 不合法")
            }
            Self::ModeMismatch { left, right } => write!(
                f,
                "两侧 dataset 的输入档不同（{} 与 {}），数值不可混比",
                left.label(),
                right.label()
            ),
        }
    }
}

impl std::error::Error for DatasetError {}

#[derive(Serialize, Deserialize)]
struct DatasetWire {
    schema_version: u32,
    mode: SourceMode,
    taken_at_unix_ms: u64,
    entries: Vec<EntryWire>,
}

#[derive(Serialize, Deserialize)]
struct EntryWire {
    symbol: String,
    address: String,
    raw: String,
}

impl EntryWire {
    fn into_entry(self) -> Result<Entry, DatasetError> {
        let symbol = Symbol::new(&self.symbol).ok_or_else(|| DatasetError::BadField {
            field: "symbol",
            value: self.symbol.clone(),
        })?;
        let address = parse_address(&self.address).ok_or_else(|| DatasetError::BadField {
            field: "address",
            value: self.address.clone(),
        })?;
        let raw = hex_decode(&self.raw).ok_or_else(|| DatasetError::BadField {
            field: "raw",
            value: self.raw.clone(),
        })?;
        Entry::new(symbol, address, raw).ok_or_else(|| DatasetError::BadField {
            field: "raw",
            value: self.raw.clone(),
        })
    }
}

/// 地址写成 `0x00001000`：与界面和 proto 的写法一致，也是人能读的形式。
fn parse_address(text: &str) -> Option<Address> {
    let digits = text
        .strip_prefix("0x")
        .or_else(|| text.strip_prefix("0X"))?;
    u32::from_str_radix(digits, 16).ok().map(Address::new)
}

fn hex_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

fn hex_decode(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) {
        return None;
    }
    let digits = text.as_bytes();
    let mut out = Vec::with_capacity(text.len() / 2);
    for pair in digits.chunks(2) {
        let high = char::from(pair[0]).to_digit(16)?;
        let low = char::from(pair[1]).to_digit(16)?;
        out.push(u8::try_from(high * 16 + low).ok()?);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn symbol(name: &str) -> Symbol {
        Symbol::new(name).expect("测试符号合法")
    }

    fn entry(name: &str, address: u32, raw: &[u8]) -> Entry {
        Entry::new(symbol(name), Address::new(address), raw.to_vec()).expect("非空字节")
    }

    fn dataset(mode: SourceMode, entries: Vec<Entry>) -> Dataset {
        Dataset::new(mode, Timestamp::from_unix_ms(1), entries).expect("合法 dataset")
    }

    #[test]
    fn entries_are_sorted_by_address_then_symbol() {
        let mut built = dataset(
            SourceMode::WithA2l,
            vec![
                entry("b", 0x1004, &[2]),
                entry("a", 0x1004, &[1]),
                entry("c", 0x1000, &[0]),
            ],
        );
        assert_eq!(
            built
                .entries()
                .iter()
                .map(|e| e.symbol().as_str())
                .collect::<Vec<_>>(),
            vec!["c", "a", "b"]
        );
        // 同一批输入换个顺序，文件字节必须一样。
        let first = built.to_json();
        built = dataset(
            SourceMode::WithA2l,
            vec![
                entry("c", 0x1000, &[0]),
                entry("a", 0x1004, &[1]),
                entry("b", 0x1004, &[2]),
            ],
        );
        assert_eq!(first, built.to_json());
    }

    #[test]
    fn duplicate_symbols_are_rejected() {
        let err = Dataset::new(
            SourceMode::ElfOnly,
            Timestamp::from_unix_ms(1),
            vec![entry("kp", 0x1000, &[1]), entry("kp", 0x1004, &[2])],
        )
        .expect_err("重复符号");
        assert!(matches!(err, DatasetError::DuplicateSymbol { .. }));
        assert!(err.to_string().contains("kp"), "{err}");
    }

    #[test]
    fn zero_length_entry_cannot_be_built() {
        assert!(Entry::new(symbol("kp"), Address::new(0x1000), Vec::new()).is_none());
    }

    #[test]
    fn json_round_trips_with_the_mode_marker_and_hex_bytes() {
        let original = dataset(
            SourceMode::ElfOnly,
            vec![entry("kp", 0x1000, &[0xDE, 0xAD])],
        );
        let text = original.to_json();
        assert!(text.contains("\"mode\": \"elf_only\""), "{text}");
        assert!(text.contains("\"raw\": \"dead\""), "{text}");
        assert!(text.contains("\"address\": \"0x00001000\""), "{text}");
        assert_eq!(Dataset::from_json(&text).expect("能读回"), original);
    }

    #[test]
    fn older_versions_are_readable_and_keep_their_number() {
        let text = r#"{
            "schema_version": 0,
            "mode": "with_a2l",
            "taken_at_unix_ms": 1000,
            "entries": [{ "symbol": "kp", "address": "0x1000", "raw": "01" }]
        }"#;
        let dataset = Dataset::from_json(text).expect("旧版可读");
        assert_eq!(dataset.schema_version(), 0, "旧版不该被悄悄升级");
        assert_eq!(dataset.mode(), SourceMode::WithA2l);
        assert_eq!(dataset.entries().len(), 1);
    }

    #[test]
    fn newer_versions_are_refused_with_a_next_step() {
        let text = r#"{
            "schema_version": 99,
            "mode": "with_a2l",
            "taken_at_unix_ms": 1000,
            "entries": []
        }"#;
        let err = Dataset::from_json(text).expect_err("版本过新");
        assert!(matches!(err, DatasetError::VersionTooNew { .. }));
        assert!(err.to_string().contains("升级"), "{err}");
    }

    #[test]
    fn bad_fields_name_the_field_and_the_value() {
        let text = r#"{
            "schema_version": 1,
            "mode": "with_a2l",
            "taken_at_unix_ms": 1000,
            "entries": [{ "symbol": "kp", "address": "1000", "raw": "01" }]
        }"#;
        let err = Dataset::from_json(text).expect_err("地址缺 0x 前缀");
        match err {
            DatasetError::BadField { field, value } => {
                assert_eq!(field, "address");
                assert_eq!(value, "1000");
            }
            other => panic!("期望字段错误，实际 {other}"),
        }
    }

    #[test]
    fn diff_lists_only_differences_in_address_order() {
        let left = dataset(
            SourceMode::WithA2l,
            vec![
                entry("ka", 0x1000, &[1]),
                entry("kb", 0x1004, &[2]),
                entry("kc", 0x1008, &[3]),
            ],
        );
        let right = dataset(
            SourceMode::WithA2l,
            vec![
                entry("ka", 0x1000, &[1]),
                entry("kb", 0x1004, &[9]),
                entry("kd", 0x100C, &[4]),
            ],
        );
        let differences = diff(&left, &right).expect("同档可比");
        let names: Vec<&str> = differences
            .iter()
            .map(|difference| difference.symbol().as_str())
            .collect();
        assert_eq!(names, vec!["kb", "kc", "kd"], "只列差异，按地址排序");
        assert_eq!(differences[0].left(), Some(&[2u8][..]));
        assert_eq!(differences[0].right(), Some(&[9u8][..]));
        assert_eq!(differences[1].right(), None, "右侧没有这条");
        assert_eq!(differences[2].left(), None, "左侧没有这条");
    }

    #[test]
    fn diff_refuses_to_compare_across_input_modes() {
        let elf = dataset(SourceMode::ElfOnly, vec![entry("ka", 0x1000, &[1])]);
        let a2l = dataset(SourceMode::WithA2l, vec![entry("ka", 0x1000, &[1])]);
        let err = diff(&elf, &a2l).expect_err("两档数值不可混比");
        assert!(matches!(err, DatasetError::ModeMismatch { .. }));
        assert!(err.to_string().contains("仅 ELF"), "{err}");
    }

    #[test]
    fn diff_of_identical_datasets_is_empty() {
        let one = dataset(SourceMode::WithA2l, vec![entry("ka", 0x1000, &[1])]);
        let two = dataset(SourceMode::WithA2l, vec![entry("ka", 0x1000, &[1])]);
        assert!(diff(&one, &two).expect("同档可比").is_empty());
    }

    #[test]
    fn hex_helpers_round_trip_and_reject_odd_input() {
        assert_eq!(hex_encode(&[0x00, 0xFF, 0x10]), "00ff10");
        assert_eq!(hex_decode("00ff10"), Some(vec![0x00, 0xFF, 0x10]));
        assert_eq!(hex_decode("abc"), None, "奇数长度不是合法字节串");
        assert_eq!(hex_decode("zz"), None, "非十六进制字符");
        assert_eq!(hex_decode(""), Some(Vec::new()));
    }
}
