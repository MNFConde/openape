//! 输出结构与双模渲染。
//!
//! 结构来自 `openape_proto`：同一份类型既走 gRPC 也走 `--json`，两边不会
//! 各写一遍再漂移（M2-11 之前这里是本地 serde 结构，已下线）。

use openape_diag::Fault;
use openape_proto::{BusCapabilities, ChannelProbe, ConnectInfo, ProtocolVersion, ReadResponse};
use openape_session::ChannelProbe as SessionProbe;
use serde::Serialize;

/// 把会话层的探测结果转成契约类型。
pub fn probe_report(probe: SessionProbe) -> ChannelProbe {
    ChannelProbe {
        channel: probe.channel,
        backend: probe.backend.to_owned(),
        capabilities: Some(BusCapabilities {
            classic_can: probe.capabilities.classic_can(),
            can_fd: probe.capabilities.can_fd(),
            hardware_timestamp: probe.capabilities.hardware_timestamp,
        }),
    }
}

/// 把建链结果转成契约类型。协议版本缺省时留 `None`，不补 0.0。
pub fn connect_info(
    resource: u8,
    comm_mode_basic: u8,
    max_cto: u16,
    max_dto: u16,
    protocol_version: Option<(u8, u8)>,
    transport_version: Option<(u8, u8)>,
) -> ConnectInfo {
    ConnectInfo {
        resource: u32::from(resource),
        comm_mode_basic: u32::from(comm_mode_basic),
        max_cto: u32::from(max_cto),
        max_dto: u32::from(max_dto),
        protocol_version: protocol_version.map(version),
        transport_version: transport_version.map(version),
    }
}

fn version((major, minor): (u8, u8)) -> ProtocolVersion {
    ProtocolVersion {
        major: u32::from(major),
        minor: u32::from(minor),
    }
}

/// 读结果。数据以十六进制串呈现而不是 `bytes`：JSON 里的 base64 没人看得懂。
pub fn read_report(channel: String, address: String, len: u16, data: &[u8]) -> ReadResponse {
    ReadResponse {
        channel,
        address,
        len: u32::from(len),
        data_hex: data
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<Vec<_>>()
            .join(" "),
    }
}

/// 按 `--json` 选结构化或人类可读，两者共用同一份值，不会漂移。
pub fn render<T: Serialize>(
    json: bool,
    value: &T,
    human: impl FnOnce() -> String,
) -> Result<String, Fault> {
    if json {
        serde_json::to_string_pretty(value)
            .map_err(|err| crate::usage_fault(format!("结构化输出失败：{err}")))
    } else {
        Ok(human())
    }
}

/// 信息源探测报告。
///
/// 目前只在 CLI 用：daemon 还没有 source 接口，等有了再上提到 `openape_proto`，
/// 免得先在契约里留一个没人消费的类型。
#[derive(Debug, Clone, Serialize)]
pub struct SourceReport {
    /// 变量清单，按符号名排序。
    pub variables: Vec<SourceVariable>,
    /// 交叉校验的结论（只有同时给了 ELF 与 A2L 时才有）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cross_check: Option<CrossCheckReport>,
    /// 加载期提示。
    pub notes: Vec<String>,
}

/// 一个变量在探测报告里的样子。
#[derive(Debug, Clone, Serialize)]
pub struct SourceVariable {
    /// 符号名。
    pub symbol: String,
    /// 首地址。
    pub address: u32,
    /// 元素类型。
    pub elem: String,
    /// 维度，空表示标量。
    pub dims: Vec<u32>,
    /// 角色。仅 ELF 档没有角色，留给用户指定。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    /// 语义是否缺失。`true` 表示这一条只有裸值，界面要标 `[raw]`。
    pub raw: bool,
}

/// 交叉校验结论。
#[derive(Debug, Clone, Serialize)]
pub struct CrossCheckReport {
    /// 字段不一致的清单，每条一句话。
    pub conflicts: Vec<String>,
    /// 只在 ELF 里出现的符号。
    pub only_in_elf: Vec<String>,
    /// 只在 A2L 里出现的符号。
    pub only_in_a2l: Vec<String>,
}

impl SourceReport {
    /// 人类可读的多行报告。
    pub fn render(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!("变量 {} 条\n", self.variables.len()));
        for variable in &self.variables {
            out.push_str(&format!("  {}\n", variable.render()));
        }
        if let Some(cross) = &self.cross_check {
            out.push_str(&format!("交叉校验：{} 条冲突\n", cross.conflicts.len()));
            for conflict in &cross.conflicts {
                out.push_str(&format!("  {conflict}\n"));
            }
            out.push_str(&format!(
                "仅 ELF 有 {} 条，仅 A2L 有 {} 条\n",
                cross.only_in_elf.len(),
                cross.only_in_a2l.len()
            ));
        }
        for note in &self.notes {
            out.push_str(&format!("注意：{note}\n"));
        }
        out
    }
}

impl SourceVariable {
    /// 单行：`cal_kp @0x00002090 f32 [raw]`。
    ///
    /// `[raw]` 是给用户的信号：这一条没有单位与换算，界面上看到的就是内存
    /// 里那几个字节。缺字段就标出来，不填默认值假装知道。
    pub fn render(&self) -> String {
        let dims = if self.dims.is_empty() {
            String::new()
        } else {
            let parts: Vec<String> = self.dims.iter().map(u32::to_string).collect();
            format!("[{}] ", parts.join("]["))
        };
        let role = match &self.role {
            Some(role) => role.as_str(),
            None => "未指定",
        };
        let raw = if self.raw { " [raw]" } else { "" };
        format!(
            "{} @{:#010X} {}{dims} 角色={role}{raw}",
            self.symbol, self.address, self.elem
        )
    }
}
