//! Curve / Map 与轴：轴的建模、引用与 R1–R7 校验。
//!
//! 轴是独立实体而非值的附属属性——两个 Map 共用一条温度轴时不复制数据
//! （对应 A2L 的 `COM_AXIS`）。配置只声明"谁引用谁"，长度与元素类型一律
//! 从变量树取：数据源是权威，配置写错长度会被 R1 挡住而不是覆盖事实。
//!
//! 规则编号与 [10-axis-binding-rfc] 第 6 节一一对应。
//!
//! [10-axis-binding-rfc]: ../../../../docs/10-axis-binding-rfc.md

use std::fmt;

use serde::ser::SerializeStruct;
use serde::{Deserialize, Serialize, Serializer};

use crate::ir::{ElemType, Symbol, Variable, VariableTree};
use crate::types::Address;

/// 工程内标识：轴与值共用命名空间，全局唯一。
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Id(String);

impl Id {
    /// 构造 id；空串或首尾带空白返回 `None`。
    pub fn new(raw: &str) -> Option<Self> {
        crate::ir::check_name(raw).then(|| Self(raw.to_owned()))
    }

    /// id 原文。
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for Id {
    type Error = BadId;

    fn try_from(raw: String) -> Result<Self, Self::Error> {
        Self::new(&raw).ok_or(BadId(raw))
    }
}

impl From<Id> for String {
    fn from(id: Id) -> Self {
        id.0
    }
}

impl fmt::Display for Id {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// id 不合法：空串或首尾带空白。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BadId(String);

impl fmt::Display for BadId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "id {:?} 不合法：不能为空，且首尾不能有空白", self.0)
    }
}

impl std::error::Error for BadId {}

/// 轴的存放方式。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AxisKind {
    /// 轴数据独立存放，多个值可引用（A2L 的 `AXIS_PTS`）。
    #[default]
    Shared,
    /// 轴数据内嵌在值数组里（A2L 的 `STD_AXIS`）。
    Inline,
}

/// 配置里的轴声明。长度不在这里——它由符号指向的变量决定。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AxisSpec {
    id: Id,
    symbol: Symbol,
    kind: AxisKind,
}

impl AxisSpec {
    /// 声明一条轴，`kind` 缺省为 shared。
    pub fn new(id: Id, symbol: Symbol) -> Self {
        Self {
            id,
            symbol,
            kind: AxisKind::Shared,
        }
    }

    /// 指定存放方式。
    pub fn with_kind(mut self, kind: AxisKind) -> Self {
        self.kind = kind;
        self
    }

    /// id。
    pub fn id(&self) -> &Id {
        &self.id
    }

    /// 符号。
    pub fn symbol(&self) -> &Symbol {
        &self.symbol
    }

    /// 存放方式。
    pub fn kind(&self) -> AxisKind {
        self.kind
    }
}

/// 配置里的值声明。维度数由数据源决定，不由声明它的节名决定。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValueSpec {
    id: Id,
    symbol: Symbol,
    axes: Vec<Id>,
}

impl ValueSpec {
    /// 声明一个 Curve 或 Map，`axes` 顺序与 DWARF 的 `dim` 顺序一致。
    pub fn new(id: Id, symbol: Symbol, axes: Vec<Id>) -> Self {
        Self { id, symbol, axes }
    }

    /// id。
    pub fn id(&self) -> &Id {
        &self.id
    }

    /// 符号。
    pub fn symbol(&self) -> &Symbol {
        &self.symbol
    }

    /// 轴引用，顺序对应 `dim` 顺序。
    pub fn axes(&self) -> &[Id] {
        &self.axes
    }
}

/// 一份绑定配置：若干轴声明加若干值声明。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BindingSpec {
    axes: Vec<AxisSpec>,
    values: Vec<ValueSpec>,
}

impl BindingSpec {
    /// 组装配置。
    pub fn new(axes: Vec<AxisSpec>, values: Vec<ValueSpec>) -> Self {
        Self { axes, values }
    }

    /// 轴声明。
    pub fn axes(&self) -> &[AxisSpec] {
        &self.axes
    }

    /// 值声明。
    pub fn values(&self) -> &[ValueSpec] {
        &self.values
    }

    /// 是否一条声明都没有。
    pub fn is_empty(&self) -> bool {
        self.axes.is_empty() && self.values.is_empty()
    }
}

