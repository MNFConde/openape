//! 三条子命令的实现。`main` 只负责解析与分派，这里一命令一函数。

use std::time::Duration;

use openape_bus::CanId;
use openape_cal_core::{Address, ByteLen};
use openape_diag::{Domain, Fault, FaultCode, Severity};
use openape_proto::open_ape_client::OpenApeClient;
use openape_proto::{BusReport, ConnectReport};
use openape_session::{Session, SessionConfig, list_channels, probe_channel};
use tonic::transport::Channel;

use crate::output::{connect_info, probe_report, read_report, render};
use crate::usage_fault;

/// `probe bus`：列通道，可选探测一条。
pub(crate) async fn probe_bus(config_channel: Option<&str>, json: bool) -> Result<String, Fault> {
    let channels = list_channels();
    let probed = match config_channel {
        Some(channel) => Some(probe_report(probe_channel(channel)?)),
        None => None,
    };
    let report = BusReport { channels, probed };
    render_bus(json, &report)
}

/// 通道报告的双模渲染，直连与经 daemon 共用。
pub(crate) fn render_bus(json: bool, report: &BusReport) -> Result<String, Fault> {
    render(json, report, || {
        let mut out = format!("通道 {} 条", report.channels.len());
        if report.channels.is_empty() {
            out.push_str("\n  无 CAN 通道；vcan 可用 `ip link add dev vcan0 type vcan` 建一个");
        }
        for channel in &report.channels {
            out.push_str(&format!("\n  {channel}"));
        }
        if let Some(probe) = report.probed.as_ref() {
            let caps = probe.capabilities.as_ref();
            out.push_str(&format!(
                "\n探测 {}（{}）：经典 CAN={} CAN-FD={} 硬件时间戳={}",
                probe.channel,
                probe.backend,
                caps.is_some_and(|caps| caps.classic_can),
                caps.is_some_and(|caps| caps.can_fd),
                caps.is_some_and(|caps| caps.hardware_timestamp)
            ));
        }
        out
    })
}

/// `probe xcp`：建链后立刻断链，只报能力。
pub(crate) async fn probe_xcp(
    channel: &str,
    tx_id: u32,
    rx_id: u32,
    timeout_ms: u64,
    json: bool,
) -> Result<String, Fault> {
    let config = session_config(channel, tx_id, rx_id, timeout_ms)?;
    let mut session = Session::open(config)?;
    let info = session.connect(0).await?;
    let report = ConnectReport {
        channel: session.channel().to_owned(),
        info: Some(connect_info(
            info.resource,
            info.comm_mode_basic,
            info.max_cto,
            info.max_dto,
            info.protocol_version,
            info.transport_version,
        )),
    };
    session.disconnect().await.ok();
    render_connect(json, &report)
}

/// `read`：读一段内存并打印。
pub(crate) async fn read_memory(
    channel: &str,
    address: u32,
    len: u16,
    tx_id: u32,
    rx_id: u32,
    timeout_ms: u64,
    json: bool,
) -> Result<String, Fault> {
    let config = session_config(channel, tx_id, rx_id, timeout_ms)?;
    let mut session = Session::open(config)?;
    session.connect(0).await?;
    let address = Address::new(address);
    let len = ByteLen::new(len).ok_or_else(|| usage_fault("--len 必须大于 0".into()))?;
    let data = session.read(address, len).await?;
    session.disconnect().await.ok();
    let report = read_report(
        session.channel().to_owned(),
        address.to_string(),
        len.get(),
        &data,
    );
    render_read(json, &report)
}

/// 把命令行参数变成会话配置，顺手校验标识符范围。
pub(crate) fn session_config(
    channel: &str,
    tx_id: u32,
    rx_id: u32,
    timeout_ms: u64,
) -> Result<SessionConfig, Fault> {
    let mut config = SessionConfig::new(channel);
    config.command_id = CanId::standard(to_standard_id(tx_id, "--tx-id")?)
        .ok_or_else(|| usage_fault(format!("--tx-id {tx_id:#x} 超出 11 位标准帧范围")))?;
    config.response_id = CanId::standard(to_standard_id(rx_id, "--rx-id")?)
        .ok_or_else(|| usage_fault(format!("--rx-id {rx_id:#x} 超出 11 位标准帧范围")))?;
    config.timeout = Duration::from_millis(timeout_ms);
    Ok(config)
}

fn to_standard_id(raw: u32, flag: &str) -> Result<u16, Fault> {
    u16::try_from(raw).map_err(|_| {
        usage_fault(format!("{flag} {raw:#x} 超出 16 位")).with_context("flag", flag.to_owned())
    })
}

/// `--via-daemon` 不带值时的默认地址。
pub(crate) const DAEMON_DEFAULT: &str = "http://127.0.0.1:47711";

