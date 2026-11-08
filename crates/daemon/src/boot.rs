//! 启动逻辑：把参数变成一台可服务的 daemon。
//!
//! 这部分放在库里而不是 `main.rs`，因为它是唯一可测的启动逻辑——`main` 只剩
//! 解析与 `serve`，那两件事没有分支可测，写不成测试也不该长。

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::Router;

use clap::Parser;
use openape_bus::{Bus, CanId};
use openape_bus_replay::{ReplayBus, ReplayFrame};
use openape_bus_socketcand::SocketcandBus;
use openape_cal_core::{Address, ByteLen};
use openape_diag::{Domain, Fault, FaultCode, Metrics, Severity};
use openape_session::{CaptureConfig, Session, SessionConfig, WriteContext};
use openape_xcp::eth::HeaderMode;
use openape_xcp::{DaqLayout, DaqListSpec, DaqSignal};

use crate::service::AppState;

/// 默认监听地址：只看本机。
pub const DEFAULT_LISTEN: &str = "127.0.0.1:47711";

/// 命令行参数。
#[derive(Debug, Clone, Parser)]
#[command(name = "openaped", about = "OpenAPE daemon：会话 + gRPC + 静态页")]
pub struct Args {
    /// 监听地址。默认仅回环，对外服务请在前面放网关。
    #[arg(long, default_value = DEFAULT_LISTEN)]
    pub listen: String,
    /// 接管的 CAN 通道。不给就只提供健康与指标。
    #[arg(long)]
    pub channel: Option<String>,
    /// 回放日志路径。给了它就不碰硬件：把日志当总线放（M5-10）。
    #[arg(long, conflicts_with_all = ["channel", "xcp_udp"])]
    pub replay: Option<PathBuf>,
    /// XCP-on-Ethernet 从站地址（`IP:端口`，UDP）。与 CAN 通道互斥；
    /// 对 XCPlite 这类默认带 CTR 头的对端再给 `--xcp-udp-headered`。
    #[arg(long, conflicts_with_all = ["channel", "replay", "socketcand"])]
    pub xcp_udp: Option<String>,
    /// UDP 包走 CTR 头模式（XCP-on-Ethernet，XCPlite 默认）。
    #[arg(long, requires = "xcp_udp")]
    pub xcp_udp_headered: bool,
    /// 对端的 DTO 头布局档案（D-27）。默认 `standard`（`[PID][数据]`）；
    /// XCPlite 这类带私有头的从站选 `xcplite`。选错不会报错，只会让
    /// 采集一条样本都出不来——`rejected` 计数会一直涨，daemon 日志也喊一声。
    #[arg(long, value_enum, default_value_t = DtoHeader::Standard, requires = "xcp_udp")]
    pub dto_header: DtoHeader,
    /// socketcand 守护进程地址（`host:端口`，TCP）。远端主机上的
    /// SocketCAN 接口经它访问（M5-11），接口名由 `--socketcand-channel`
    /// 给出。
    #[arg(long, conflicts_with_all = ["channel", "replay", "xcp_udp"])]
    pub socketcand: Option<String>,
    /// socketcand 模式下的接口名（远端主机上的 CAN 接口）。
    #[arg(long, default_value = "vcan0", requires = "socketcand")]
    pub socketcand_channel: String,
    /// 命令帧标识符。
    #[arg(long, default_value_t = 0x600)]
    pub tx_id: u32,
    /// 响应帧标识符，DAQ 数据帧也走它。
    #[arg(long, default_value_t = 0x601)]
    pub rx_id: u32,
    /// 静态页目录。
    #[arg(long, default_value = "ui/dist")]
    pub static_dir: PathBuf,
    /// DAQ 事件通道号。默认 0；真实从站的事件号按 A2L 或手册取（XCPlite
    /// 的 mainloop 是 1）。配错的表现是**命令全被接受但永不出帧**，不报错。
    #[arg(long, default_value_t = 0)]
    pub daq_event: u16,
    /// WRITE_DAQ 条目的地址扩展字节。默认 0——多数真 ECU 上 0 是绝对寻址；
    /// XCPlite 的默认配置里 0 是标定段相对，绝对寻址要 0x01。
    #[arg(long, value_parser = parse_u8, default_value_t = 0)]
    pub address_extension: u8,
    /// 打开 SET_DAQ_LIST_MODE 的时间戳位。有的从站（XCPlite）强制要这一位，
    /// 不带就拒配（回 0x21/0x24 这类话）。DTO 里带的时间戳目前只解析、
    /// 不进样本时间轴（D-25 / D-27）。
    #[arg(long)]
    pub daq_timestamp: bool,
    /// 采样信号，`地址:长度`（可重复），例如 `0x1000:2`。
    #[arg(long = "sample", value_parser = parse_signal)]
    pub samples: Vec<SampleSpec>,
    /// ELF 文件。给界面提供变量清单与地址。
    #[arg(long)]
    pub elf: Option<PathBuf>,
    /// A2L 文件。与 ELF 同时给时顺带做交叉校验，冲突进故障列表。
    #[arg(long)]
    pub a2l: Option<PathBuf>,
    /// 审计日志与写前快照的目录。默认临时目录下的 `openape`。
    #[arg(long)]
    pub state_dir: Option<PathBuf>,
}