/// 校验规则编号。与 RFC 第 6 节一一对应；R6 是"允许"，不产生诊断，
/// 故没有对应变体。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[non_exhaustive]
pub enum Rule {
    /// 轴点数与对应维度不一致。
    AxisLenMismatch,
    /// `axes` 长度与维度数不一致，或值不是数组。
    ArityMismatch,
    /// 轴符号与值符号相同。
    AxisSymbolEqualsValue,
    /// id 重复，或引用的 id / 符号不存在。
    Unresolved,
    /// 多维数组没有绑定，按裸数组展示。
    UnboundArray,
    /// 轴符号不是一维数值数组。
    AxisShape,
}

impl Rule {
    /// 规则编号，用于报告与测试断言。
    pub const fn label(self) -> &'static str {
        match self {
            Self::AxisLenMismatch => "R1",
            Self::ArityMismatch => "R2",
            Self::AxisSymbolEqualsValue => "R3",
            Self::Unresolved => "R4",
            Self::UnboundArray => "R5",
            Self::AxisShape => "R7",
        }
    }

    /// 严重度。唯一源在这里，报告与筛选都读它。
    pub const fn severity(self) -> Severity {
        match self {
            Self::UnboundArray => Severity::Warning,
            Self::AxisLenMismatch
            | Self::ArityMismatch
            | Self::AxisSymbolEqualsValue
            | Self::Unresolved
            | Self::AxisShape => Severity::Error,
        }
    }
}

impl fmt::Display for Rule {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

/// 诊断严重度。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    /// 警告：不阻断加载。
    Warning,
    /// 错误：拒绝加载。
    Error,
}

/// 一条诊断。`subject` 是被点名的轴或值的 id，便于用户定位到配置行。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    rule: Rule,
    subject: String,
    message: String,
}

impl Diagnostic {
    fn new(rule: Rule, subject: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            rule,
            subject: subject.into(),
            message: message.into(),
        }
    }

    /// 命中的规则。
    pub fn rule(&self) -> Rule {
        self.rule
    }

    /// 严重度，取自规则。
    pub fn severity(&self) -> Severity {
        self.rule.severity()
    }

    /// 被点名的对象。
    pub fn subject(&self) -> &str {
        &self.subject
    }

    /// 说明。
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} {} [{}] {}：{}",
            match self.severity() {
                Severity::Error => "错误",
                Severity::Warning => "警告",
            },
            self.rule.label(),
            self.subject,
            self.rule_label_purpose(),
            self.message
        )
    }
}

impl Diagnostic {
    fn rule_label_purpose(&self) -> &'static str {
        match self.rule {
            Rule::AxisLenMismatch => "轴点数与维度不符",
            Rule::ArityMismatch => "轴数与维度数不符",
            Rule::AxisSymbolEqualsValue => "轴符号与值符号相同",
            Rule::Unresolved => "引用无法解析",
            Rule::UnboundArray => "多维数组未绑定",
            Rule::AxisShape => "轴形状不合法",
        }
    }
}

impl Serialize for Diagnostic {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut out = serializer.serialize_struct("Diagnostic", 4)?;
        out.serialize_field("rule", self.rule.label())?;
        out.serialize_field("severity", &self.severity())?;
        out.serialize_field("subject", &self.subject)?;
        out.serialize_field("message", &self.message)?;
        out.end()
    }
}

/// 解析后的轴：长度与元素类型来自数据源。
#[derive(Debug, Clone, PartialEq)]
pub struct Axis {
    id: Id,
    symbol: Symbol,
    address: Address,
    elem: ElemType,
    len: u32,
    kind: AxisKind,
}

impl Axis {
    /// 直接构造已解析的轴。`points == 0` 返回 `None`——零点数的轴不存在。
    ///
    /// 供权威来源（A2L 解析、ELF 加配置）使用；由用户配置驱动的情况走
    /// [`AxisBinding::resolve`]，那条路会跑 R1–R7。
    pub fn new(
        id: Id,
        symbol: Symbol,
        address: Address,
        elem: ElemType,
        points: u32,
        kind: AxisKind,
    ) -> Option<Self> {
        if points == 0 {
            return None;
        }
        Some(Self {
            id,
            symbol,
            address,
            elem,
            len: points,
            kind,
        })
    }

    /// id。
    pub fn id(&self) -> &Id {
        &self.id
    }

