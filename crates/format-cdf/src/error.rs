//! 读入失败。每条都带行号与"下一步做什么"。

use thiserror::Error;

/// CDF 读入失败。
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum Error {
    /// 文件读不了。
    #[error("读不了 CDF 文件 {path}：{detail}；确认路径与读权限")]
    Io {
        /// 出问题的路径。
        path: String,
        /// 底层原因。
        detail: String,
    },

    /// XML 本身不合法（不是 CDF 结构问题，而是连标签都没配平）。
    #[error(
        "第 {line} 行 XML 解析失败：{detail}；确认文件是工具完整导出的 CDF，未被截断也未被手工改坏"
    )]
    Xml {
        /// 出错行号（1 起）。
        line: usize,
        /// 解析器给的说明。
        detail: String,
    },

    /// 根元素不是 `MSRSW`。
    #[error(
        "这不是 CDF：根元素是 <{root}>，应为 <MSRSW>（CDF 2.0.0 schema）；\
         确认导出的是标定数据（CDF）而不是描述文件（A2L/MCD-2 MC）或测量文件（MDF）"
    )]
    NotCdf {
        /// 实际读到的根元素名。
        root: String,
    },

    /// 必需子元素缺失。
    #[error("第 {line} 行 <{parent}> 缺少 <{element}>：{why}")]
    MissingElement {
        /// 缺子元素的容器名。
        parent: String,
        /// 缺的元素名。
        element: &'static str,
        /// 行号（1 起）。
        line: usize,
        /// 这个元素为什么必需。
        why: &'static str,
    },

    /// 出现 schema 不允许的子元素。
    #[error(
        "第 {line} 行 <{parent}> 里出现 <{element}>：CDF 2.0.0 的 {parent} 只允许 {allowed}；\
         若这是 CDF 2.1.0 的新元素，请提供一份样本以便补支持"
    )]
    UnexpectedElement {
        /// 容器名。
        parent: String,
        /// 不允许的子元素名。
        element: String,
        /// 行号（1 起）。
        line: usize,
        /// 容器允许的子元素（逗号分隔）。
        allowed: String,
    },

    /// `CATEGORY` 不在 schema 的枚举里。
    #[error(
        "第 {line} 行 {path} 的 CATEGORY 是 {value}：不在 CDF 2.0.0 的取值里；\
         检查是否拼错，或该文件来自更新的版本"
    )]
    BadCategory {
        /// 标定量路径。
        path: String,
        /// 读到的值。
        value: String,
        /// 行号（1 起）。
        line: usize,
    },

    /// 数组维度不是正整数。
    #[error("第 {line} 行 {path} 的 SW-ARRAYSIZE 取值 {value} 无效：数组维度必须是正整数")]
    BadArraySize {
        /// 标定量路径。
        path: String,
        /// 读到的值。
        value: String,
        /// 行号（1 起）。
        line: usize,
    },

    /// 文件结构合法但一条标定量都没有。
    #[error(
        "CDF 里没有可导入的标定量：{detail}；\
         2.0.0 的取值在 SW-SYSTEM/SW-INSTANCE-SPEC/SW-INSTANCE-TREE/SW-INSTANCE 下，\
         确认导出工具写了 SW-INSTANCE-TREE"
    )]
    Empty {
        /// 缺在哪一层。
        detail: String,
    },
}