/// 描述文件装配在 `assembly` 里：daemon 与 CLI 共用同一次解析的结果。
pub use openape_assembly::descriptors::{LoadedVariables, load_variables};

/// DTO 头布局档案的可选项（D-27）。命令行名字与
/// `openape_xcp_proto::dto_profile` 里的档案常量一一对应。
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum DtoHeader {
    /// 标准 `[PID][数据]`。
    Standard,
    /// XCPlite 的私有头：相对 ODT、0xAA 标记、DAQ list、32 位时间戳。
    Xcplite,
}

impl DtoHeader {
    /// 对应的档案。选哪一个只在这一处决定，泵与总线都不再分支。
    pub fn profile(self) -> &'static openape_xcp_proto::dto_profile::DtoProfile {
        use openape_xcp_proto::dto_profile as profiles;
        match self {
            Self::Standard => &profiles::STANDARD,
            Self::Xcplite => &profiles::XCPLITE,
        }
    }
}

/// DAQ 配置里那些「随 ECU 而定」的参数。默认值就是规范默认，改它们是为了
/// 适配具体从站——事件号按 A2L 取，地址扩展按该 ECU 的段约定取。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DaqOptions {
    /// 事件通道号。
    pub event: u16,
    /// WRITE_DAQ 条目的地址扩展字节。
    pub address_extension: u8,
    /// 要求 DTO 带从站时间戳（SET_DAQ_LIST_MODE 的 timestamp 位）。
    pub timestamp: bool,
}

impl DaqOptions {
    /// 拼一条 DAQ list：预分频固定 1（要抽稀时另说，现在没有需求）。
    fn spec(&self, signals: Vec<DaqSignal>) -> Option<DaqListSpec> {
        DaqListSpec::new(self.event, 1, signals)
            .map(|spec| spec.with_address_extension(self.address_extension))
            .map(|spec| {
                if self.timestamp {
                    spec.with_timestamp()
                } else {
                    spec
                }
            })
    }
}

impl Args {
    /// 从命令行取 DAQ 参数。
    pub fn daq_options(&self) -> DaqOptions {
        DaqOptions {
            event: self.daq_event,
            address_extension: self.address_extension,
            timestamp: self.daq_timestamp,
        }
    }
}

/// 一路采样。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SampleSpec {
    /// 首地址。
    pub address: u32,
    /// 字节数。
    pub len: u16,
}

/// 解析 `地址:长度`。
pub fn parse_signal(raw: &str) -> Result<SampleSpec, String> {
    let (address, len) = raw
        .split_once(':')
        .ok_or_else(|| format!("{raw} 形如 0x1000:2"))?;
    let address = parse_u32(address)?;
    let len: u16 = len.parse().map_err(|_| format!("{len} 不是字节数"))?;
    if len == 0 {
        return Err("长度必须大于 0".to_owned());
    }
    Ok(SampleSpec { address, len })
}

/// 解析十进制或 `0x` 十六进制。
pub fn parse_u32(raw: &str) -> Result<u32, String> {
    let trimmed = raw.trim();
    match trimmed
        .strip_prefix("0x")
        .or_else(|| trimmed.strip_prefix("0X"))
    {
        Some(hex) => u32::from_str_radix(hex, 16).map_err(|_| format!("{raw} 不是十六进制数")),
        None => trimmed.parse().map_err(|_| format!("{raw} 不是数字")),
    }
}

/// 解析十进制或 `0x` 十六进制的单字节。
pub fn parse_u8(raw: &str) -> Result<u8, String> {
    let value = parse_u32(raw)?;
    u8::try_from(value).map_err(|_| format!("{raw} 超出单字节范围（0x00..=0xFF）"))
}

/// 用参数拼出会话配置，顺带校验标识符范围。
pub fn session_config(channel: &str, tx_id: u32, rx_id: u32) -> Result<SessionConfig, Fault> {
    let mut config = SessionConfig::new(channel);
    config.command_id = CanId::standard(to_id(tx_id, "--tx-id")?)
        .ok_or_else(|| usage("--tx-id 超出 11 位标准帧范围"))?;
    config.response_id = CanId::standard(to_id(rx_id, "--rx-id")?)
        .ok_or_else(|| usage("--rx-id 超出 11 位标准帧范围"))?;
    Ok(config)
}

/// 把采样列表转成协议信号表。
pub fn signals(specs: &[SampleSpec]) -> Result<Vec<DaqSignal>, Fault> {
    if specs.is_empty() {
        return Err(usage("给了 --channel 却没有 --sample，无事可做"));
    }
    specs
        .iter()
        .map(|spec| {
            Ok(DaqSignal {
                address: Address::new(spec.address),
                len: ByteLen::new(spec.len).ok_or_else(|| usage("采样长度必须大于 0"))?,
            })
        })
        .collect()
}