    /// 符号。
    pub fn symbol(&self) -> &Symbol {
        &self.symbol
    }

    /// 数据源地址。
    pub fn address(&self) -> Address {
        self.address
    }

    /// 元素类型。
    pub fn elem(&self) -> ElemType {
        self.elem
    }

    /// 轴点数。至少 1 点——零长度的轴在数据源层就不存在，故不提供
    /// `is_empty`，方法名也不叫 `len`。
    pub fn points(&self) -> u32 {
        self.len
    }

    /// 存放方式。
    pub fn kind(&self) -> AxisKind {
        self.kind
    }
}

impl Serialize for Axis {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut out = serializer.serialize_struct("Axis", 5)?;
        out.serialize_field("id", &self.id)?;
        out.serialize_field("symbol", &self.symbol)?;
        out.serialize_field("len", &self.len)?;
        out.serialize_field("kind", &self.kind)?;
        out.serialize_field("elem", &self.elem)?;
        out.end()
    }
}

/// 值的维度形态，由维度数推出而不是另存一个字段。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ValueKind {
    /// 一维，A2L 的 `CURVE`。
    Curve,
    /// 二维，A2L 的 `MAP`。
    Map,
    /// 三维及以上：MVP 不导出 A2L，只按裸数组展示。
    Higher,
}

/// 解析后的值。
#[derive(Debug, Clone, PartialEq)]
pub struct BoundValue {
    id: Id,
    symbol: Symbol,
    address: Address,
    elem: ElemType,
    dims: Vec<u32>,
    axes: Vec<Id>,
}

impl BoundValue {
    /// 直接构造已解析的值。
    ///
    /// 同 [`Axis::new`]：这条口子面向权威来源，不跑 R1–R7。不变量仍在
    /// 这里守住——维度不能为空、不能含 0，轴数必须等于维度数。
    pub fn new(
        id: Id,
        symbol: Symbol,
        address: Address,
        elem: ElemType,
        dims: Vec<u32>,
        axes: Vec<Id>,
    ) -> Option<Self> {
        if dims.is_empty() || dims.contains(&0) || dims.len() != axes.len() {
            return None;
        }
        Some(Self {
            id,
            symbol,
            address,
            elem,
            dims,
            axes,
        })
    }

    /// id。
    pub fn id(&self) -> &Id {
        &self.id
    }

    /// 符号。
    pub fn symbol(&self) -> &Symbol {
        &self.symbol
    }

    /// 数据源地址。
    pub fn address(&self) -> Address {
        self.address
    }

    /// 元素类型。
    pub fn elem(&self) -> ElemType {
        self.elem
    }

    /// 各维长度。
    pub fn dims(&self) -> &[u32] {
        &self.dims
    }

    /// 轴引用，顺序对应 `dims`。
    pub fn axes(&self) -> &[Id] {
        &self.axes
    }

    /// 形态。
    pub fn kind(&self) -> ValueKind {
        match self.dims.len() {
            1 => ValueKind::Curve,
            2 => ValueKind::Map,
            _ => ValueKind::Higher,
        }
    }

    /// 元素总数。
    pub fn elem_count(&self) -> u64 {
        self.dims.iter().map(|&d| u64::from(d)).product()
    }
}

impl Serialize for BoundValue {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut out = serializer.serialize_struct("Value", 6)?;
        out.serialize_field("id", &self.id)?;
        out.serialize_field("symbol", &self.symbol)?;
        out.serialize_field("kind", &self.kind())?;
        out.serialize_field("dims", &self.dims)?;
        out.serialize_field("axes", &self.axes)?;
        out.serialize_field("elem", &self.elem)?;
        out.end()
    }
}

/// 绑定结果：解析后的轴、值与警告。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AxisBinding {
    axes: Vec<Axis>,
    values: Vec<BoundValue>,
    diagnostics: Vec<Diagnostic>,
}

impl AxisBinding {
    /// 由已解析的轴与值直接组装，用于权威来源（A2L 或 ELF 加配置）。
    pub fn from_parts(axes: Vec<Axis>, values: Vec<BoundValue>) -> Self {
        Self {
            axes,
            values,
            diagnostics: Vec::new(),
        }
    }

    /// 轴。
    pub fn axes(&self) -> &[Axis] {
        &self.axes
    }

    /// 值。
    pub fn values(&self) -> &[BoundValue] {
        &self.values
    }

