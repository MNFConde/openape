//! 读入期提示：不阻断加载，但用户该知道的事。
//!
//! 与错误分开是刻意的——真实 ECU 的 A2L 常带几百条警告，把它们当错误会
//! 让软件对着一份能用的文件罢工；但静默丢掉又会让用户以为语义完整。

use serde::Serialize;

/// 提示的严重度。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    /// 信息：读到了但做了降级处理。
    Info,
    /// 警告：语义或对象被跳过，结果能用但不完整。
    Warning,
}

/// 一条读入期提示。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Notice {
    severity: Severity,
    /// 来源：`a2lfile` 或具体对象名。
    subject: String,
    message: String,
}

impl Notice {
    /// 信息级提示。
    pub fn info(subject: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Info,
            subject: subject.into(),
            message: message.into(),
        }
    }

    /// 警告级提示。
    pub fn warning(subject: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Warning,
            subject: subject.into(),
            message: message.into(),
        }
    }

    /// 严重度。
    pub fn severity(&self) -> Severity {
        self.severity
    }

    /// 来源。
    pub fn subject(&self) -> &str {
        &self.subject
    }

    /// 说明。
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl std::fmt::Display for Notice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} [{}] {}",
            match self.severity {
                Severity::Info => "信息",
                Severity::Warning => "警告",
            },
            self.subject,
            self.message
        )
    }
}
