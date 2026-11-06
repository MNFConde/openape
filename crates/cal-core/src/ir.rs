//! 变量树与观测/标定量模型。
//!
//! 形状由 ELF 与 A2L 两份输入共同决定（[D-13]）：两份都有的（符号、地址、
//! 元素类型、维度、角色）是必填字段；只有 A2L 才有的（单位、量程、换算）
//! 一律 `Option`，缺失即"按裸值展示"，不填默认值假装知道。
//!
//! 非法状态在构造处就被拒绝：维度含 0、量程上下限颠倒、符号为空都造不出
//! 类型实例。字节长度不单独存——它由元素宽度与维度算出来，改一处不会与
//! 另一处漂移。
//!
//! [D-13]: ../../docs/decisions.md

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::types::{Address, ByteLen};

/// 变量符号路径，如 `cal_gains.kp`。
///
/// 空符号或首尾带空白的符号没有意义，构造时就拒绝。
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Symbol(String);

impl Symbol {
    /// 构造符号；空串或首尾空白返回 `None`。
    pub fn new(raw: &str) -> Option<Self> {
        if raw.is_empty() || raw.trim() != raw {
            return None;
        }
        Some(Self(raw.to_owned()))
    }

    /// 符号原文。
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for Symbol {
    type Error = BadSymbol;

    fn try_from(raw: String) -> Result<Self, Self::Error> {
        Self::new(&raw).ok_or(BadSymbol(raw))
    }
}

impl From<Symbol> for String {
    fn from(symbol: Symbol) -> Self {
        symbol.0
    }
}

impl fmt::Display for Symbol {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// 符号不合法：空串或首尾带空白。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BadSymbol(String);

impl fmt::Display for BadSymbol {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "符号 {:?} 不合法：不能为空，且首尾不能有空白", self.0)
    }
}

impl std::error::Error for BadSymbol {}

/// 元素类型。XCP 能按字节读写的基本类型全在这里。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum ElemType {
    /// 无符号 8 位。
    U8,
    /// 有符号 8 位。
    I8,
    /// 无符号 16 位。
    U16,
    /// 有符号 16 位。
    I16,
    /// 无符号 32 位。
    U32,
    /// 有符号 32 位。
    I32,
    /// 32 位浮点。
    F32,
    /// 64 位浮点。
    F64,
}

impl ElemType {
    /// 单个元素的字节宽度。
    pub const fn width(self) -> ByteLen {
        match self {
            Self::U8 | Self::I8 => ByteLen::ONE,
            Self::U16 | Self::I16 => ByteLen::TWO,
            Self::U32 | Self::I32 | Self::F32 => ByteLen::FOUR,
            Self::F64 => ByteLen::EIGHT,
        }
    }
}

/// 变量扮演的角色。仅 ELF 档没有语义来源，由用户指定，故为 `Option`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum Role {
    /// 观测：只读，进采集通路。
    Measurement,
    /// 标定：可写，进标定表格与写入通路。
    Characteristic,
}

/// 物理单位，如 `rpm`。A2L 的空单位表示"没有单位"，在这里是 `None`。
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Unit(String);

impl Unit {
    /// 构造单位；空串返回 `None`——"没有单位"用 `Option` 表达，不是空串。
    pub fn new(raw: &str) -> Option<Self> {
        if raw.is_empty() {
            None
        } else {
            Some(Self(raw.to_owned()))
        }
    }

    /// 单位原文。
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for Unit {
    type Error = EmptyUnit;

    fn try_from(raw: String) -> Result<Self, Self::Error> {
        Self::new(&raw).ok_or(EmptyUnit)
    }
}

impl From<Unit> for String {
    fn from(unit: Unit) -> Self {
        unit.0
    }
}

impl fmt::Display for Unit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// 单位为空。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EmptyUnit;

impl fmt::Display for EmptyUnit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("单位不能为空串；没有单位请用 None")
    }
}

impl std::error::Error for EmptyUnit {}

/// 量程。上下限颠倒或含 NaN 都造不出来。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "RangeWire")]
pub struct Range {
    min: f64,
    max: f64,
}