    /// 警告。加载成功时只会是 R5 一类不阻断的问题。
    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }

    /// 按 id 查轴。
    pub fn axis(&self, id: &Id) -> Option<&Axis> {
        self.axes.iter().find(|a| a.id() == id)
    }

    /// 按 id 查值。
    pub fn value(&self, id: &Id) -> Option<&BoundValue> {
        self.values.iter().find(|v| v.id() == id)
    }

    /// 用配置对变量树解析轴与值，跑完 R1–R7。
    ///
    /// 任一条错误规则命中即整体失败，错误一次报全——分次报会让用户逐个
    /// 修配置，来回多轮。
    pub fn resolve(tree: &VariableTree, spec: &BindingSpec) -> Result<Self, BindingError> {
        let mut diagnostics = Vec::new();
        check_unique_ids(spec, &mut diagnostics);
        resolve_axes(tree, spec, &mut diagnostics);
        let axes = collect_axes(tree, spec);
        resolve_values(tree, spec, &axes, &mut diagnostics);
        reject_unbound(tree, spec, &mut diagnostics);

        let (errors, warnings): (Vec<_>, Vec<_>) = diagnostics
            .into_iter()
            .partition(|d| d.severity() == Severity::Error);
        if errors.is_empty() {
            Ok(Self {
                axes,
                values: collect_values(tree, spec),
                diagnostics: warnings,
            })
        } else {
            Err(BindingError { errors })
        }
    }
}

impl Serialize for AxisBinding {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut out = serializer.serialize_struct("AxisBinding", 3)?;
        out.serialize_field("axes", &self.axes)?;
        out.serialize_field("values", &self.values)?;
        out.serialize_field("diagnostics", &self.diagnostics)?;
        out.end()
    }
}

/// 绑定失败，带全部错误诊断。
#[derive(Debug, Clone, PartialEq)]
pub struct BindingError {
    errors: Vec<Diagnostic>,
}

impl BindingError {
    /// 全部错误，按发现顺序。
    pub fn errors(&self) -> &[Diagnostic] {
        &self.errors
    }
}

impl fmt::Display for BindingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "轴绑定配置有 {} 处错误：", self.errors.len())?;
        for (index, error) in self.errors.iter().enumerate() {
            if index > 0 {
                writeln!(f)?;
            }
            write!(f, "  {error}")?;
        }
        Ok(())
    }
}

impl std::error::Error for BindingError {}

/// 解出可用的轴：符号必须存在且是一维数值数组（标量按 1 点处理）。
fn collect_axes(tree: &VariableTree, spec: &BindingSpec) -> Vec<Axis> {
    spec.axes()
        .iter()
        .filter_map(|declared| {
            let variable = tree.get(declared.symbol())?;
            Axis::new(
                declared.id().clone(),
                declared.symbol().clone(),
                variable.address(),
                variable.elem(),
                axis_len(variable),
                declared.kind(),
            )
        })
        .collect()
}

fn axis_len(variable: &Variable) -> u32 {
    variable.dims().first().copied().unwrap_or(1)
}

/// 解出可用的值：符号必须存在且至少一维。
fn collect_values(tree: &VariableTree, spec: &BindingSpec) -> Vec<BoundValue> {
    spec.values()
        .iter()
        .filter_map(|declared| {
            let variable = tree.get(declared.symbol())?;
            BoundValue::new(
                declared.id().clone(),
                declared.symbol().clone(),
                variable.address(),
                variable.elem(),
                variable.dims().to_vec(),
                declared.axes().to_vec(),
            )
        })
        .collect()
}

/// R4：id 在轴与值之间全局唯一。两者共用一个命名空间，因此这一条必须
/// 在同一处检查——分开查就漏掉"轴与值撞名"。
fn check_unique_ids(spec: &BindingSpec, out: &mut Vec<Diagnostic>) {
    let mut seen: Vec<&Id> = Vec::new();
    let ids = spec
        .axes()
        .iter()
        .map(AxisSpec::id)
        .chain(spec.values().iter().map(ValueSpec::id));
    for id in ids {
        if seen.contains(&id) {
            out.push(Diagnostic::new(
                Rule::Unresolved,
                id.as_str(),
                "id 重复；轴与值共用同一命名空间",
            ));
        } else {
            seen.push(id);
        }
    }
}

