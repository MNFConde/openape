//! 输出结构与双模渲染。
//!
//! 结构来自 `openape_proto`：同一份类型既走 gRPC 也走 `--json`，两边不会
//! 各写一遍再漂移（M2-11 之前这里是本地 serde 结构，已下线）。

use openape_diag::Fault;
use openape_proto::{BusCapabilities, ChannelProbe, ConnectInfo, ProtocolVersion, ReadResponse};
use openape_session::ChannelProbe as SessionProbe;
use serde::Serialize;

/// 把会话层的探测结果转成契约类型。
pub(crate) fn probe_report(probe: SessionProbe) -> ChannelProbe {
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
pub(crate) fn connect_info(
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
pub(crate) fn read_report(channel: String, address: String, len: u16, data: &[u8]) -> ReadResponse {
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
pub(crate) fn render<T: Serialize>(
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
