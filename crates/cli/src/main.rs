//! 单次探测外壳。
//!
//! 一条命令即一次自足事务：拿设备锁、建链、做完、退出。结构化输出（`--json`）
//! 供外部脚本与 AI 代理编排，退出码按故障域分段（D-18）。

#![forbid(unsafe_code)]

mod commands;
mod output;

use std::process::ExitCode;

use clap::{Parser, Subcommand};
use openape_diag::{Fault, FaultCode, Severity, Verbosity, log};

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
    /// 经运行中的 daemon 执行，复用它的会话与设备锁。
    ///
    /// 不给值时用 `http://127.0.0.1:47711`；自定义地址写成 `--via-daemon=URL`，
    /// 必须是等号形式，否则子命令名会被当成地址。
    #[arg(
        long,
        global = true,
        value_name = "URL",
        num_args = 0..=1,
        require_equals = true,
        default_missing_value = commands::DAEMON_DEFAULT
    )]
    via_daemon: Option<String>,
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

/// 命令行用法错误。归内部域：这不是 ECU 或总线的问题。
pub(crate) fn usage_fault(message: String) -> Fault {
    Fault::new(
        FaultCode::new(openape_diag::Domain::Internal, 1).expect("序号 1 合法"),
        Severity::Error,
        message,
    )
    .with_hint("用 `openape <命令> --help` 查看参数")
}

async fn run(cli: &Cli) -> Result<String, Fault> {
    match &cli.command {
        Command::Probe { target } => match target {
            ProbeTarget::Bus(args) => match cli.via_daemon.as_deref() {
                Some(url) => commands::probe_bus_via(url, args.channel.as_deref(), cli.json).await,
                None => commands::probe_bus(args.channel.as_deref(), cli.json).await,
            },
            ProbeTarget::Xcp(args) => match cli.via_daemon.as_deref() {
                Some(url) => commands::probe_xcp_via(url, &args.channel, cli.json).await,
                None => {
                    commands::probe_xcp(
                        &args.channel,
                        args.tx_id,
                        args.rx_id,
                        args.timeout_ms,
                        cli.json,
                    )
                    .await
                }
            },
        },
        Command::Read(args) => match cli.via_daemon.as_deref() {
            Some(url) => commands::read_via(url, args.address, args.len, cli.json).await,
            None => {
                commands::read_memory(
                    &args.channel,
                    args.address,
                    args.len,
                    args.tx_id,
                    args.rx_id,
                    args.timeout_ms,
                    cli.json,
                )
                .await
            }
        },
    }
}

fn report_fault(fault: &Fault, json: bool) {
    if json {
        match serde_json::to_string_pretty(fault) {
            Ok(text) => eprintln!("{text}"),
            Err(_) => eprintln!("{fault}"),
        }
    } else {
        eprintln!("{fault}");
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
            report_fault(&fault, cli.json);
            ExitCode::from(fault.exit_code())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_and_decimal_ids_parse() {
        assert_eq!(parse_u32("0x600").expect("十六进制"), 0x600);
        assert_eq!(parse_u32("0X10").expect("大写前缀"), 0x10);
        assert_eq!(parse_u32("1536").expect("十进制"), 1536);
        assert!(parse_u32("0xZZ").is_err());
    }

    #[test]
    fn via_daemon_defaults_to_the_local_daemon() {
        let cli = Cli::try_parse_from(["openape", "--via-daemon", "probe", "bus"]).expect("可解析");
        assert_eq!(
            cli.via_daemon.as_deref(),
            Some(commands::DAEMON_DEFAULT),
            "不带值时用默认地址"
        );

        let cli = Cli::try_parse_from(["openape", "--via-daemon=http://box:47711", "probe", "bus"])
            .expect("可解析");
        assert_eq!(cli.via_daemon.as_deref(), Some("http://box:47711"));
    }

    #[test]
    fn cli_definition_is_valid() {
        use clap::CommandFactory;
        Cli::command().debug_assert();
    }
}