/// R3 与 R7：轴符号不得等于值符号、符号必须存在、轴必须是一维。
fn resolve_axes(tree: &VariableTree, spec: &BindingSpec, out: &mut Vec<Diagnostic>) {
    for declared in spec.axes() {
        if spec
            .values()
            .iter()
            .any(|value| value.symbol() == declared.symbol())
        {
            out.push(Diagnostic::new(
                Rule::AxisSymbolEqualsValue,
                declared.id().as_str(),
                format!("轴与值指向同一符号 {}", declared.symbol()),
            ));
        }
        match tree.get(declared.symbol()) {
            None => out.push(Diagnostic::new(
                Rule::Unresolved,
                declared.id().as_str(),
                format!("符号 {} 在数据源里不存在", declared.symbol()),
            )),
            Some(variable) if variable.dims().len() > 1 => out.push(Diagnostic::new(
                Rule::AxisShape,
                declared.id().as_str(),
                format!(
                    "符号 {} 是 {} 维数组，轴只能是一维数组或标量",
                    declared.symbol(),
                    variable.dims().len()
                ),
            )),
            Some(_) => {}
        }
    }
}

/// R1、R2、R3、R4：值的引用、维度数与轴点数。
fn resolve_values(
    tree: &VariableTree,
    spec: &BindingSpec,
    axes: &[Axis],
    out: &mut Vec<Diagnostic>,
) {
    for declared in spec.values() {
        let Some(variable) = tree.get(declared.symbol()) else {
            out.push(Diagnostic::new(
                Rule::Unresolved,
                declared.id().as_str(),
                format!("符号 {} 在数据源里不存在", declared.symbol()),
            ));
            continue;
        };
        let dims = variable.dims();
        if dims.is_empty() {
            out.push(Diagnostic::new(
                Rule::ArityMismatch,
                declared.id().as_str(),
                format!("符号 {} 是标量，Curve 与 Map 至少要一维", declared.symbol()),
            ));
            continue;
        }
        if declared.axes().len() != dims.len() {
            out.push(Diagnostic::new(
                Rule::ArityMismatch,
                declared.id().as_str(),
                format!(
                    "声明了 {} 个轴，符号 {} 是 {} 维",
                    declared.axes().len(),
                    declared.symbol(),
                    dims.len()
                ),
            ));
            continue;
        }
        for (index, axis_id) in declared.axes().iter().enumerate() {
            let Some(axis) = axes.iter().find(|axis| axis.id() == axis_id) else {
                out.push(Diagnostic::new(
                    Rule::Unresolved,
                    declared.id().as_str(),
                    format!("引用了未声明的轴 {axis_id}"),
                ));
                continue;
            };
            if axis.symbol() == declared.symbol() {
                out.push(Diagnostic::new(
                    Rule::AxisSymbolEqualsValue,
                    declared.id().as_str(),
                    format!("轴符号与值符号相同：{}", declared.symbol()),
                ));
            }
            if axis.points() != dims[index] {
                out.push(Diagnostic::new(
                    Rule::AxisLenMismatch,
                    declared.id().as_str(),
                    format!(
                        "第 {} 维是 {} 点，轴 {axis_id}（{}）是 {} 点；确认 axes 顺序与 dim 顺序一致",
                        index + 1,
                        dims[index],
                        axis.symbol(),
                        axis.points()
                    ),
                ));
            }
        }
    }
}