impl Range {
    /// 构造量程；`min > max` 或任一为 NaN 返回 `None`。
    pub fn new(min: f64, max: f64) -> Option<Self> {
        if min.is_nan() || max.is_nan() || min > max {
            return None;
        }
        Some(Self { min, max })
    }

    /// 下限。
    pub fn min(&self) -> f64 {
        self.min
    }

    /// 上限。
    pub fn max(&self) -> f64 {
        self.max
    }

    /// 值是否落在量程内（闭区间）。
    pub fn contains(&self, value: f64) -> bool {
        (self.min..=self.max).contains(&value)
    }
}

/// 量程的线格式。存在的唯一理由是给 [`Range`] 一个可校验的反序列化入口。
#[derive(Deserialize)]
struct RangeWire {
    min: f64,
    max: f64,
}

impl TryFrom<RangeWire> for Range {
    type Error = BadRange;

    fn try_from(wire: RangeWire) -> Result<Self, Self::Error> {
        Self::new(wire.min, wire.max).ok_or(BadRange {
            min: wire.min,
            max: wire.max,
        })
    }
}

/// 量程不合法：下限大于上限，或含 NaN。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BadRange {
    /// 给出的下限。
    pub min: f64,
    /// 给出的上限。
    pub max: f64,
}

impl fmt::Display for BadRange {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "量程 [{}, {}] 不合法：需满足 min <= max 且不含 NaN",
            self.min, self.max
        )
    }
}

impl std::error::Error for BadRange {}

/// 原始值到物理值的换算。A2L 档专有。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
#[non_exhaustive]
pub enum Scale {
    /// `physical = raw * factor + offset`。
    Linear {
        /// 增益。
        factor: f64,
        /// 偏移。
        offset: f64,
    },
}

impl Scale {
    /// 应用换算。
    pub fn apply(&self, raw: f64) -> f64 {
        match *self {
            Self::Linear { factor, offset } => raw * factor + offset,
        }
    }
}

/// 语义字段集合。**全部可为空**——仅 ELF 档一个都填不出来，也不该编。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Semantics {
    #[serde(skip_serializing_if = "Option::is_none")]
    unit: Option<Unit>,
    #[serde(skip_serializing_if = "Option::is_none")]
    range: Option<Range>,
    #[serde(skip_serializing_if = "Option::is_none")]
    scale: Option<Scale>,
}

impl Semantics {
    /// 什么都没有——仅 ELF 档的默认状态。
    pub fn none() -> Self {
        Self::default()
    }

    /// 补单位。
    pub fn with_unit(mut self, unit: Unit) -> Self {
        self.unit = Some(unit);
        self
    }

    /// 补量程。
    pub fn with_range(mut self, range: Range) -> Self {
        self.range = Some(range);
        self
    }

    /// 补换算。
    pub fn with_scale(mut self, scale: Scale) -> Self {
        self.scale = Some(scale);
        self
    }

    /// 单位。
    pub fn unit(&self) -> Option<&Unit> {
        self.unit.as_ref()
    }

    /// 量程。
    pub fn range(&self) -> Option<Range> {
        self.range
    }

    /// 换算。
    pub fn scale(&self) -> Option<Scale> {
        self.scale
    }

    /// 三个字段是否都缺失。
    pub fn is_empty(&self) -> bool {
        self.unit.is_none() && self.range.is_none() && self.scale.is_none()
    }
}

/// 一条变量：符号、地址、元素类型、维度，加上可选的语义。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Variable {
    symbol: Symbol,
    address: Address,
    elem: ElemType,
    dims: Vec<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    role: Option<Role>,
    #[serde(skip_serializing_if = "Semantics::is_empty")]
    semantics: Semantics,
}

impl Variable {
    /// 造一条标量变量：无角色、无语义。
    pub fn new(symbol: Symbol, address: Address, elem: ElemType) -> Self {
        Self {
            symbol,
            address,
            elem,
            dims: Vec::new(),
            role: None,
            semantics: Semantics::none(),
        }
    }

    /// 加维度。空列表表示标量；含 0 的维度没有意义，返回 `None`。
    pub fn with_dims(mut self, dims: Vec<u32>) -> Option<Self> {
        if dims.contains(&0) {
            return None;
        }
        self.dims = dims;
        Some(self)
    }

