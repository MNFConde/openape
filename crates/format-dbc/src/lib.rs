//! DBC 信息源：解析 CAN 数据库，并做信号 pack/unpack。
//!
//! 自研而不复用 C 系解析器（[D-12] 的路线）：本项目要的只是"总线帧 → 物理
//! 值"这一段，而 DBC 的语法里绝大多数内容（属性、注释、变体、环境变量、
//! 多路复用嵌套）与这一段无关。读进来不认识的行记成提示跳过，不因为一条
//! `BA_DEF_` 没实现就让整份数据库读不了。
//!
//! DBC 没有公开规范，各工具的边角行为并不统一。本 crate 的口径是**按
//! Vector 工具链的通行行为**实现，与 `can-dbc` 逐条对照，差异在测试里逐条
//! 列举并注明采纳哪边——拿另一个实现当标准会把无关边角卡成阻塞。
//!
//! [D-12]: ../../docs/decisions.md

#![forbid(unsafe_code)]

mod parse;

pub use parse::{Database, Message, Signal, load_str};

/// 字节序。DBC 里 `@1` 是 Intel（小端），`@0` 是 Motorola（大端）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ByteOrder {
    /// 小端，位编号从低到高连续。
    Intel,
    /// 大端，位编号按 Motorola 的锯齿序排列。
    Motorola,
}

/// 解析或取值失败。
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// 整份文本里一条 `BO_` 都没有——多半是给错了文件。
    #[error("这份 DBC 里没有任何消息定义；确认文件选对了")]
    NoMessages,
    /// 帧长度对不上。
    #[error("帧长度应为 {expected} 字节，实际 {actual} 字节")]
    FrameLength {
        /// 消息定义的长度。
        expected: usize,
        /// 调用方给的长度。
        actual: usize,
    },
    /// 物理值超出信号定义的范围。
    #[error("{signal} 的值 {value} 超出量程 [{min}, {max}]")]
    OutOfRange {
        /// 信号名。
        signal: String,
        /// 给定值。
        value: f64,
        /// 下限。
        min: f64,
        /// 上限。
        max: f64,
    },
}
