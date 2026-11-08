//! 单次探测外壳。
//!
//! 一条命令即一次自足事务：拿设备锁、建链、做完、退出。结构化输出（`--json`）
//! 供外部脚本与 AI 代理编排，退出码按故障域分段（D-18）。

#![forbid(unsafe_code)]

use std::process::ExitCode;

use clap::{Parser, Subcommand};
use openape_cli::{calibration, commands};
use openape_diag::{Fault, Verbosity, log};

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
    /// daemon 指标（性能面板的终端侧）。--watch 每秒刷新一行。
    Metrics {
        /// 循环刷新而不是打一次就走
        #[arg(long)]
        watch: bool,
    },
    /// 一次跑完各故障类别的判定（M5-16）。默认只查不碰 ECU；真建链要 --connect。
    Doctor(DoctorArgs),
    /// 读一段 ECU 内存
    Read(ReadArgs),
    /// 写一个标定量（单点写，带写前读回、量程校验、快照与审计）
    Write(WriteArgs),
    /// 标定页：查、切、对比
    Page {
        #[command(subcommand)]
        action: PageAction,
    },
    /// 标定数据集：对比与回滚
    Dataset {
        #[command(subcommand)]
        action: DatasetAction,
    },
}

#[derive(Debug, Subcommand)]
enum PageAction {
    /// 查当前标定页
    Get(CommonArgs),
    /// 切标定页（切完读回确认）
    Set(PageSetArgs),
    /// 参考页与工作页的差异
    Diff(PageDiffArgs),
}

#[derive(Debug, Subcommand)]
enum DatasetAction {
    /// 比较两份 dataset 文件
    Diff {
        /// 左侧文件
        #[arg(long)]
        left: std::path::PathBuf,
        /// 右侧文件
        #[arg(long)]
        right: std::path::PathBuf,
    },
    /// 按快照回滚（逐项单点写，同样留痕）
    Restore(WriteArgs),
}

/// doctor 自检的参数。所有项都可缺省：缺了就跳过那一类，不猜。
#[derive(Debug, Parser)]
struct DoctorArgs {
    /// CAN 通道（给了才做通道探测）
    #[arg(long)]
    channel: Option<String>,
    /// 描述文件 ELF（给了才做信息源检查）
    #[arg(long)]
    elf: Option<std::path::PathBuf>,
    /// 描述文件 A2L
    #[arg(long)]
    a2l: Option<std::path::PathBuf>,
    /// daemon 地址（给了才做 daemon 连通检查）
    #[arg(long, default_missing_value = commands::DAEMON_DEFAULT)]
    daemon: Option<String>,
    /// 真建一次 XCP 链（会碰 ECU；默认跳过）
    #[arg(long)]
    connect: bool,
}

/// 只连设备的公共参数。
#[derive(Debug, Parser)]
struct CommonArgs {
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
struct PageSetArgs {
    /// 目标页号
    #[arg(long)]
    page: u8,
    #[command(flatten)]
    common: CommonArgs,
}

#[derive(Debug, Parser)]
struct PageDiffArgs {
    /// 参考页号
    #[arg(long)]
    reference: u8,
    /// 工作页号
    #[arg(long)]
    working: u8,
    /// ELF 文件；页上取样的变量来自它
    #[arg(long)]
    elf: Option<std::path::PathBuf>,
    /// A2L 文件
    #[arg(long)]
    a2l: Option<std::path::PathBuf>,
    #[command(flatten)]
    common: CommonArgs,
}

#[derive(Debug, Parser)]
struct WriteArgs {
    /// CAN 通道
    #[arg(long)]
    channel: String,
    /// 符号名。给 `--value` 时必需。
    #[arg(long)]
    symbol: Option<String>,
    /// 起始地址。没有符号时必需。
    #[arg(long, value_parser = parse_u32)]
    address: Option<u32>,
    /// 原样字节，十六进制
    #[arg(long)]
    data: Option<String>,
    /// 物理值，按变量的元素类型与换算编码
    #[arg(long)]
    value: Option<f64>,
    /// ELF 文件
    #[arg(long)]
    elf: Option<std::path::PathBuf>,
    /// A2L 文件
    #[arg(long)]
    a2l: Option<std::path::PathBuf>,
    /// 审计与快照目录
    #[arg(long)]
    state_dir: Option<std::path::PathBuf>,
    /// 谁写入的，进审计
    #[arg(long, default_value = "cli")]
    actor: String,
    /// 按快照回滚时用：快照文件路径
    #[arg(long)]
    snapshot: Option<std::path::PathBuf>,
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

impl WriteArgs {
    fn session_options(&self) -> calibration::SessionOptions {
        calibration::SessionOptions {
            channel: self.channel.clone(),
            tx_id: self.tx_id,
            rx_id: self.rx_id,
            timeout_ms: self.timeout_ms,
        }
    }