    /// 指定角色。
    pub fn with_role(mut self, role: Role) -> Self {
        self.role = Some(role);
        self
    }

    /// 补语义。
    pub fn with_semantics(mut self, semantics: Semantics) -> Self {
        self.semantics = semantics;
        self
    }

    /// 符号。
    pub fn symbol(&self) -> &Symbol {
        &self.symbol
    }

    /// 首地址。
    pub fn address(&self) -> Address {
        self.address
    }

    /// 元素类型。
    pub fn elem(&self) -> ElemType {
        self.elem
    }

    /// 各维长度；空表示标量。
    pub fn dims(&self) -> &[u32] {
        &self.dims
    }

    /// 角色；仅 ELF 档为 `None`。
    pub fn role(&self) -> Option<Role> {
        self.role
    }

    /// 语义字段。
    pub fn semantics(&self) -> &Semantics {
        &self.semantics
    }

    /// 元素个数。标量为 1。
    pub fn elem_count(&self) -> u64 {
        self.dims.iter().map(|&d| u64::from(d)).product()
    }

    /// 总字节长度。超过 64 KiB 或乘法溢出返回 `None`——XCP-on-CAN 单次
    /// 读不了这么大的块，属于调用方必须知道的错误。
    pub fn byte_len(&self) -> Option<ByteLen> {
        let total = self
            .elem_count()
            .checked_mul(u64::from(self.elem.width().get()))?;
        u16::try_from(total).ok().and_then(ByteLen::new)
    }
}

/// 变量树：按符号索引的一批变量，符号唯一。
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(transparent)]
pub struct VariableTree {
    variables: Vec<Variable>,
}

impl VariableTree {
    /// 建树。符号重复返回 `Err` 并指出是哪个符号——静默去重会让用户
    /// 以为少了一个变量。
    pub fn new(variables: Vec<Variable>) -> Result<Self, DuplicateSymbol> {
        let mut seen = BTreeMap::new();
        for variable in &variables {
            if let Some(previous) = seen.insert(variable.symbol.clone(), variable.address()) {
                return Err(DuplicateSymbol {
                    symbol: variable.symbol.clone(),
                    first: previous,
                    second: variable.address(),
                });
            }
        }
        Ok(Self { variables })
    }

    /// 按符号查变量。
    pub fn get(&self, symbol: &Symbol) -> Option<&Variable> {
        self.variables.iter().find(|v| &v.symbol == symbol)
    }

    /// 遍历全部变量。
    pub fn iter(&self) -> impl Iterator<Item = &Variable> {
        self.variables.iter()
    }

    /// 变量条数。
    pub fn len(&self) -> usize {
        self.variables.len()
    }

    /// 是否为空。
    pub fn is_empty(&self) -> bool {
        self.variables.is_empty()
    }
}

/// 符号在树里出现了两次。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DuplicateSymbol {
    /// 重复的符号。
    pub symbol: Symbol,
    /// 第一次出现的地址。
    pub first: Address,
    /// 第二次出现的地址。
    pub second: Address,
}

impl fmt::Display for DuplicateSymbol {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "符号 {} 重复：先出现在 {}，又出现在 {}",
            self.symbol, self.first, self.second
        )
    }
}

impl std::error::Error for DuplicateSymbol {}

#[cfg(test)]
mod tests {
    use super::*;

    fn symbol(raw: &str) -> Symbol {
        Symbol::new(raw).expect("测试符号合法")
    }

    fn addr(raw: u32) -> Address {
        Address::new(raw)
    }

    #[test]
    fn symbol_rejects_empty_and_padded() {
        assert!(Symbol::new("").is_none());
        assert!(Symbol::new(" lead").is_none());
        assert!(Symbol::new("trail ").is_none());
        assert_eq!(
            Symbol::new("cal_map").map(|s| s.to_string()),
            Some("cal_map".to_owned())
        );
        assert!(serde_json::from_str::<Symbol>("\"\"").is_err());
    }

    #[test]
    fn elem_width_covers_every_variant() {
        assert_eq!(ElemType::U8.width().get(), 1);
        assert_eq!(ElemType::I8.width().get(), 1);
        assert_eq!(ElemType::U16.width().get(), 2);
        assert_eq!(ElemType::I16.width().get(), 2);
        assert_eq!(ElemType::U32.width().get(), 4);
        assert_eq!(ElemType::I32.width().get(), 4);
        assert_eq!(ElemType::F32.width().get(), 4);
        assert_eq!(ElemType::F64.width().get(), 8);
    }

