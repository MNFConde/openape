//! `openaped` 的可复用部分：服务实现与路由组装。
//!
//! 二进制只做参数解析与启动，把这两块留给库，测试才能不经子进程直接验证
//! grpc-web、静态托管与样本流。

#![forbid(unsafe_code)]

pub mod boot;
pub mod server;
pub mod service;
