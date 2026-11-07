//! `openaped`：常驻外壳。
//!
//! 默认只绑回环（D-22）：远程接入、认证与 ACL 由前置网关承担，daemon 自己
//! 不实现。要对外提供服务，用 `--listen 0.0.0.0:47711` 显式开放。

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use clap::Parser;
use openape_bus::CanId;
use openape_cal_core::{Address, ByteLen};
use openape_diag::{Fault, Metrics};
use openape_session::{Session, SessionConfig};
use openape_xcp::{DaqListSpec, DaqSignal};
use tracing_subscriber::EnvFilter;

use openape_daemon::server;
use openape_daemon::service::AppState;

/// 默认监听地址：只看本机。
const DEFAULT_LISTEN: &str = "127.0.0.1:47711";

#[derive(Debug, Parser)]
#[command(name = "openaped", about = "OpenAPE daemon：会话 + gRPC + 静态页")]
struct Args {
    /// 监听地址。默认仅回环，对外服务请在前面放网关。
    #[arg(long, default_value = DEFAULT_LISTEN)]
    listen: String,
    /// 接管的 CAN 通道。不给就只提供健康与指标。
    #[arg(long)]
    channel: Option<String>,
    /// 命令帧标识符。
    #[arg(long, default_value_t = 0x600)]
    tx_id: u32,
    /// 响应帧标识符，DAQ 数据帧也走它。
    #[arg(long, default_value_t = 0x601)]
    rx_id: u32,
    /// 静态页目录。
    #[arg(long, default_value = "ui/dist")]
    static_dir: PathBuf,
    /// 采样信号，`地址:长度`（可重复），例如 `0x1000:2`。
    #[arg(long = "sample", value_parser = parse_signal)]
    samples: Vec<(u32, u16)>,
}

fn parse_signal(raw: &str) -> Result<(u32, u16), String> {
    let (address, len) = raw
        .split_once(':')
        .ok_or_else(|| format!("{raw} 形如 0x1000:2"))?;
    let address = parse_u32(address)?;
    let len: u16 = len.parse().map_err(|_| format!("{len} 不是字节数"))?;
    if len == 0 {
        return Err("长度必须大于 0".to_owned());
    }
    Ok((address, len))
}

fn parse_u32(raw: &str) -> Result<u32, String> {
    let trimmed = raw.trim();
    match trimmed
        .strip_prefix("0x")
        .or_else(|| trimmed.strip_prefix("0X"))
    {
        Some(hex) => u32::from_str_radix(hex, 16).map_err(|_| format!("{raw} 不是十六进制数")),
        None => trimmed.parse().map_err(|_| format!("{raw} 不是数字")),
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let args = Args::parse();
    let metrics = Arc::new(Metrics::new());
    let state = match args.channel.as_deref() {
        Some(channel) => Arc::new(open_session(channel, &args, metrics).await?),
        None => Arc::new(AppState::idle(metrics)),
    };

    if let Some(channel) = state.channel() {
        println!("已接管 {channel}");
    }

    let app = server::build_router(state, &args.static_dir);
    let listener = tokio::net::TcpListener::bind(&args.listen).await?;
    let address: SocketAddr = listener.local_addr()?;
    println!("openaped 监听 http://{address}");

    axum::serve(listener, app).await?;
    Ok(())
}

/// 开会话、建链、配 DAQ、起采集泵。任一失败都直接退出：半启动的 daemon
/// 比不启动更难排查。
async fn open_session(
    channel: &str,
    args: &Args,
    metrics: Arc<Metrics>,
) -> Result<AppState, Fault> {
    let mut config = SessionConfig::new(channel);
    config.command_id = CanId::standard(to_id(args.tx_id, "--tx-id")?)
        .ok_or_else(|| usage("--tx-id 超出 11 位标准帧范围"))?;
    config.response_id = CanId::standard(to_id(args.rx_id, "--rx-id")?)
        .ok_or_else(|| usage("--rx-id 超出 11 位标准帧范围"))?;
    let mut session = Session::open(config)?;
    let info = session.connect(0).await?;
    tracing::info!(max_cto = info.max_cto, max_dto = info.max_dto, "建链成功");

    if args.samples.is_empty() {
        // 没有采样信号就没有采集泵，接管通道只为了占锁，没有意义。
        return Err(usage("给了 --channel 却没有 --sample，无事可做"));
    }
    let signals = args
        .samples
        .iter()
        .map(|(address, len)| {
            Ok(DaqSignal {
                address: Address::new(*address),
                len: ByteLen::new(*len).ok_or_else(|| usage("采样长度必须大于 0"))?,
            })
        })
        .collect::<Result<Vec<_>, Fault>>()?;

    let list = DaqListSpec::new(0, 1, signals).ok_or_else(|| usage("采样列表为空"))?;
    let layout = session.configure_daq(&[list]).await?;
    let capture = session.start_capture(layout);
    Ok(AppState::with_capture(
        metrics,
        channel.to_owned(),
        session,
        capture,
    ))
}

fn to_id(raw: u32, flag: &str) -> Result<u16, Fault> {
    u16::try_from(raw)
        .ok()
        .filter(|id| *id <= 0x7FF)
        .ok_or_else(|| usage(format!("{flag} {raw:#x} 超出 11 位标准帧范围")))
}

fn usage(message: impl Into<String>) -> Fault {
    Fault::new(
        openape_diag::FaultCode::new(openape_diag::Domain::Internal, 1).expect("序号 1 合法"),
        openape_diag::Severity::Error,
        message.into(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signals_parse_hex_address_with_length() {
        assert_eq!(parse_signal("0x1000:2"), Ok((0x1000, 2)));
        assert_eq!(parse_signal("4096:4"), Ok((4096, 4)));
    }

    #[test]
    fn bad_signal_spellings_are_refused() {
        assert!(parse_signal("0x1000").is_err(), "缺长度");
        assert!(parse_signal("0x1000:0").is_err(), "长度 0");
        assert!(parse_signal("0xzz:2").is_err(), "非十六进制");
    }

    #[test]
    fn ids_beyond_eleven_bits_are_refused() {
        assert!(to_id(0x800, "--tx-id").is_err());
        assert!(to_id(0x7FF, "--tx-id").is_ok());
    }
}