/// 开会话、建链、配 DAQ、起采集泵。
///
/// 任一失败都直接报错：半启动的 daemon 比不启动更难排查——设备锁在手却
/// 没有采集，运维只能靠猜。
pub async fn open_session(
    channel: &str,
    tx_id: u32,
    rx_id: u32,
    specs: &[SampleSpec],
    daq: DaqOptions,
    metrics: Arc<Metrics>,
) -> Result<AppState, Fault> {
    let config = session_config(channel, tx_id, rx_id)?;
    let list = daq
        .spec(signals(specs)?)
        .ok_or_else(|| usage("采样列表为空"))?;

    let mut session = Session::open(config)?;
    let info = session.connect(0).await?;
    tracing::info!(max_cto = info.max_cto, max_dto = info.max_dto, "建链成功");
    let layout = session.configure_daq(&[list]).await?;
    let capture = session.start_capture_with(
        layout,
        CaptureConfig::default().with_metrics(metrics.clone()),
    );
    Ok(AppState::with_capture(
        metrics,
        channel.to_owned(),
        session,
        capture,
        Some(info),
    ))
}

/// 把回放日志解成帧序列，按扩展名分派（.asc 文本 / .blf 二进制）。
pub fn parse_replay(path: &Path) -> Result<Vec<ReplayFrame>, Fault> {
    let extension = path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(str::to_ascii_lowercase);
    match extension.as_deref() {
        Some("asc") => {
            let text = std::fs::read_to_string(path)
                .map_err(|err| usage(format!("读回放日志失败：{err}")))?;
            openape_bus_replay::asc::parse(&text).map_err(|err| usage(err.to_string()))
        }
        Some("blf") => {
            let file =
                std::fs::File::open(path).map_err(|err| usage(format!("读回放日志失败：{err}")))?;
            openape_bus_replay::blf::frames(std::io::BufReader::new(file))
                .map_err(|err| usage(err.to_string()))
        }
        other => Err(usage(format!(
            "回放日志扩展名要是 .asc 或 .blf，拿到 {}（{}）",
            other.unwrap_or("无"),
            path.display()
        ))),
    }
}

/// 用回放日志装配会话与采集（M5-10）：不碰硬件、不拿设备锁，采集与
/// 实时走同一条泵、同一条代码路径。回放是单向观察：不建链也不配
/// DAQ，布局按参数声明的采样表镜像——日志里的 DTO 内容由它自己说话，
/// master 只需要知道每个 ODT 有哪些列。
pub fn open_replay_session(
    path: &Path,
    tx_id: u32,
    rx_id: u32,
    specs: &[SampleSpec],
    metrics: Arc<Metrics>,
) -> Result<AppState, Fault> {
    let config = session_config("replay", tx_id, rx_id)?;
    let frames = parse_replay(path)?;
    tracing::info!(frames = frames.len(), path = %path.display(), "回放日志装载");
    let bus: Arc<dyn Bus> = Arc::new(ReplayBus::new(frames));
    let session = Session::open_with_bus(config, bus)?;
    let mut layout = DaqLayout::default();
    let lens: Vec<ByteLen> = signals(specs)?.iter().map(|signal| signal.len).collect();
    layout.insert_odt(0, &lens);
    let capture = session.start_capture_with(
        layout,
        CaptureConfig::default().with_metrics(metrics.clone()),
    );
    Ok(AppState::with_capture(
        metrics,
        "replay".to_owned(),
        session,
        capture,
        None,
    ))
}

/// XCP-on-Ethernet 的连接参数。三项都是「对端是什么样」的描述，跟采样与
/// DAQ 参数分开传——它们的改动频率不同，混在一起只会让签名越长越乱。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UdpLink {
    /// 从站地址，`IP:端口`。
    pub peer: String,
    /// UDP 包走 CTR 头模式（一条包可装多条消息）。
    pub headered: bool,
    /// DTO 头布局档案（D-27）。
    pub dto_header: DtoHeader,
}

/// 连一台 XCP-on-Ethernet 从站装配会话与采集（M5-12）：UDP 总线进会
/// 话，建链、配 DAQ、起采集——采集与 CAN 走同一条泵，传输差异收在
/// `UdpXcpBus` 里。
pub async fn open_udp_session(
    link: &UdpLink,
    tx_id: u32,
    rx_id: u32,
    specs: &[SampleSpec],
    daq: DaqOptions,
    metrics: Arc<Metrics>,
) -> Result<AppState, Fault> {
    let address: SocketAddr = link
        .peer
        .parse()
        .map_err(|_| usage(format!("{} 形如 127.0.0.1:5555", link.peer)))?;
    let mode = if link.headered {
        HeaderMode::Headered
    } else {
        HeaderMode::Headerless
    };
    let config = session_config("udp", tx_id, rx_id)?;
    let bus = openape_xcp::eth::UdpXcpBus::open(address, config.response_id, mode)
        .await
        .map_err(|err| usage(format!("连 {address} 失败：{err}")))?;
    let mut session = Session::open_with_bus(config, Arc::new(bus))?;
    let list = daq
        .spec(signals(specs)?)
        .ok_or_else(|| usage("采样列表为空"))?;
    let info = session.connect(0).await?;
    tracing::info!(max_cto = info.max_cto, max_dto = info.max_dto, "建链成功");
    let layout = session.configure_daq(&[list]).await?;
    let capture = session.start_capture_with(
        layout,
        CaptureConfig::default()
            .with_metrics(metrics.clone())
            .with_dto(link.dto_header.profile()),
    );
    Ok(AppState::with_capture(
        metrics,
        "udp".to_owned(),
        session,
        capture,
        Some(info),
    ))
}

