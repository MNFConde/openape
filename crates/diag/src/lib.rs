//! 维测日志、故障与指标的统一出口。
//!
//! 所有 crate 都依赖本 crate，因此它不得依赖任何业务 crate——见
//! `docs/11-architecture-layers.md` 的 L0 硬约束。

#![forbid(unsafe_code)]

pub mod fault;
pub mod log;

pub use fault::{Domain, Fault, FaultCode, Severity};
pub use log::{Direction, Verbosity};
