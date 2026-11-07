//! gRPC 契约与消息类型。
//!
//! 本 crate 是 L1 契约档：只有类型与服务签名，不含业务逻辑，也不依赖任何
//! 本项目 crate。它是 CLI、daemon 与 UI 三方唯一的对齐点——同一份 `.proto`
//! 既生成服务端的 trait，也生成客户端的 stub，还生成 CLI `--json` 用的
//! serde 结构（见 `build.rs`）。

#![forbid(unsafe_code)]

/// 契约的消息、服务与客户端/服务端桩。
///
/// 内部模块允许 `missing_docs`：字段文档在 `.proto` 里，生成代码里的字段不
/// 会带上它们，逐条补属于给生成物重复抄写一遍。
pub mod v1 {
    #![allow(missing_docs)]

    tonic::include_proto!("openape.v1");
}

pub use v1::*;
