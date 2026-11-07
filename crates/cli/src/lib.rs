//! CLI 的可复用部分：子命令实现与输出渲染。
//!
//! 二进制只管解析与退出码；逻辑留在这里，测试才能直接调用它们——经子进程
//! 跑一遍看不到覆盖率，出了问题只能靠猜。

#![forbid(unsafe_code)]

pub mod commands;
pub mod output;

use openape_diag::{Fault, FaultCode, Severity};

/// 命令行用法错误。归内部域：这不是 ECU 或总线的问题。
pub fn usage_fault(message: String) -> Fault {
    Fault::new(
        FaultCode::new(openape_diag::Domain::Internal, 1).expect("序号 1 合法"),
        Severity::Error,
        message,
    )
    .with_hint("用 `openape <命令> --help` 查看参数")
}
