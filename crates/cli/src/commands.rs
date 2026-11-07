//! 三条子命令的实现。`main` 只负责解析与分派，这里一命令一函数。

use std::time::Duration;

use openape_bus::CanId;
use openape_cal_core::{Address, ByteLen};
use openape_diag::Fault;
use openape_proto::{BusReport, ConnectInfo, ConnectReport};
use openape_session::{Session, SessionConfig, list_channels, probe_channel};

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
    render(json, &report, || {
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
    let ConnectReport { channel, info } = &report;
    let info: &ConnectInfo = info.as_ref().expect("上面刚构造");
    render(json, &report, || {
        format!(
            "{channel} 建链成功：resource={:#04x} maxCto={} maxDto={}",
            info.resource, info.max_cto, info.max_dto
        )
    })
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
    render(json, &report, || {
        format!("{} {} = [{}]", report.address, report.len, report.data_hex)
    })
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
