//! 跨 crate 共享的基础类型与变量 IR。
//!
//! 地址、长度、句柄三类 newtype 是全局单一源：各 crate 不得自定义等价
//! 类型（见 `docs/11-architecture-layers.md` 的接口一致性节）。
//!
//! `ir` 模块是 ELF 与 A2L 两个前端共同的出口模型：两份输入都有的字段
//! 必填，只有 A2L 才有的语义字段一律可空（[D-13]）。
//!
//! [D-13]: ../../docs/decisions.md

#![forbid(unsafe_code)]

mod config;
mod ir;
mod types;

pub use config::{ConfigError, ProjectConfig, SCHEMA_VERSION};

pub use ir::{
    Axis, AxisBinding, AxisKind, AxisSpec, BadId, BadRange, BadSymbol, BindingError, BindingSpec,
    BoundValue, Diagnostic, DuplicateSymbol, ElemType, EmptyUnit, Id, Range, Role, Rule, Scale,
    Semantics, Severity, Symbol, Unit, ValueKind, ValueSpec, Variable, VariableTree,
};
pub use types::{Address, ByteLen, Handle, ZeroLen};
