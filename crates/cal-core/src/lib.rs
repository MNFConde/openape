//! 跨 crate 共享的基础类型。
//!
//! 地址、长度、句柄三类 newtype 是全局单一源：各 crate 不得自定义等价
//! 类型（见 `docs/11-architecture-layers.md` 的接口一致性节）。

#![forbid(unsafe_code)]

mod types;

pub use types::{Address, ByteLen, Handle, ZeroLen};