/// 连一台 socketcand 守护进程装配会话与采集（M5-11）：远端主机的
/// SocketCAN 接口经 TCP 暴露，采集与本地 CAN 走同一条泵——上层对
/// 后端零分支，能力位由 [`SocketcandBus`] 如实上报。
pub async fn open_socketcand_session(
    address: &str,
    channel: &str,
    tx_id: u32,
    rx_id: u32,
    specs: &[SampleSpec],
    daq: DaqOptions,
    metrics: Arc<Metrics>,
) -> Result<AppState, Fault> {
    let peer: SocketAddr = address
        .parse()
        .map_err(|_| usage(format!("{address} 形如 127.0.0.1:29536")))?;
    let bus = SocketcandBus::open(peer, channel)
        .await
        .map_err(|err| usage(format!("连 socketcand {address} 失败：{err}")))?;
    let config = session_config(channel, tx_id, rx_id)?;
    let mut session = Session::open_with_bus(config, Arc::new(bus))?;
    let list = daq
        .spec(signals(specs)?)
        .ok_or_else(|| usage("采样列表为空"))?;
    let info = session.connect(0).await?;
    tracing::info!(max_cto = info.max_cto, max_dto = info.max_dto, "建链成功");
    let layout = session.configure_daq(&[list]).await?;
    let capture = session.start_capture_with(
        layout,
        CaptureConfig::default().with_metrics(metrics.clone()),
    );
    Ok(AppState::with_capture(
        metrics,
        format!("socketcand:{channel}"),
        session,
        capture,
        Some(info),
    ))
}

/// 按参数决定状态：给了通道就接管，给了回放日志就放日志，给了 UDP
/// 从站就连以太网，给了 socketcand 就走远端主机，都没给就只做健康
/// 与指标。
pub async fn build_state(args: &Args, metrics: Arc<Metrics>) -> Result<Arc<AppState>, Fault> {
    let state = match (
        args.channel.as_deref(),
        args.replay.as_deref(),
        args.xcp_udp.as_deref(),
        args.socketcand.as_deref(),
    ) {
        (Some(channel), _, _, _) => {
            open_session(
                channel,
                args.tx_id,
                args.rx_id,
                &args.samples,
                args.daq_options(),
                metrics,
            )
            .await?
        }
        (_, Some(path), _, _) => {
            open_replay_session(path, args.tx_id, args.rx_id, &args.samples, metrics)?
        }
        (_, _, Some(peer), _) => {
            open_udp_session(
                &UdpLink {
                    peer: peer.to_owned(),
                    headered: args.xcp_udp_headered,
                    dto_header: args.dto_header,
                },
                args.tx_id,
                args.rx_id,
                &args.samples,
                args.daq_options(),
                metrics,
            )
            .await?
        }
        (_, _, _, Some(address)) => {
            open_socketcand_session(
                address,
                &args.socketcand_channel,
                args.tx_id,
                args.rx_id,
                &args.samples,
                args.daq_options(),
                metrics,
            )
            .await?
        }
        (None, None, None, None) => AppState::idle(metrics),
    };
    // 变量清单与是否接管硬件无关：可以只看描述文件不连 ECU。
    let state = match load_variables(args.elf.as_deref(), args.a2l.as_deref()) {
        Some(loaded) => {
            let mode = loaded.mode;
            let tree = loaded.tree;
            let state = state.with_variables(loaded.list);
            match tree {
                Some(tree) => state.with_tree(tree, mode),
                None => state,
            }
        }
        None => state,
    };
    // 写入上下文与清单无关：没有清单也能按地址 raw 写。
    let context = WriteContext::open(&state_dir(args), "daemon", state_mode(&state))?;
    Ok(Arc::new(state.with_write_context(context)))
}

/// 审计与快照放在哪里。默认走临时目录：这两类数据是过程记录，不该混进
/// 用户的工程目录。
pub fn state_dir(args: &Args) -> PathBuf {
    args.state_dir
        .clone()
        .unwrap_or_else(|| std::env::temp_dir().join("openape"))
}

/// 当前状态的输入档：有变量树且有 A2L 才算带语义。
fn state_mode(state: &AppState) -> openape_cal_core::SourceMode {
    if state.tree().is_some() {
        openape_cal_core::SourceMode::WithA2l
    } else {
        openape_cal_core::SourceMode::ElfOnly
    }
}

/// 组装好的路由与已绑定的监听口。
pub struct Prepared {
    /// 应用。
    pub app: Router,
    /// 已绑定的监听口（`--listen` 是 `:0` 时端口由系统给）。
    pub listener: tokio::net::TcpListener,
    /// 实际监听地址。
    pub address: SocketAddr,
}

