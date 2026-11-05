//! 单次探测外壳。
//!
//! 一条命令即一次自足事务：拿设备锁、建链、做完、退出。结构化输出（`--json`）
//! 供外部脚本与 AI 代理编排，退出码按故障域分段（D-18）。

#![forbid(unsafe_code)]

use std::process::ExitCode;
use std::time::Duration;

use clap::{Parser, Subcommand};
use openape_cal_core::{Address, ByteLen};
use openape_diag::{Fault, Verbosity, log};
use openape_session::{ChannelProbe, Session, SessionConfig, list_channels, probe_channel};
use serde::Serialize;

#[derive(Debug, Parser)]
#[command(name = "openape", version, about = "XCP 标定与观测工具")]
struct Cli {
    /// 结构化输出，字段与将来的 gRPC 契约对齐。
    #[arg(long, global = true)]
    json: bool,
    /// 输出 DEBUG 级日志。
    #[arg(long, global = true)]
    verbose: bool,
    /// 记录每一帧（收发方向与字节）。
    #[arg(long, global = true)]
    frame_trace: bool,
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// 探测硬件与协议
    Probe {
        #[command(subcommand)]
        target: ProbeTarget,
    },
    /// 读一段 ECU 内存
    Read(ReadArgs),
}

#[derive(Debug, Subcommand)]
enum ProbeTarget {
    /// 列出或探测 CAN 通道
    Bus(BusArgs),
    /// 探测 XCP 建链
    Xcp(XcpArgs),
}

#[derive(Debug, Parser)]
struct BusArgs {
    /// 要探测的通道；省略则只列本机通道
    #[arg(long)]
    channel: Option<String>,
}

#[derive(Debug, Parser)]
struct XcpArgs {
    /// CAN 通道
    #[arg(long)]
    channel: String,
    /// 命令帧标识符
    #[arg(long, value_parser = parse_u32, default_value = "0x600")]
    tx_id: u32,
    /// 响应帧标识符
    #[arg(long, value_parser = parse_u32, default_value = "0x601")]
    rx_id: u32,
    /// 单次等待响应毫秒数
    #[arg(long, default_value_t = 200)]
    timeout_ms: u64,
}

#[derive(Debug, Parser)]
struct ReadArgs {
    /// CAN 通道
    #[arg(long)]
    channel: String,
    /// 起始地址，支持 0x 前缀
    #[arg(long, value_parser = parse_u32)]
    address: u32,
    /// 字节数
    #[arg(long)]
    len: u16,
    /// 命令帧标识符
    #[arg(long, value_parser = parse_u32, default_value = "0x600")]
    tx_id: u32,
    /// 响应帧标识符
    #[arg(long, value_parser = parse_u32, default_value = "0x601")]
    rx_id: u32,
    /// 单次等待响应毫秒数
    #[arg(long, default_value_t = 200)]
    timeout_ms: u64,
}

fn parse_u32(text: &str) -> Result<u32, std::num::ParseIntError> {
    match text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
        Some(hex) => u32::from_str_radix(hex, 16),
        None => text.parse(),
    }
}

#[derive(Debug, Serialize)]
struct BusReport {
    channels: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    probed: Option<ProbeReport>,
}

