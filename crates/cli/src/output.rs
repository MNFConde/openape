//! 输出结构与双模渲染。

use openape_diag::Fault;
use openape_session::ChannelProbe;
use serde::Serialize;

/// 结构源暂时是本文件的 serde 结构：M1 只有 CLI 一个消费者，M2-11 建 proto
/// 之后由生成的类型取代，届时本文件下线。
#[derive(Debug, Serialize)]
pub(crate) struct BusReport {
    pub(crate) channels: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) probed: Option<ProbeReport>,
}

/// 通道探测结果。
#[derive(Debug, Serialize)]
pub(crate) struct ProbeReport {
    pub(crate) channel: String,
    pub(crate) backend: String,
    pub(crate) classic_can: bool,
    pub(crate) can_fd: bool,
    pub(crate) hardware_timestamp: bool,
}

impl From<ChannelProbe> for ProbeReport {
    fn from(probe: ChannelProbe) -> Self {
        Self {
            channel: probe.channel,
            backend: probe.backend.to_owned(),
            classic_can: probe.capabilities.classic_can(),
            can_fd: probe.capabilities.can_fd(),
            hardware_timestamp: probe.capabilities.hardware_timestamp,
        }
    }
}

/// 建链结果。
#[derive(Debug, Serialize)]
pub(crate) struct ConnectReport {
    pub(crate) channel: String,
    pub(crate) resource: u8,
    pub(crate) max_cto: u16,
    pub(crate) max_dto: u16,
    pub(crate) protocol_version: Option<String>,
}

/// 读结果。
#[derive(Debug, Serialize)]
pub(crate) struct ReadReport {
    pub(crate) channel: String,
    pub(crate) address: String,
    pub(crate) len: u16,
    pub(crate) data_hex: String,
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