    #[test]
    fn scalar_byte_len_is_elem_width() {
        let v = Variable::new(symbol("kp"), addr(0x1000), ElemType::F32);
        assert_eq!(v.elem_count(), 1);
        assert_eq!(v.byte_len().map(ByteLen::get), Some(4));
    }

    #[test]
    fn matrix_byte_len_multiplies_dims() {
        // RFC 第 8 节的 cal_map[4][3]
        let v = Variable::new(symbol("cal_map"), addr(0x2060), ElemType::F32)
            .with_dims(vec![4, 3])
            .expect("[4][3] 合法");
        assert_eq!(v.elem_count(), 12);
        assert_eq!(v.byte_len().map(ByteLen::get), Some(48));
    }

    #[test]
    fn zero_dim_is_rejected() {
        let base = Variable::new(symbol("m"), addr(0x1000), ElemType::F32);
        assert!(base.clone().with_dims(vec![0]).is_none());
        assert!(base.with_dims(vec![4, 0]).is_none());
    }

    #[test]
    fn oversized_variable_has_no_byte_len() {
        // 8 × 8192 × 4 B = 256 KiB，超出单次读取能力
        let v = Variable::new(symbol("big"), addr(0x1000), ElemType::U32)
            .with_dims(vec![8192, 8])
            .expect("维度合法");
        assert!(v.byte_len().is_none());
    }

    #[test]
    fn elf_only_variable_serializes_without_filler() {
        let v = Variable::new(symbol("cal_map"), addr(0x2060), ElemType::F32)
            .with_dims(vec![4, 3])
            .expect("合法");
        let json = serde_json::to_value(&v).expect("可序列化");
        assert!(json.get("role").is_none(), "无角色就不该出现 key：{json}");
        assert!(
            json.get("semantics").is_none(),
            "无语义就不该出现 key：{json}"
        );
        assert_eq!(json["dims"], serde_json::json!([4, 3]));
    }

    #[test]
    fn semantics_apply_scales_linearly() {
        let scale = Scale::Linear {
            factor: 0.5,
            offset: -10.0,
        };
        assert_eq!(scale.apply(40.0), 10.0);
    }

    #[test]
    fn range_rejects_inverted_and_nan() {
        assert!(Range::new(5.0, 1.0).is_none());
        assert!(Range::new(f64::NAN, 1.0).is_none());
        let range = Range::new(-1.0e9, 1.0e9).expect("合法");
        assert!(range.contains(0.0));
        assert!(!range.contains(1.0e10));
    }

    #[test]
    fn unit_rejects_empty() {
        assert!(Unit::new("").is_none());
        assert_eq!(
            Unit::new("rpm").map(|u| u.to_string()),
            Some("rpm".to_owned())
        );
    }

    #[test]
    fn tree_rejects_duplicate_symbol_with_both_addresses() {
        let a = Variable::new(symbol("dup"), addr(0x1000), ElemType::U8);
        let b = Variable::new(symbol("dup"), addr(0x2000), ElemType::U8);
        let err = VariableTree::new(vec![a, b]).expect_err("重复必须报错");
        assert_eq!(err.symbol.to_string(), "dup");
        assert_eq!(err.first, addr(0x1000));
        assert_eq!(err.second, addr(0x2000));
        assert!(err.to_string().contains("0x00002000"));
    }

    #[test]
    fn tree_looks_up_by_symbol() {
        let tree = VariableTree::new(vec![
            Variable::new(symbol("a"), addr(0x1000), ElemType::U8),
            Variable::new(symbol("b"), addr(0x1004), ElemType::F32),
        ])
        .expect("无重复");
        assert_eq!(tree.len(), 2);
        assert_eq!(
            tree.get(&symbol("b")).map(Variable::address),
            Some(addr(0x1004))
        );
        assert!(tree.get(&symbol("c")).is_none());
        assert!(!tree.is_empty());
        assert!(VariableTree::new(Vec::new()).expect("空树合法").is_empty());
    }
}