/// R5：多维数组没绑定，按裸数组展示。
fn reject_unbound(tree: &VariableTree, spec: &BindingSpec, out: &mut Vec<Diagnostic>) {
    for variable in tree.iter() {
        if variable.dims().len() < 2 {
            continue;
        }
        let bound = spec
            .values()
            .iter()
            .any(|value| value.symbol() == variable.symbol());
        if !bound {
            out.push(Diagnostic::new(
                Rule::UnboundArray,
                variable.symbol().as_str(),
                format!("{} 维数组未绑定轴，按裸数组展示", variable.dims().len()),
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sym(raw: &str) -> Symbol {
        Symbol::new(raw).expect("测试符号合法")
    }

    fn id(raw: &str) -> Id {
        Id::new(raw).expect("测试 id 合法")
    }

    /// RFC 第 8 节的数据源：cal_map[4][3]、cal_axis_x[3]、cal_axis_y[4]。
    fn golden_tree() -> VariableTree {
        let map = crate::ir::Variable::new(sym("cal_map"), Address::new(0x2060), ElemType::F32)
            .with_dims(vec![4, 3])
            .expect("[4][3] 合法");
        let x = crate::ir::Variable::new(sym("cal_axis_x"), Address::new(0x2040), ElemType::F32)
            .with_dims(vec![3])
            .expect("[3] 合法");
        let y = crate::ir::Variable::new(sym("cal_axis_y"), Address::new(0x2030), ElemType::F32)
            .with_dims(vec![4])
            .expect("[4] 合法");
        VariableTree::new(vec![map, x, y]).expect("无重复")
    }

    /// RFC 第 8 节的配置。
    fn golden_spec() -> BindingSpec {
        BindingSpec::new(
            vec![
                AxisSpec::new(id("x_speed"), sym("cal_axis_x")),
                AxisSpec::new(id("y_temp"), sym("cal_axis_y")),
            ],
            vec![ValueSpec::new(
                id("eff_map"),
                sym("cal_map"),
                vec![id("y_temp"), id("x_speed")],
            )],
        )
    }

    fn rules(err: &BindingError) -> Vec<&'static str> {
        err.errors().iter().map(|d| d.rule().label()).collect()
    }

    #[test]
    fn golden_binding_resolves_clean() {
        let binding =
            AxisBinding::resolve(&golden_tree(), &golden_spec()).expect("golden 必须通过");
        assert_eq!(binding.axes().len(), 2);
        assert_eq!(binding.values().len(), 1);
        assert!(binding.diagnostics().is_empty(), "正例不该有诊断");

        let x = binding.axis(&id("x_speed")).expect("有 x_speed");
        assert_eq!(x.points(), 3);
        assert_eq!(x.elem(), ElemType::F32);
        assert_eq!(x.kind(), AxisKind::Shared);
        let y = binding.axis(&id("y_temp")).expect("有 y_temp");
        assert_eq!(y.points(), 4);

        let value = binding.value(&id("eff_map")).expect("有 eff_map");
        assert_eq!(value.dims(), &[4, 3]);
        assert_eq!(value.axes(), &[id("y_temp"), id("x_speed")]);
        assert_eq!(value.kind(), ValueKind::Map);
        assert_eq!(value.elem_count(), 12);
    }

    #[test]
    fn golden_binding_serializes_to_rfc_sample() {
        // RFC 第 8 节的期望 IR 快照（地址不入快照：地址不是约定内容）
        let binding =
            AxisBinding::resolve(&golden_tree(), &golden_spec()).expect("golden 必须通过");
        let json = serde_json::to_value(&binding).expect("可序列化");
        let expected = serde_json::json!({
            "axes": [
                {"id": "x_speed", "symbol": "cal_axis_x", "len": 3, "kind": "shared", "elem": "f32"},
                {"id": "y_temp", "symbol": "cal_axis_y", "len": 4, "kind": "shared", "elem": "f32"}
            ],
            "values": [
                {"id": "eff_map", "symbol": "cal_map", "kind": "map",
                 "dims": [4, 3], "axes": ["y_temp", "x_speed"], "elem": "f32"}
            ],
            "diagnostics": []
        });
        assert_eq!(json, expected);
    }

    #[test]
    fn constructors_reject_impossible_shapes() {
        assert!(
            Axis::new(
                id("a"),
                sym("x"),
                Address::new(0),
                ElemType::U8,
                0,
                AxisKind::Shared
            )
            .is_none(),
            "零点数的轴不存在"
        );
        assert!(
            Axis::new(
                id("a"),
                sym("x"),
                Address::new(0),
                ElemType::U8,
                3,
                AxisKind::Inline
            )
            .is_some()
        );
        let base = || (id("v"), sym("m"), Address::new(0), ElemType::F32);
        let (i, s, a, e) = base();
        assert!(BoundValue::new(i.clone(), s.clone(), a, e, Vec::new(), Vec::new()).is_none());
        assert!(
            BoundValue::new(
                i.clone(),
                s.clone(),
                a,
                e,
                vec![4, 0],
                vec![id("x"), id("y")]
            )
            .is_none()
        );
        assert!(
            BoundValue::new(i.clone(), s.clone(), a, e, vec![4, 3], vec![id("x")]).is_none(),
            "轴数必须等于维度数"
        );
        assert!(BoundValue::new(i, s, a, e, vec![4, 3], vec![id("y"), id("x")]).is_some());
    }

    #[test]
    fn r1_rejects_swapped_axes() {
        // RFC 第 8 节的反例：x(3 点) 去对 dim[0]=4
        let spec = BindingSpec::new(
            vec![
                AxisSpec::new(id("x_speed"), sym("cal_axis_x")),
                AxisSpec::new(id("y_temp"), sym("cal_axis_y")),
            ],
            vec![ValueSpec::new(
                id("eff_map"),
                sym("cal_map"),
                vec![id("x_speed"), id("y_temp")],
            )],
        );
        let err = AxisBinding::resolve(&golden_tree(), &spec).expect_err("顺序写反必须被拒");
        assert_eq!(rules(&err), vec!["R1", "R1"]);
        let text = err.to_string();
        assert!(text.contains("eff_map"), "报告要点名对象：{text}");
        assert!(text.contains("确认 axes 顺序"), "提示要可操作：{text}");
    }

    #[test]
    fn r2_rejects_axis_count_mismatch() {
        let spec = BindingSpec::new(
            vec![AxisSpec::new(id("x_speed"), sym("cal_axis_x"))],
            vec![ValueSpec::new(
                id("eff_map"),
                sym("cal_map"),
                vec![id("x_speed")],
            )],
        );
        let err = AxisBinding::resolve(&golden_tree(), &spec).expect_err("2 维只觉得 1 个轴不行");
        assert_eq!(rules(&err), vec!["R2"]);
    }

    #[test]
    fn r2_rejects_scalar_value() {
        let tree = VariableTree::new(vec![crate::ir::Variable::new(
            sym("scalar"),
            Address::new(0x10),
            ElemType::U8,
        )])
        .expect("无重复");
        let spec = BindingSpec::new(
            Vec::new(),
            vec![ValueSpec::new(id("s"), sym("scalar"), Vec::new())],
        );
        let err = AxisBinding::resolve(&tree, &spec).expect_err("标量不能当 Curve");
        assert_eq!(rules(&err), vec!["R2"]);
    }

    #[test]
    fn r3_rejects_axis_pointing_at_value() {
        let spec = BindingSpec::new(
            vec![
                AxisSpec::new(id("self"), sym("cal_map")),
                AxisSpec::new(id("x_speed"), sym("cal_axis_x")),
            ],
            vec![ValueSpec::new(
                id("eff_map"),
                sym("cal_map"),
                vec![id("self"), id("x_speed")],
            )],
        );
        let err = AxisBinding::resolve(&golden_tree(), &spec).expect_err("轴不能是值自己");
        assert!(rules(&err).contains(&"R3"));
    }

    #[test]
    fn r4_rejects_duplicate_id_across_axis_and_value() {
        let spec = BindingSpec::new(
            vec![AxisSpec::new(id("same"), sym("cal_axis_x"))],
            vec![ValueSpec::new(
                id("same"),
                sym("cal_axis_x"),
                vec![id("same")],
            )],
        );
        let err = AxisBinding::resolve(&golden_tree(), &spec).expect_err("id 重复必须报");
        assert!(rules(&err).contains(&"R4"));
    }

    #[test]
    fn r4_rejects_missing_symbol_and_missing_axis() {
        let spec = BindingSpec::new(
            vec![AxisSpec::new(id("gone"), sym("no_such_axis"))],
            vec![ValueSpec::new(
                id("eff_map"),
                sym("cal_map"),
                vec![id("gone"), id("never_declared")],
            )],
        );
        let err = AxisBinding::resolve(&golden_tree(), &spec).expect_err("引用缺失必须报");
        let text = err.to_string();
        assert!(text.contains("no_such_axis"), "要点名缺失的轴符号：{text}");
        assert!(
            text.contains("never_declared"),
            "要点名缺失的轴引用：{text}"
        );
        assert!(rules(&err).iter().all(|label| *label == "R4"), "{text}");
    }

    #[test]
    fn r5_warns_on_unbound_multidim_array() {
        let mut variables: Vec<crate::ir::Variable> = golden_tree().iter().cloned().collect();
        variables.push(
            crate::ir::Variable::new(sym("lonely"), Address::new(0x3000), ElemType::U16)
                .with_dims(vec![2, 2])
                .expect("合法"),
        );
        let tree = VariableTree::new(variables).expect("无重复");
        let binding = AxisBinding::resolve(&tree, &golden_spec()).expect("未绑定只是警告");
        assert_eq!(binding.diagnostics().len(), 1);
        let diagnostic = &binding.diagnostics()[0];
        assert_eq!(diagnostic.rule().label(), "R5");
        assert_eq!(diagnostic.severity(), Severity::Warning);
        assert_eq!(diagnostic.subject(), "lonely");
    }

    #[test]
    fn r6_shares_one_axis_between_two_values() {
        let mut variables: Vec<crate::ir::Variable> = golden_tree().iter().cloned().collect();
        variables.push(
            crate::ir::Variable::new(sym("limiter"), Address::new(0x2080), ElemType::F32)
                .with_dims(vec![3])
                .expect("合法"),
        );
        let tree = VariableTree::new(variables).expect("无重复");
        let BindingSpec { axes, .. } = golden_spec();
        let spec = BindingSpec::new(
            axes,
            vec![
                ValueSpec::new(
                    id("eff_map"),
                    sym("cal_map"),
                    vec![id("y_temp"), id("x_speed")],
                ),
                ValueSpec::new(id("limiter"), sym("limiter"), vec![id("x_speed")]),
            ],
        );

        let binding = AxisBinding::resolve(&tree, &spec).expect("共享轴合法");
        assert!(binding.diagnostics().is_empty(), "共享轴不该有诊断");
        assert_eq!(binding.values().len(), 2);
        let limiter = binding.value(&id("limiter")).expect("有 limiter");
        assert_eq!(limiter.kind(), ValueKind::Curve);
        assert_eq!(limiter.axes(), &[id("x_speed")]);
    }

    #[test]
    fn r7_rejects_multidim_axis() {
        let spec = BindingSpec::new(
            vec![AxisSpec::new(id("bad_axis"), sym("cal_map"))],
            vec![ValueSpec::new(
                id("eff_map"),
                sym("cal_map"),
                vec![id("bad_axis")],
            )],
        );
        let err = AxisBinding::resolve(&golden_tree(), &spec).expect_err("二维数组不能当轴");
        assert!(rules(&err).contains(&"R7"));
    }

    #[test]
    fn scalar_axis_counts_as_one_point() {
        let tree = VariableTree::new(vec![
            crate::ir::Variable::new(sym("one"), Address::new(0x10), ElemType::U8),
            crate::ir::Variable::new(sym("curve"), Address::new(0x20), ElemType::U8)
                .with_dims(vec![1])
                .expect("合法"),
        ])
        .expect("无重复");
        let spec = BindingSpec::new(
            vec![AxisSpec::new(id("a"), sym("one"))],
            vec![ValueSpec::new(id("c"), sym("curve"), vec![id("a")])],
        );
        let binding = AxisBinding::resolve(&tree, &spec).expect("标量轴 1 点合法");
        assert_eq!(binding.axis(&id("a")).map(Axis::points), Some(1));
    }

    #[test]
    fn errors_are_all_reported_at_once() {
        let spec = BindingSpec::new(
            vec![AxisSpec::new(id("x_speed"), sym("cal_axis_x"))],
            vec![
                ValueSpec::new(id("eff_map"), sym("cal_map"), vec![id("x_speed")]),
                ValueSpec::new(id("ghost"), sym("no_such_value"), vec![id("x_speed")]),
            ],
        );
        let err = AxisBinding::resolve(&golden_tree(), &spec).expect_err("两处错误");
        assert!(err.errors().len() >= 2, "一次报全：{err}");
        assert!(err.to_string().contains("处错误"));
    }

    #[test]
    fn empty_spec_binds_nothing_without_error() {
        let binding =
            AxisBinding::resolve(&golden_tree(), &BindingSpec::default()).expect("空配置合法");
        assert!(binding.axes().is_empty());
        assert!(binding.values().is_empty());
        assert_eq!(binding.diagnostics().len(), 1, "cal_map 未绑定 → 一条 R5");
        assert!(BindingSpec::default().is_empty());
    }

    #[test]
    fn id_rejects_blank_like_symbol() {
        assert!(Id::new("").is_none());
        assert!(Id::new(" x").is_none());
        assert_eq!(
            Id::new("x_speed").map(|i| i.to_string()),
            Some("x_speed".to_owned())
        );
    }
}
