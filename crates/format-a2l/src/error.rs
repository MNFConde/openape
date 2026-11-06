//! 读入失败。每条都带"下一步做什么"。

use thiserror::Error;

/// A2L 读入失败。
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum Error {
    /// `a2lfile` 解析失败（语法或引用错误，消息里已带行号）。
    #[error("A2L 解析失败：{0}；确认文件完整、未被截断")]
    Parse(String),
    /// 指名的模块不存在。
    #[error(
        "A2L 里没有名为 {wanted} 的模块；可选模块：{}",
        available.join(", ")
    )]
    ModuleNotFound {
        /// 调用方要求的模块名。
        wanted: String,
        /// 文件里实际存在的模块。
        available: Vec<String>,
    },
    /// 文件里有多个模块且未指名。
    #[error(
        "A2L 有多个模块（{}），请用 --module 指定其一",
        available.join(", ")
    )]
    ModuleAmbiguous {
        /// 文件里存在的模块。
        available: Vec<String>,
    },
    /// 符号重复：同一名字既有标定量又有观测量。
    #[error("符号 {symbol} 重复：{first} 与 {second} 同名；这会让读写指到错的对象，请先修 A2L")]
    DuplicateSymbol {
        /// 重复的符号名。
        symbol: String,
        /// 先出现的对象类。
        first: &'static str,
        /// 后出现的对象类。
        second: &'static str,
    },
    /// 该变量导不出 A2L。
    #[error("{symbol} 导不出 A2L：{reason}")]
    NotExportable {
        /// 变量符号。
        symbol: String,
        /// 原因。
        reason: String,
    },
    /// 某个对象无法表达：没有数值类型或维度信息。
    #[error("{kind} {name} 无法表达：{reason}")]
    Unrepresentable {
        /// 对象类（`AXIS_PTS`、`CHARACTERISTIC`、`MEASUREMENT`）。
        kind: &'static str,
        /// 对象名。
        name: String,
        /// 原因。
        reason: String,
    },
}
