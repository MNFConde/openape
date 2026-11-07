//! 启动逻辑：把参数变成一台可服务的 daemon。
//!
//! 这部分放在库里而不是 `main.rs`，因为它是唯一可测的启动逻辑——`main` 只剩
//! 解析与 `serve`，那两件事没有分支可测，写不成测试也不该长。

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::Router;

use clap::Parser;
use openape_bus::CanId;
use openape_cal_core::{Address, ByteLen};
use openape_diag::{Domain, Fault, FaultCode, Metrics, Severity};
use openape_session::{CaptureConfig, Session, SessionConfig};
use openape_xcp::{DaqListSpec, DaqSignal};

use openape_proto as proto;

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
    /// 命令帧标识符。
    #[arg(long, default_value_t = 0x600)]
    pub tx_id: u32,
    /// 响应帧标识符，DAQ 数据帧也走它。
    #[arg(long, default_value_t = 0x601)]
    pub rx_id: u32,
    /// 静态页目录。
    #[arg(long, default_value = "ui/dist")]
    pub static_dir: PathBuf,
    /// 采样信号，`地址:长度`（可重复），例如 `0x1000:2`。
    #[arg(long = "sample", value_parser = parse_signal)]
    pub samples: Vec<SampleSpec>,
    /// ELF 文件。给界面提供变量清单与地址。
    #[arg(long)]
    pub elf: Option<PathBuf>,
    /// A2L 文件。与 ELF 同时给时顺带做交叉校验，冲突进故障列表。
    #[arg(long)]
    pub a2l: Option<PathBuf>,
}

/// 加载变量清单。两个文件都不给时返回 `None`——界面据此说明"未加载描述
/// 文件"，而不是显示一个空表格让人以为 ECU 没有变量。
pub fn load_variables(elf: Option<&Path>, a2l: Option<&Path>) -> Option<proto::VariableList> {
    if elf.is_none() && a2l.is_none() {
        return None;
    }
    let mut notes = Vec::new();
    let elf_tree = elf.and_then(|path| load_elf_tree(path, &mut notes));
    let a2l_tree = a2l.and_then(|path| load_a2l_tree(path, &mut notes));
    if let (Some(elf), Some(a2l)) = (&elf_tree, &a2l_tree) {
        notes.extend(
            openape_assembly::cross_check(elf, a2l)
                .conflicts()
                .iter()
                .map(ToString::to_string),
        );
    }
    let primary = elf_tree.as_ref().or(a2l_tree.as_ref());
    Some(proto::VariableList {
        variables: primary.map(entries).unwrap_or_default(),
        notes,
        loaded: primary.is_some(),
    })
}

fn load_elf_tree(path: &Path, notes: &mut Vec<String>) -> Option<openape_cal_core::VariableTree> {
    let source = match openape_format_elf::load(path) {
        Ok(source) => source,
        Err(err) => {
            notes.push(format!("读 ELF 失败：{err}"));
            return None;
        }
    };
    let (tree, elf_notes) = match source.to_ir() {
        Ok(pair) => pair,
        Err(err) => {
            notes.push(format!("ELF 进 IR 失败：{err}"));
            return None;
        }
    };
    notes.extend(elf_notes.iter().map(ToString::to_string));
    Some(tree)
}

fn load_a2l_tree(path: &Path, notes: &mut Vec<String>) -> Option<openape_cal_core::VariableTree> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) => {
            notes.push(format!("读不了 {}：{err}", path.display()));
            return None;
        }
    };
    let source = match openape_format_a2l::load_str(&text, None) {
        Ok(source) => source,
        Err(err) => {
            notes.push(format!("读 A2L 失败：{err}"));
            return None;
        }
    };
    notes.extend(source.notes().iter().map(ToString::to_string));
    Some(source.tree().clone())
}

fn entries(tree: &openape_cal_core::VariableTree) -> Vec<proto::VariableEntry> {
    tree.iter()
        .map(|variable| proto::VariableEntry {
            symbol: variable.symbol().as_str().to_owned(),
            address: variable.address().get(),
            elem: format!("{:?}", variable.elem()),
            dims: variable.dims().to_vec(),
            role: variable
                .role()
                .map(|role| format!("{role:?}"))
                .unwrap_or_default(),
            raw: variable.semantics().is_empty(),
        })
        .collect()
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
    metrics: Arc<Metrics>,
) -> Result<AppState, Fault> {
    let config = session_config(channel, tx_id, rx_id)?;
    let list = DaqListSpec::new(0, 1, signals(specs)?).ok_or_else(|| usage("采样列表为空"))?;

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
        info,
    ))
}

/// 按参数决定状态：给了通道就接管，没给就只做健康与指标。
pub async fn build_state(args: &Args, metrics: Arc<Metrics>) -> Result<Arc<AppState>, Fault> {
    let state = match args.channel.as_deref() {
        Some(channel) => {
            open_session(channel, args.tx_id, args.rx_id, &args.samples, metrics).await?
        }
        None => AppState::idle(metrics),
    };
    // 变量清单与是否接管硬件无关：可以只看描述文件不连 ECU。
    Ok(Arc::new(
        match load_variables(args.elf.as_deref(), args.a2l.as_deref()) {
            Some(variables) => state.with_variables(variables),
            None => state,
        },
    ))
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
    if let Some(channel) = args.channel.as_deref() {
        tracing::info!(channel, "已接管通道");
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
        assert!(!list.loaded);
        assert!(list.variables.is_empty());
        assert!(
            list.notes.iter().any(|note| note.contains("读 ELF 失败")),
            "失败原因要留在提示里：{:?}",
            list.notes
        );
    }

    #[test]
    fn unreadable_a2l_is_reported_too() {
        let missing = std::path::Path::new("/definitely/not/here.a2l");
        let list = load_variables(None, Some(missing)).expect("给了文件就该有清单");
        assert!(!list.loaded);
        assert!(
            list.notes.iter().any(|note| note.contains("读不了")),
            "失败原因要留在提示里：{:?}",
            list.notes
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
        assert!(list.loaded);
        assert!(!list.variables.is_empty());
        assert!(
            list.variables.iter().all(|entry| entry.raw),
            "仅 ELF 档每条都是裸值，不填默认语义"
        );
        assert!(
            list.variables.iter().all(|entry| entry.role.is_empty()),
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
            !list.notes.iter().any(|note| note.contains("不一致")),
            "同一来源的两份文件不该报冲突：{:?}",
            list.notes
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
            Arc::new(Metrics::new()),
        )
        .await
        .expect_err("通道不存在");
        assert_eq!(err.code().domain(), Domain::Bus);
    }
}