/// 建状态、组路由、绑定端口。三件事有分支，所以它们属于库。
pub async fn prepare(args: &Args, metrics: Arc<Metrics>) -> Result<Prepared, Fault> {
    let state = build_state(args, metrics).await?;
    let app = crate::server::build_router(state, &args.static_dir);
    let listener = tokio::net::TcpListener::bind(&args.listen)
        .await
        .map_err(|err| usage(format!("绑定 {} 失败：{err}", args.listen)))?;
    let address = listener
        .local_addr()
        .map_err(|err| usage(format!("读监听地址失败：{err}")))?;
    Ok(Prepared {
        app,
        listener,
        address,
    })
}

/// 起服务直到被关闭。二进制只调它，可测的部分都在这里。
pub async fn run(args: Args) -> Result<(), Fault> {
    let metrics = Arc::new(Metrics::new());
    let prepared = prepare(&args, metrics).await?;
    match (&args.channel, &args.replay, &args.xcp_udp, &args.socketcand) {
        (Some(channel), _, _, _) => tracing::info!(channel, "已接管通道"),
        (None, Some(path), _, _) => tracing::info!(path = %path.display(), "回放模式"),
        (None, _, Some(peer), _) => tracing::info!(peer, "XCP-on-Ethernet 模式"),
        (None, _, _, Some(address)) => {
            tracing::info!(address, channel = %args.socketcand_channel, "socketcand 模式")
        }
        (None, None, None, None) => {}
    }
    println!("openaped 监听 http://{}", prepared.address);
    axum::serve(prepared.listener, prepared.app)
        .await
        .map_err(|err| usage(format!("服务异常退出：{err}")))
}

/// 从环境变量解析参数后起服务。
pub async fn run_from_env() -> Result<(), Fault> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();
    run(Args::parse()).await
}

fn to_id(raw: u32, flag: &str) -> Result<u16, Fault> {
    u16::try_from(raw)
        .ok()
        .filter(|id| *id <= 0x7FF)
        .ok_or_else(|| usage(format!("{flag} {raw:#x} 超出 11 位标准帧范围")))
}