#[derive(Debug, Serialize)]
struct ProbeReport {
    channel: String,
    backend: String,
    classic_can: bool,
    can_fd: bool,
    hardware_timestamp: bool,
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

#[derive(Debug, Serialize)]
struct ConnectReport {
    channel: String,
    resource: u8,
    max_cto: u16,
    max_dto: u16,
    protocol_version: Option<String>,
}

#[derive(Debug, Serialize)]
struct ReadReport {
    channel: String,
    address: String,
    len: u16,
    data_hex: String,
}

fn session_config(
    channel: &str,
    tx_id: u32,
    rx_id: u32,
    timeout_ms: u64,
) -> Result<SessionConfig, Fault> {
    let mut config = SessionConfig::new(channel);
    config.command_id = openape_bus::CanId::standard(tx_id as u16)
        .ok_or_else(|| usage_fault(format!("--tx-id {tx_id:#x} 超出 11 位标准帧范围")))?;
    config.response_id = openape_bus::CanId::standard(rx_id as u16)
        .ok_or_else(|| usage_fault(format!("--rx-id {rx_id:#x} 超出 11 位标准帧范围")))?;
    config.timeout = Duration::from_millis(timeout_ms);
    Ok(config)
}

fn usage_fault(message: String) -> Fault {
    Fault::new(
        openape_diag::FaultCode::new(openape_diag::Domain::Internal, 1).expect("序号 1 合法"),
        openape_diag::Severity::Error,
        message,
    )
    .with_hint("标识符取 0x000..=0x7FF；扩展帧暂不支持")
}

async fn run(cli: &Cli) -> Result<String, Fault> {
    match &cli.command {
        Command::Probe { target } => {
            match target {
                ProbeTarget::Bus(args) => {
                    let channels = list_channels();
                    let probed = match &args.channel {
                        Some(channel) => Some(ProbeReport::from(probe_channel(channel)?)),
                        None => None,
                    };
                    let report = BusReport { channels, probed };
                    Ok(render(cli.json, &report, || {
                        let mut out = String::new();
                        out.push_str(&format!("通道 {} 条", report.channels.len()));
                        if report.channels.is_empty() {
                            out.push_str("\n  无 CAN 通道；vcan 可用 `ip link add dev vcan0 type vcan` 建一个");
                        }
                        for channel in &report.channels {
                            out.push_str(&format!("\n  {channel}"));
                        }
                        if let Some(probe) = &report.probed {
                            out.push_str(&format!(
                                "\n探测 {}（{}）：经典 CAN={} CAN-FD={} 硬件时间戳={}",
                                probe.channel,
                                probe.backend,
                                probe.classic_can,
                                probe.can_fd,
                                probe.hardware_timestamp
                            ));
                        }
                        out
                    })?)
                }
                ProbeTarget::Xcp(args) => {
                    let config =
                        session_config(&args.channel, args.tx_id, args.rx_id, args.timeout_ms)?;
                    let mut session = Session::open(config)?;
                    let info = session.connect(0).await?;
                    let report = ConnectReport {
                        channel: session.channel().to_owned(),
                        resource: info.resource,
                        max_cto: info.max_cto,
                        max_dto: info.max_dto,
                        protocol_version: info
                            .protocol_version
                            .map(|(major, minor)| format!("{major}.{minor}")),
                    };
                    session.disconnect().await.ok();
                    Ok(render(cli.json, &report, || {
                        format!(
                            "{} 建链成功：resource={:#04x} maxCto={} maxDto={}",
                            report.channel, report.resource, report.max_cto, report.max_dto
                        )
                    })?)
                }
            }
        }
        Command::Read(args) => {
            let config = session_config(&args.channel, args.tx_id, args.rx_id, args.timeout_ms)?;
            let mut session = Session::open(config)?;
            session.connect(0).await?;
            let address = Address::new(args.address);
            let len =
                ByteLen::new(args.len).ok_or_else(|| usage_fault("--len 必须大于 0".into()))?;
            let data = session.read(address, len).await?;
            session.disconnect().await.ok();
            let report = ReadReport {
                channel: session.channel().to_owned(),
                address: address.to_string(),
                len: len.get(),
                data_hex: data
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect::<Vec<_>>()
                    .join(" "),
            };
            Ok(render(cli.json, &report, || {
                format!("{} {} = [{}]", report.address, report.len, report.data_hex)
            })?)
        }
    }
}

fn render<T: Serialize>(
    json: bool,
    value: &T,
    human: impl FnOnce() -> String,
) -> Result<String, Fault> {
    if json {
        serde_json::to_string_pretty(value)
            .map_err(|err| usage_fault(format!("结构化输出失败：{err}")))
    } else {
        Ok(human())
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    log::init(if cli.verbose {
        Verbosity::Verbose
    } else {
        Verbosity::Normal
    });
    log::set_frame_trace(cli.frame_trace);

    let runtime = match tokio::runtime::Runtime::new() {
        Ok(runtime) => runtime,
        Err(err) => {
            eprintln!("无法启动异步运行时：{err}");
            return ExitCode::from(90);
        }
    };

    match runtime.block_on(run(&cli)) {
        Ok(text) => {
            println!("{text}");
            ExitCode::SUCCESS
        }
        Err(fault) => {
            if cli.json {
                match serde_json::to_string_pretty(&fault) {
                    Ok(text) => eprintln!("{text}"),
                    Err(_) => eprintln!("{fault}"),
                }
            } else {
                eprintln!("{fault}");
            }
            ExitCode::from(fault.exit_code())
        }
    }
}
