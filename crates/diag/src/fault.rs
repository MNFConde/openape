//! 统一故障类型：稳定编号、严重度、上下文、可操作建议与错误链。

use std::collections::BTreeMap;
use std::fmt;

use serde::Serialize;

/// 故障所属域。编号与 CLI 退出码都按域分段，段之间不共享号码。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Domain {
    /// 总线与硬件链路。
    Bus,
    /// XCP 协议与会话。
    Xcp,
    /// 变量信息来源（ELF、A2L、DBC）。
    Source,
    /// 编排层：装配、设备锁、录制。
    Session,
    /// 本软件内部缺陷或未分类错误。
    Internal,
}

impl Domain {
    /// 域段表：编号段基数与退出码段基数。
    ///
    /// 两者在同一条臂里给出——它们是同一个决定（这个域占哪一段），分成两个
    /// match 迟早改一处漏一处，出现"编号 2011 配退出码 30"这种错配。
    const fn bases(self) -> (u32, u8) {
        match self {
            Self::Bus => (1_000, 10),
            Self::Xcp => (2_000, 20),
            Self::Source => (3_000, 30),
            Self::Session => (4_000, 40),
            Self::Internal => (9_000, 90),
        }
    }

    /// 编号段基数：域内序号从 1 到 999。
    pub const fn code_base(self) -> u32 {
        self.bases().0
    }

    /// CLI 退出码段基数。与编号段一一对应，便于脚本按类别分流。
    pub const fn exit_base(self) -> u8 {
        self.bases().1
    }
}

/// 故障编号：`域段 + 域内序号`，例如 `2011` 是 XCP 域第 11 号。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct FaultCode(u32);

impl FaultCode {
    /// 域内序号上限（含）。超过即说明编号规划失控。
    pub const MAX_SEQ: u32 = 999;

    /// 构造编号。序号 0 或超过 [`Self::MAX_SEQ`] 时返回 `None`——
    /// 非法编号在这里就造不出来。
    pub const fn new(domain: Domain, seq: u32) -> Option<Self> {
        if seq == 0 || seq > Self::MAX_SEQ {
            return None;
        }
        Some(Self(domain.code_base() + seq))
    }

    /// 常量序号的构造：序号越界在**编译期**就失败。
    ///
    /// 只在序号是字面量或本仓的常量表时用——它把"我知道这个序号合法"从注释
    /// 变成编译器检查，调用点因此不必再写一次 expect。运行期算出来的序号走
    /// [`Self::new`]，那里 `None` 是真有可能发生的分支，要按错误处理。
    ///
    /// 编译期求值失败的消息形如 ``evaluation of `X` failed``，根因就是这里
    /// 的 panic 文案。
    pub const fn from_const(domain: Domain, seq: u32) -> Self {
        match Self::new(domain, seq) {
            Some(code) => code,
            None => panic!("故障序号必须在 1..=999 内"),
        }
    }

    /// 原始编号。
    pub const fn get(self) -> u32 {
        self.0
    }

    /// 反解所属域。
    pub const fn domain(self) -> Domain {
        match self.0 / 1_000 {
            1 => Domain::Bus,
            2 => Domain::Xcp,
            3 => Domain::Source,
            4 => Domain::Session,
            _ => Domain::Internal,
        }
    }
}

impl fmt::Display for FaultCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// 严重度。只有 `Error` 与 `Fatal` 会让 CLI 以非零码退出。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    /// 可继续运行的信息级异常。
    Info,
    /// 降级但可继续：能力缺失、退让路径。
    Warning,
    /// 当前操作失败，进程仍可服务其它请求。
    Error,
    /// 会话不可继续。
    Fatal,
}

/// 一条故障。字段全部只读，构造走 builder，避免半成品状态。
#[derive(Debug, Clone, Serialize)]
pub struct Fault {
    code: FaultCode,
    severity: Severity,
    message: String,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    context: BTreeMap<&'static str, String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    hint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cause: Option<Box<Fault>>,
}

impl Fault {
    /// 新建一条故障。`message` 描述"发生了什么"，`hint` 描述"怎么办"。
    pub fn new(code: FaultCode, severity: Severity, message: impl Into<String>) -> Self {
        Self {
            code,
            severity,
            message: message.into(),
            context: BTreeMap::new(),
            hint: None,
            cause: None,
        }
    }

    /// 附加上下文字段（后端名、通道、地址之类）。
    pub fn with_context(mut self, key: &'static str, value: impl Into<String>) -> Self {
        self.context.insert(key, value.into());
        self
    }

    /// 附加可操作建议。上层可直接展示，无需自己拼话术。
    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    /// 挂上更低层的故障，形成错误链。
    pub fn with_cause(mut self, cause: Fault) -> Self {
        self.cause = Some(Box::new(cause));
        self
    }

    /// 参数顺序友好的构造别名，便于 `Fault::bus(1001, Severity::Error, ..)` 之类调用。
    ///
    /// 返回 `Option`：编号非法时为 `None`。
    pub fn build(
        domain: Domain,
        seq: u32,
        severity: Severity,
        message: impl Into<String>,
    ) -> Option<Self> {
        Some(Self::new(FaultCode::new(domain, seq)?, severity, message))
    }