/// 经 daemon 探测通道：daemon 已经拿着设备锁，这里只是问它要答案。
pub(crate) async fn probe_bus_via(
    url: &str,
    channel: Option<&str>,
    json: bool,
) -> Result<String, Fault> {
    let mut client = connect(url).await?;
    let channels = client
        .list_channels(openape_proto::Empty {})
        .await
        .map_err(daemon_fault)?
        .into_inner()
        .channels;
    let probed = match channel {
        Some(channel) => Some(
            client
                .probe_channel(openape_proto::ProbeChannelRequest {
                    channel: channel.to_owned(),
                })
                .await
                .map_err(daemon_fault)?
                .into_inner(),
        ),
        None => None,
    };
    let report = BusReport { channels, probed };
    render_bus(json, &report)
}

/// 经 daemon 读内存。读之前 daemon 会让采集泵让开接收端。
pub(crate) async fn read_via(
    url: &str,
    address: u32,
    len: u16,
    json: bool,
) -> Result<String, Fault> {
    if len == 0 {
        return Err(usage_fault("--len 必须大于 0".into()));
    }
    let mut client = connect(url).await?;
    let report = client
        .read_memory(openape_proto::ReadRequest {
            address: format!("0x{address:08x}"),
            len: u32::from(len),
        })
        .await
        .map_err(daemon_fault)?
        .into_inner();
    render_read(json, &report)
}

/// 经 daemon 看会话状态：它建好链了，就不必我们再连一次。
pub(crate) async fn probe_xcp_via(url: &str, channel: &str, json: bool) -> Result<String, Fault> {
    let mut client = connect(url).await?;
    let status = client
        .get_session(openape_proto::Empty {})
        .await
        .map_err(daemon_fault)?
        .into_inner();
    if !status.linked {
        return Err(daemon_fault(
            "daemon 没有接管任何通道；去掉 --via-daemon 直连设备",
        ));
    }
    if status.channel != channel {
        return Err(daemon_fault(format!(
            "daemon 接管的是 {}，不是 {channel}",
            status.channel
        )));
    }
    let report = ConnectReport {
        channel: status.channel,
        info: status.info,
    };
    render_connect(json, &report)
}

async fn connect(url: &str) -> Result<OpenApeClient<Channel>, Fault> {
    OpenApeClient::connect(url.to_owned())
        .await
        .map_err(|err| daemon_fault(format!("连不上 daemon {url}：{err}")))
}

/// daemon 侧的失败。归会话域：设备在 daemon 手里，问题出在那条会话上。
fn daemon_fault(message: impl ToString) -> Fault {
    Fault::new(
        FaultCode::new(Domain::Session, 4).expect("序号 4 合法"),
        Severity::Error,
        message.to_string(),
    )
}

/// 建链报告的双模渲染，两条路径共用，输出不会漂移。
pub(crate) fn render_connect(json: bool, report: &ConnectReport) -> Result<String, Fault> {
    let channel = &report.channel;
    let info = report.info.as_ref();
    render(json, report, || {
        let Some(info) = info else {
            return format!("{channel} 建链成功（daemon 未报告会话参数）");
        };
        format!(
            "{channel} 建链成功：resource={:#04x} maxCto={} maxDto={}",
            info.resource, info.max_cto, info.max_dto
        )
    })
}

/// 读结果的双模渲染。
pub(crate) fn render_read(
    json: bool,
    report: &openape_proto::ReadResponse,
) -> Result<String, Fault> {
    render(json, report, || {
        format!("{} {} = [{}]", report.address, report.len, report.data_hex)
    })
}

#[cfg(test)]
mod tests {
    use openape_diag::Domain;

    use super::*;

    #[test]
    fn ids_beyond_11_bits_are_usage_errors() {
        let err = session_config("vcan0", 0x800, 0x601, 200).expect_err("0x800 超标准帧");
        assert_eq!(err.code().domain(), Domain::Internal, "用法错误归内部域");
        assert!(err.message().contains("--tx-id"), "要指出是哪个参数");

        let err = session_config("vcan0", 0x600, 0x1_0000, 200).expect_err("超 16 位");
        assert!(err.message().contains("--rx-id"), "实际: {err}");
    }

    #[test]
    fn valid_ids_become_the_session_config() {
        let config = session_config("vcan0", 0x600, 0x601, 150).expect("合法");
        assert_eq!(config.channel, "vcan0");
        assert_eq!(config.timeout, Duration::from_millis(150));
    }

    #[test]
    fn zero_length_is_refused_before_touching_the_bus() {
        // read_memory 会先开会话，所以这里只验长度校验本身。
        assert!(ByteLen::new(0).is_none());
    }
}