    fn options(&self) -> calibration::WriteOptions {
        calibration::WriteOptions {
            session: self.session_options(),
            symbol: self.symbol.clone(),
            address: self.address,
            data: self.data.clone(),
            value: self.value,
            elf: self.elf.clone(),
            a2l: self.a2l.clone(),
            state_dir: self
                .state_dir
                .clone()
                .unwrap_or_else(|| std::env::temp_dir().join("openape")),
            actor: self.actor.clone(),
        }
    }
}

#[derive(Debug, Subcommand)]
enum ProbeTarget {
    /// 列出或探测 CAN 通道
    Bus(BusArgs),
    /// 探测 XCP 建链
    Xcp(XcpArgs),
    /// 报描述文件里的变量，两份都给时做交叉校验
    Source(SourceArgs),
}

#[derive(Debug, Parser)]
struct SourceArgs {
    /// ELF 文件
    #[arg(long)]
    elf: Option<std::path::PathBuf>,
    /// A2L 文件
    #[arg(long)]
    a2l: Option<std::path::PathBuf>,
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

async fn run(cli: &Cli) -> Result<String, Fault> {
    match &cli.command {
        Command::Probe { target } => probe(cli, target).await,
        Command::Metrics { watch } => metrics(cli, *watch).await,
        Command::Doctor(args) => doctor(cli, args).await,
        Command::Read(args) => read(cli, args).await,
        Command::Write(args) => write(cli, args).await,
        Command::Page { action } => page(cli, action).await,
        Command::Dataset { action } => dataset(cli, action).await,
    }
}

/// doctor：一次跑完各类别判定，收集全部结论而不是遇错即停。
async fn doctor(cli: &Cli, args: &DoctorArgs) -> Result<String, Fault> {
    let options = commands::DoctorOptions {
        channel: args.channel.as_deref(),
        elf: args.elf.as_deref(),
        a2l: args.a2l.as_deref(),
        daemon: args.daemon.as_deref(),
        connect: args.connect,
    };
    let checks = commands::doctor(options).await;
    let failed = checks
        .iter()
        .filter(|check| check.outcome == "fail")
        .count();
    let skipped = checks
        .iter()
        .filter(|check| check.outcome == "skip")
        .count();
    let text = commands::render_doctor(cli.json, &checks)?;
    if failed > 0 {
        // 明细先落 stdout（json 消费者要的也是这份），故障随后走统一面板
        // 与非零退出码——明细不能只活在"见上方报告"这句话里。
        println!("{text}");
        return Err(openape_cli::usage_fault(format!(
            "{failed} 类未通过（{skipped} 类跳过）"
        )));
    }
    Ok(text)
}

/// 指标：性能面板的终端侧。--watch 每秒重画，Ctrl-C 退出。
async fn metrics(cli: &Cli, watch: bool) -> Result<String, Fault> {
    let Some(url) = cli.via_daemon.as_deref() else {
        return Err(openape_cli::usage_fault(
            "指标在 daemon 手里：给 --via-daemon <地址>，或用默认本地地址".to_owned(),
        ));
    };
    let mut client = commands::connect_daemon(url).await?;
    let mut line = String::new();
    loop {
        let snapshot = client
            .get_metrics(openape_proto::Empty {})
            .await
            .map_err(commands::daemon_fault)?
            .into_inner();
        let human = format!(
            "采集 {} 帧 · {:.1} fps · 丢帧 {} · 延迟 p50 {} µs / p99 {} µs · 队列 {}",
            snapshot.daq_frames_total,
            snapshot.daq_fps,
            snapshot.dropped_frames_total,
            snapshot.latency_p50_us,
            snapshot.latency_p99_us,
            snapshot.queue_depth,
        );
        if !watch {
            // 与性能面板同一口径、同一份数字，只是搬到终端。
            return commands::render(cli.json, &snapshot, || human);
        }
        // 行内重画：\r 回行首 + 清到行尾，日志不被刷屏。
        line.clear();
        line.push_str(&human);
        print!("\r\x1b[K{line}");
        use std::io::Write as _;
        std::io::stdout().flush().ok();
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }
}

async fn write(cli: &Cli, args: &WriteArgs) -> Result<String, Fault> {
    match cli.via_daemon.as_deref() {
        Some(url) => calibration::write_via_daemon(url, &args.options(), cli.json).await,
        None => calibration::write_direct(&args.options(), cli.json).await,
    }
}

async fn page(cli: &Cli, action: &PageAction) -> Result<String, Fault> {
    match action {
        PageAction::Get(args) => {
            let options = calibration::SessionOptions {
                channel: args.channel.clone(),
                tx_id: args.tx_id,
                rx_id: args.rx_id,
                timeout_ms: args.timeout_ms,
            };
            calibration::page_get(&options, cli.via_daemon.as_deref(), cli.json).await
        }
        PageAction::Set(args) => {
            let options = calibration::SessionOptions {
                channel: args.common.channel.clone(),
                tx_id: args.common.tx_id,
                rx_id: args.common.rx_id,
                timeout_ms: args.common.timeout_ms,
            };
            calibration::page_set(&options, cli.via_daemon.as_deref(), args.page, cli.json).await
        }
        PageAction::Diff(args) => {
            let mut options = calibration::WriteOptions {
                session: calibration::SessionOptions {
                    channel: args.common.channel.clone(),
                    tx_id: args.common.tx_id,
                    rx_id: args.common.rx_id,
                    timeout_ms: args.common.timeout_ms,
                },
                symbol: None,
                address: None,
                data: None,
                value: None,
                elf: args.elf.clone(),
                a2l: args.a2l.clone(),
                state_dir: std::env::temp_dir().join("openape"),
                actor: "cli".to_owned(),
            };
            options.actor = "cli".to_owned();
            calibration::page_diff(
                &options,
                cli.via_daemon.as_deref(),
                args.reference,
                args.working,
                cli.json,
            )
            .await
        }
    }
}

async fn dataset(cli: &Cli, action: &DatasetAction) -> Result<String, Fault> {
    match action {
        DatasetAction::Diff { left, right } => calibration::dataset_diff(left, right, cli.json),
        DatasetAction::Restore(args) => {
            let snapshot = args.snapshot.as_deref().ok_or_else(|| {
                openape_cli::usage_fault("dataset restore 需要 --snapshot <文件>".to_owned())
            })?;
            calibration::dataset_restore(snapshot, &args.options(), cli.json).await
        }
    }
}

async fn probe(cli: &Cli, target: &ProbeTarget) -> Result<String, Fault> {
    match target {
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
        // 描述文件是本地的，不经过 daemon：它描述的 ECU 可能与 daemon 那头
        // 的会话不是同一台，借 daemon 读只会让两份数据的来源混在一起。
        ProbeTarget::Source(args) => {
            commands::probe_source(args.elf.as_deref(), args.a2l.as_deref(), cli.json)
        }
    }
}

async fn read(cli: &Cli, args: &ReadArgs) -> Result<String, Fault> {
    match cli.via_daemon.as_deref() {
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