fn usage(message: impl Into<String>) -> Fault {
    Fault::new(
        FaultCode::new(Domain::Internal, 1).expect("序号 1 合法"),
        Severity::Error,
        message.into(),
    )
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[test]
    fn signals_parse_hex_address_with_length() {
        assert_eq!(
            parse_signal("0x1000:2"),
            Ok(SampleSpec {
                address: 0x1000,
                len: 2
            })
        );
        assert_eq!(
            parse_signal("4096:4"),
            Ok(SampleSpec {
                address: 4096,
                len: 4
            })
        );
    }

    #[test]
    fn bad_signal_spellings_are_refused() {
        assert!(parse_signal("0x1000").is_err(), "缺长度");
        assert!(parse_signal("0x1000:0").is_err(), "长度 0");
        assert!(parse_signal("0xzz:2").is_err(), "非十六进制");
        assert!(parse_signal("0x1000:abc").is_err(), "长度不是数");
    }

    #[test]
    fn ids_beyond_eleven_bits_are_refused() {
        assert!(to_id(0x800, "--tx-id").is_err());
        assert!(to_id(0x7FF, "--tx-id").is_ok());
    }

    #[test]
    fn session_config_carries_the_requested_ids() {
        let config = session_config("vcan0", 0x123, 0x456).expect("合法");
        assert_eq!(config.channel, "vcan0");
        assert_eq!(config.command_id, CanId::standard(0x123).expect("合法"));
        assert_eq!(config.response_id, CanId::standard(0x456).expect("合法"));
    }

    #[test]
    fn an_empty_sample_list_is_refused_before_opening_anything() {
        let err = signals(&[]).expect_err("空列表");
        assert!(err.message().contains("--sample"), "{err}");
    }

    #[test]
    fn signals_translate_addresses_and_lengths() {
        let signals = signals(&[SampleSpec {
            address: 0x1000,
            len: 2,
        }])
        .expect("合法");
        assert_eq!(signals[0].address, Address::new(0x1000));
        assert_eq!(signals[0].len.get(), 2);
    }

    #[test]
    fn args_parse_samples_and_defaults() {
        let args = Args::parse_from(["openaped", "--sample", "0x1000:2", "--sample", "4096:4"]);
        assert_eq!(
            args.samples,
            vec![
                SampleSpec {
                    address: 0x1000,
                    len: 2
                },
                SampleSpec {
                    address: 4096,
                    len: 4
                }
            ]
        );
        assert_eq!(args.listen, DEFAULT_LISTEN, "默认只看本机");
    }

    #[tokio::test]
    async fn an_idle_daemon_needs_no_device() {
        let args = Args::parse_from(["openaped"]);
        let state = build_state(&args, Arc::new(Metrics::new()))
            .await
            .expect("不接管通道不需要设备");
        assert!(state.channel().is_none());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn prepare_binds_and_reports_the_real_address() {
        let args = Args::parse_from(["openaped", "--listen", "127.0.0.1:0"]);
        let prepared = prepare(&args, Arc::new(Metrics::new()))
            .await
            .expect("端口可绑");
        assert_ne!(prepared.address.port(), 0, "系统分配的端口要读回来");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn run_keeps_serving_until_dropped() {
        let args = Args::parse_from(["openaped", "--listen", "127.0.0.1:0"]);
        let server = tokio::spawn(run(args));
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        assert!(!server.is_finished(), "服务不该自己退出");
        server.abort();
    }

    #[test]
    fn unreadable_files_leave_a_note_instead_of_an_empty_table() {
        // 文件给错是常事（路径写错、换了机器）。界面要能说"读 ELF 失败：
        // 没有这个文件"，而不是画一张空表格让人以为 ECU 没有变量。
        let missing = std::path::Path::new("/definitely/not/here.elf");
        let list = load_variables(Some(missing), None).expect("给了文件就该有清单");
        assert!(!list.list.loaded);
        assert!(list.list.variables.is_empty());
        assert!(
            list.list
                .notes
                .iter()
                .any(|note| note.contains("读 ELF 失败")),
            "失败原因要留在提示里：{:?}",
            list.list.notes
        );
    }

    #[test]
    fn unreadable_a2l_is_reported_too() {
        let missing = std::path::Path::new("/definitely/not/here.a2l");
        let list = load_variables(None, Some(missing)).expect("给了文件就该有清单");
        assert!(!list.list.loaded);
        assert!(
            list.list.notes.iter().any(|note| note.contains("读不了")),
            "失败原因要留在提示里：{:?}",
            list.list.notes
        );
    }

    #[test]
    fn no_descriptor_files_means_not_loaded_not_empty() {
        // 「未加载」与「加载了但没有变量」是两件事：界面上前者要提示去加
        // --elf，后者要提示文件可能不对。
        assert!(load_variables(None, None).is_none());
    }

    #[test]
    fn elf_variables_reach_the_wire_with_raw_marked() {
        let path = std::path::Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../fixtures/elf/test.elf"
        ));
        let list = load_variables(Some(path), None).expect("给了文件就该有清单");
        assert!(list.list.loaded);
        assert!(!list.list.variables.is_empty());
        assert!(
            list.list.variables.iter().all(|entry| entry.raw),
            "仅 ELF 档每条都是裸值，不填默认语义"
        );
        assert!(
            list.list
                .variables
                .iter()
                .all(|entry| entry.role.is_empty()),
            "角色留给用户，daemon 不替他判定"
        );
    }

    #[test]
    fn two_descriptors_in_agreement_produce_no_conflict_notes() {
        let elf = std::path::Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../fixtures/elf/test.elf"
        ));
        let a2l = std::path::Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../fixtures/a2l/elf-export-golden.a2l"
        ));
        let list = load_variables(Some(elf), Some(a2l)).expect("给了文件就该有清单");
        assert!(
            !list.list.notes.iter().any(|note| note.contains("不一致")),
            "同一来源的两份文件不该报冲突：{:?}",
            list.list.notes
        );
    }

    #[tokio::test]
    async fn a_missing_channel_fails_without_leaving_a_lock() {
        let err = open_session(
            "nosuchif9",
            0x600,
            0x601,
            &[SampleSpec {
                address: 0x1000,
                len: 2,
            }],
            DaqOptions::default(),
            Arc::new(Metrics::new()),
        )
        .await
        .expect_err("通道不存在");
        assert_eq!(err.code().domain(), Domain::Bus);
    }

    // ---- 回放（M5-10）----

    fn tempdir(name: &str) -> PathBuf {
        // 名字带调用方给的子名：同一线程里多次取目录时互不覆盖，不然
        // 第二次调用把第一个目录连根删了，写出线程建文件就 ENOENT。
        let dir = std::env::temp_dir().join(format!(
            "openape-daemon-boot-{}-{:?}-{name}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("建临时目录");
        dir
    }

    /// 一份最小回放日志：一帧杂音 + 两帧 DTO（响应 id 0x601、pid 0、
    /// 两个数据字节）。DTO 放在 100/300 ms：给测试订阅流留出时间——
    /// broadcast 队列只保订阅之后的样本。
    fn write_replay_log(path: &PathBuf) {
        std::fs::write(
            path,
            "\
date Sun Nov 09 12:00:00 2026
Begin Triggerblock
   0.000000 1  Tx   100   1   42
   0.100000 1  Rx   601   3   00 34 12
   0.300000 1  Rx   601   3   00 78 56
End TriggerBlock
",
        )
        .expect("写回放日志");
    }

    #[tokio::test]
    async fn a_replay_session_feeds_samples_with_log_timestamps() {
        let dir = tempdir("replay-session");
        let log = dir.join("run.asc");
        write_replay_log(&log);

        let state = open_replay_session(
            &log,
            0x600,
            0x601,
            &[SampleSpec {
                address: 0x1000,
                len: 2,
            }],
            Arc::new(Metrics::new()),
        )
        .expect("回放会话不碰硬件");
        assert_eq!(state.channel(), Some("replay"));

        let capture = state.capture().expect("回放也接管了采集");
        let mut reader = capture.stream().subscribe();
        let first = tokio::time::timeout(Duration::from_secs(2), reader.recv())
            .await
            .expect("首帧要在 100 ms 后出")
            .expect("有样本");
        assert_eq!(first.pid, 0);
        assert_eq!(first.payload, vec![0x34, 0x12]);
        let second = tokio::time::timeout(Duration::from_secs(2), reader.recv())
            .await
            .expect("次帧要在 300 ms 后出")
            .expect("有样本");
        assert_eq!(second.payload, vec![0x78, 0x56]);
        assert!(
            (280_000..=360_000).contains(&second.timestamp_us),
            "样本时间戳要对齐日志 300 ms 偏移（D-25 保留日志时间）：{}",
            second.timestamp_us
        );
    }

    #[tokio::test]
    async fn a_replay_session_refuses_an_unknown_extension_by_name() {
        let dir = tempdir("replay-ext");
        let log = dir.join("run.csv");
        std::fs::write(&log, "1,2\n").expect("写探针");
        let err = open_replay_session(
            &log,
            0x600,
            0x601,
            &[SampleSpec {
                address: 0x1000,
                len: 2,
            }],
            Arc::new(Metrics::new()),
        )
        .expect_err("未知扩展名要被点名");
        assert!(err.message().contains(".asc"), "{err}");
    }

    #[test]
    fn channel_and_replay_are_mutually_exclusive() {
        let err = Args::try_parse_from(["openaped", "--channel", "vcan0", "--replay", "run.asc"])
            .expect_err("两个来源互斥");
        assert!(err.to_string().contains("cannot be used"), "{err}");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn build_state_routes_replay_without_touching_the_bus_layer() {
        let dir = tempdir("replay-route");
        let log = dir.join("run.asc");
        write_replay_log(&log);
        let args = Args::parse_from([
            "openaped",
            "--listen",
            "127.0.0.1:0",
            "--replay",
            log.to_str().expect("路径可转"),
            "--sample",
            "0x1000:2",
            "--state-dir",
            dir.join("state").to_str().expect("路径可转"),
        ]);
        let state = build_state(&args, Arc::new(Metrics::new()))
            .await
            .expect("回放装配不需要设备");
        assert_eq!(state.channel(), Some("replay"), "回放也要占据通道位");
    }

    // ---- XCP-on-Ethernet（M5-12）----

    /// 起一台 sim-ecu 的 UDP 从站，返回它的地址。
    async fn udp_slave() -> std::net::SocketAddr {
        let socket = Arc::new(
            tokio::net::UdpSocket::bind("127.0.0.1:0")
                .await
                .expect("从站要能占端口"),
        );
        let addr = socket.local_addr().expect("地址可知");
        let mut ecu = openape_sim_ecu::SimEcu::new(openape_sim_ecu::SimEcuConfig::default_ids());
        ecu.poke(0x1000, &[0x11, 0x22]);
        tokio::spawn(openape_sim_ecu::serve_udp(
            ecu,
            socket,
            Arc::new(Metrics::new()),
            HeaderMode::Headerless,
        ));
        addr
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_udp_session_connects_to_the_sim_slave_and_streams_dto() {
        let addr = udp_slave().await;
        let dir = tempdir("udp-session");
        let args = Args::parse_from([
            "openaped",
            "--listen",
            "127.0.0.1:0",
            "--xcp-udp",
            addr.to_string().as_str(),
            "--sample",
            "0x1000:2",
            "--state-dir",
            dir.join("state").to_str().expect("路径可转"),
        ]);
        let state = build_state(&args, Arc::new(Metrics::new()))
            .await
            .expect("UDP 装配要能建链");
        assert_eq!(state.channel(), Some("udp"));
        let info = state.info().expect("建链信息要留在状态里");
        assert_ne!(info.max_cto, 0, "从站报了包宽");

        let capture = state.capture().expect("UDP 也接管了采集");
        let mut reader = capture.stream().subscribe();
        let sample = tokio::time::timeout(Duration::from_secs(2), reader.recv())
            .await
            .expect("两秒内要有 DTO 样本")
            .expect("有样本");
        assert_eq!(sample.payload, vec![0x11, 0x22], "DTO 值来自 poke 的内存");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_wrong_dto_header_yields_no_samples_but_grows_the_reject_counter() {
        // 从站发标准 DTO，命令却按 xcplite 档案解：这正是"配错档案"的样子。
        // 不能安静得像没在采集——样本为零的同时 rejected 必须涨。
        let addr = udp_slave().await;
        let dir = tempdir("udp-wrong-profile");
        let args = Args::parse_from([
            "openaped",
            "--listen",
            "127.0.0.1:0",
            "--xcp-udp",
            addr.to_string().as_str(),
            "--dto-header",
            "xcplite",
            "--sample",
            "0x1000:2",
            "--state-dir",
            dir.join("state").to_str().expect("路径可转"),
        ]);
        let state = build_state(&args, Arc::new(Metrics::new()))
            .await
            .expect("建链照样能成——错的是档案，不是链路");
        let capture = state.capture().expect("采集已起");
        let mut reader = capture.stream().subscribe();
        assert!(
            tokio::time::timeout(Duration::from_millis(400), reader.recv())
                .await
                .is_err(),
            "档案不符时不该凑出样本"
        );
        let stats = capture.stats();
        assert_eq!(stats.produced, 0);
        assert!(
            stats.rejected > 0,
            "配错的证据在 rejected 计数上：{stats:?}"
        );
    }

    #[test]
    fn daq_event_and_address_extension_default_to_zero_and_take_hex() {
        let plain = Args::try_parse_from(["openaped"]).expect("默认参数可解析");
        assert_eq!(
            plain.daq_options(),
            DaqOptions::default(),
            "不写就用规范默认：事件 0、地址扩展 0"
        );

        let tricky = Args::try_parse_from([
            "openaped",
            "--daq-event",
            "1",
            "--address-extension",
            "0x01",
            "--daq-timestamp",
        ])
        .expect("事件号、地址扩展与时间戳位可单独给");
        assert_eq!(
            tricky.daq_options(),
            DaqOptions {
                event: 1,
                address_extension: 0x01,
                timestamp: true,
            }
        );

        let err = Args::try_parse_from(["openaped", "--address-extension", "0x100"])
            .expect_err("地址扩展是单字节");
        assert!(err.to_string().contains("0x100"), "{err}");
    }

    #[test]
    fn dto_header_flag_picks_the_profile_and_requires_the_udp_endpoint() {
        let err = Args::try_parse_from(["openaped", "--dto-header", "xcplite"])
            .expect_err("档案开关不能脱离 UDP 端点用");
        assert!(err.to_string().contains("required"), "{err}");

        let args = Args::try_parse_from([
            "openaped",
            "--xcp-udp",
            "127.0.0.1:5555",
            "--dto-header",
            "xcplite",
        ])
        .expect("给了端点就能指定档案");
        assert_eq!(args.dto_header.profile().name, "xcplite");
        assert_eq!(
            args.dto_header.profile().pid,
            openape_xcp_proto::dto_profile::PidKind::RelativeToDaqList,
            "私有头档案的编号是 list 内相对号"
        );

        let default = Args::try_parse_from(["openaped", "--xcp-udp", "127.0.0.1:5555"])
            .expect("不写档案也能起");
        assert_eq!(
            default.dto_header.profile().name,
            "standard",
            "默认必须是标准口径，否则标准从站全被解错"
        );
    }

    #[test]
    fn headered_flag_requires_the_udp_endpoint() {
        let err = Args::try_parse_from(["openaped", "--xcp-udp-headered"])
            .expect_err("带头开关不能脱离端点用");
        assert!(err.to_string().contains("required"), "{err}");
        let args = Args::try_parse_from([
            "openaped",
            "--xcp-udp",
            "127.0.0.1:5555",
            "--xcp-udp-headered",
        ])
        .expect("给了端点就能开");
        assert!(args.xcp_udp_headered);
    }

    #[test]
    fn xcp_udp_is_mutually_exclusive_with_the_can_channel() {
        let err = Args::try_parse_from([
            "openaped",
            "--channel",
            "vcan0",
            "--xcp-udp",
            "127.0.0.1:5555",
        ])
        .expect_err("CAN 与 UDP 互斥");
        assert!(err.to_string().contains("cannot be used"), "{err}");
    }

    #[test]
    fn socketcand_arguments_carry_defaults_and_exclusions() {
        let args = Args::try_parse_from(["openaped", "--socketcand", "127.0.0.1:29536"])
            .expect("socketcand 地址可解析");
        assert_eq!(args.socketcand_channel, "vcan0", "接口名默认 vcan0");
        let err = Args::try_parse_from([
            "openaped",
            "--channel",
            "vcan0",
            "--socketcand",
            "127.0.0.1:29536",
        ])
        .expect_err("CAN 与 socketcand 互斥");
        assert!(err.to_string().contains("cannot be used"), "{err}");
        let err = Args::try_parse_from(["openaped", "--socketcand-channel", "can1"])
            .expect_err("接口名开关不能脱离地址用");
        assert!(err.to_string().contains("required"), "{err}");
    }

    #[tokio::test]
    async fn a_dead_socketcand_endpoint_fails_fast_with_a_named_cause() {
        // 没有守护进程监听的端口：连接被拒要转成人话故障，而不是挂着。
        let dead: SocketAddr = "127.0.0.1:1".parse().expect("合法地址");
        let err = open_socketcand_session(
            &dead.to_string(),
            "vcan0",
            0x600,
            0x601,
            &[SampleSpec {
                address: 0x1000,
                len: 2,
            }],
            DaqOptions::default(),
            Arc::new(Metrics::new()),
        )
        .await
        .expect_err("没有守护进程要失败");
        assert!(err.message().contains("socketcand"), "{err}");
    }
}