    /// 故障编号。
    pub const fn code(&self) -> FaultCode {
        self.code
    }

    /// 严重度。
    pub const fn severity(&self) -> Severity {
        self.severity
    }

    /// 人类可读的"发生了什么"。
    pub fn message(&self) -> &str {
        &self.message
    }

    /// 可操作建议。
    pub fn hint(&self) -> Option<&str> {
        self.hint.as_deref()
    }

    /// CLI 退出码。按域分段，未分类落到 1。
    pub const fn exit_code(&self) -> u8 {
        self.code.domain().exit_base()
    }

    /// 错误链，最外层在前。
    pub fn chain(&self) -> impl Iterator<Item = &Fault> {
        std::iter::successors(Some(self), |f| f.cause.as_deref())
    }
}

impl fmt::Display for Fault {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{}] {}", self.code, self.message)?;
        if let Some(hint) = &self.hint {
            write!(f, "（{hint}）")?;
        }
        Ok(())
    }
}

impl std::error::Error for Fault {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.cause
            .as_deref()
            .map(|c| c as &(dyn std::error::Error + 'static))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_rejects_zero_and_overflow() {
        assert!(FaultCode::new(Domain::Bus, 0).is_none());
        assert!(FaultCode::new(Domain::Bus, FaultCode::MAX_SEQ + 1).is_none());
        assert!(FaultCode::new(Domain::Bus, 1).is_some());
    }

    #[test]
    fn from_const_matches_new_for_legal_seq() {
        for domain in [
            Domain::Bus,
            Domain::Xcp,
            Domain::Source,
            Domain::Session,
            Domain::Internal,
        ] {
            let checked = FaultCode::new(domain, 7).expect("序号 7 合法");
            assert_eq!(FaultCode::from_const(domain, 7), checked);
        }
    }

    #[test]
    fn from_const_is_usable_in_const_context() {
        // 常量求值即校验：序号越界在这里直接编译不过。
        const CODE: FaultCode = FaultCode::from_const(Domain::Xcp, 11);
        assert_eq!(CODE.get(), 2011);
        assert_eq!(CODE.domain(), Domain::Xcp);
    }

    #[test]
    #[should_panic(expected = "1..=999")]
    fn from_const_rejects_out_of_range_at_runtime() {
        let _ = FaultCode::from_const(Domain::Bus, 0);
    }

    #[test]
    fn code_domain_round_trips() {
        for domain in [
            Domain::Bus,
            Domain::Xcp,
            Domain::Source,
            Domain::Session,
            Domain::Internal,
        ] {
            let code = FaultCode::new(domain, 7).expect("序号 7 合法");
            assert_eq!(code.domain(), domain);
            assert_eq!(code.get(), domain.code_base() + 7);
        }
    }

    #[test]
    fn exit_codes_align_with_domain_and_do_not_overlap() {
        let domains = [
            Domain::Bus,
            Domain::Xcp,
            Domain::Source,
            Domain::Session,
            Domain::Internal,
        ];
        let bases: Vec<u8> = domains.iter().map(|d| d.exit_base()).collect();
        // 相邻段的距离大于域内可能的退出码跨度（本设计每域一个退出码）。
        for pair in bases.windows(2) {
            assert!(pair[0] < pair[1], "退出码段必须严格递增");
        }
        let fault = Fault::build(Domain::Xcp, 11, Severity::Error, "从站无应答").expect("编号合法");
        assert_eq!(fault.exit_code(), Domain::Xcp.exit_base());
    }

    #[test]
    fn chain_walks_from_outermost_to_cause() {
        let inner = Fault::build(Domain::Bus, 1, Severity::Error, "socket 已关闭").expect("合法");
        let outer = Fault::build(Domain::Xcp, 11, Severity::Fatal, "连接失败")
            .expect("合法")
            .with_cause(inner);
        let messages: Vec<&str> = outer.chain().map(|f| f.message()).collect();
        assert_eq!(messages, ["连接失败", "socket 已关闭"]);
    }

    #[test]
    fn display_shows_code_message_and_hint() {
        let plain = Fault::build(Domain::Bus, 3, Severity::Error, "打不开通道").expect("合法");
        assert_eq!(
            plain.to_string(),
            "[1003] 打不开通道",
            "无建议时不该多出括号"
        );

        let hinted = plain.with_hint("先确认通道存在");
        assert_eq!(
            hinted.to_string(),
            "[1003] 打不开通道（先确认通道存在）",
            "有建议时要跟在同一行"
        );
    }

    #[test]
    fn context_and_hint_survive_serialization() {
        let fault = Fault::build(Domain::Bus, 3, Severity::Error, "打不开通道")
            .expect("合法")
            .with_context("channel", "vcan0")
            .with_hint("先确认 ip link 里有该通道");
        let json = serde_json::to_value(&fault).expect("可序列化");
        assert_eq!(json["code"], 1003);
        assert_eq!(json["severity"], "error");
        assert_eq!(json["context"]["channel"], "vcan0");
        assert_eq!(json["hint"], "先确认 ip link 里有该通道");
    }
}
