//! 按配置选总线后端。**这是全仓唯一按名字指认后端的位置**（[D-17]）。
//!
//! 上层只看 [`Bus`] 与 [`BusCapabilities`]：加一种硬件 = 这里加一个分支，
//! `xcp`、`session`、界面与 CLI 一行都不改。平台差异也收在这里——选了
//! Windows 专用的后端而当前平台没有，错误由后端自己给出，上层不写 `cfg!`。
//!
//! [D-17]: ../../../docs/decisions.md#d-17-crate-分档与依赖方向

use std::sync::Arc;

use openape_bus::Bus;
use openape_diag::{Domain, Fault, FaultCode, Severity};

/// 配置里写的后端名。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    /// Linux SocketCAN（含 vcan）。
    SocketCan,
    /// 周立功 CAN-FD 卡，仅 Windows。
    Zlg,
}

impl Backend {
    /// 从配置文本解析。未知名字不猜，直接拒绝——猜错会打开另一台设备。
    pub fn parse(name: &str) -> Option<Self> {
        match name.trim().to_ascii_lowercase().as_str() {
            "socketcan" | "can" | "vcan" | "" => Some(Self::SocketCan),
            "zlg" => Some(Self::Zlg),
            _ => None,
        }
    }

    /// 规范名，用于日志与设备锁。
    pub const fn label(self) -> &'static str {
        match self {
            Self::SocketCan => openape_bus_socketcan::BACKEND,
            Self::Zlg => openape_bus_zlg::BACKEND,
        }
    }
}

/// 打开一条通道。
///
/// 不做"先试 socketcan 再试 zlg"这类兜底：试错会打开用户没指定的设备，
/// 而 CAN 设备通常是独占的。
pub fn open(backend: Backend, channel: &str) -> Result<Arc<dyn Bus>, Fault> {
    match backend {
        Backend::SocketCan => {
            let bus = openape_bus_socketcan::SocketCanBus::open(channel)?;
            Ok(Arc::new(bus))
        }
        Backend::Zlg => {
            let index = openape_bus_zlg::ZlgBus::parse_channel(channel)
                .ok_or_else(|| unknown_channel(channel, "周立功通道写成 `zlg:0` 或 `zlgbus0`"))?;
            let bus = openape_bus_zlg::ZlgBus::open(
                channel,
                openape_bus_zlg::ZlgConfig::default_for(index),
            )?;
            Ok(Arc::new(bus))
        }
    }
}

/// 打开失败的兜底故障。后端自己给的故障优先，这里只在参数层出错时用。
fn unknown_channel(channel: &str, hint: &str) -> Fault {
    Fault::new(
        FaultCode::from_const(Domain::Bus, 42),
        Severity::Error,
        format!("通道 {channel} 的写法不认识"),
    )
    .with_context("channel", channel.to_owned())
    .with_hint(hint.to_owned())
}

/// 后端名不认识时的故障，供命令行与配置解析共用。
pub fn unknown_backend(name: &str) -> Fault {
    Fault::new(
        FaultCode::from_const(Domain::Bus, 43),
        Severity::Error,
        format!("后端 {name} 不认识"),
    )
    .with_hint("可用：socketcan（Linux）、zlg（仅 Windows）")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_parse_to_backends() {
        assert_eq!(Backend::parse("socketcan"), Some(Backend::SocketCan));
        assert_eq!(Backend::parse(" vcan "), Some(Backend::SocketCan));
        assert_eq!(
            Backend::parse(""),
            Some(Backend::SocketCan),
            "空名是默认后端"
        );
        assert_eq!(Backend::parse("ZLG"), Some(Backend::Zlg), "名字不分大小写");
        assert_eq!(Backend::parse("vector"), None, "未支持的后端不猜");
    }

    #[test]
    fn unknown_names_get_an_actionable_fault() {
        let fault = unknown_backend("vector");
        assert_eq!(fault.code().domain(), Domain::Bus);
        assert!(fault.hint().is_some_and(|hint| hint.contains("zlg")));
    }

    #[test]
    fn backend_labels_are_the_crate_names() {
        assert_eq!(Backend::SocketCan.label(), "socketcan");
        assert_eq!(Backend::Zlg.label(), "zlg");
    }

    #[cfg(not(windows))]
    #[test]
    fn choosing_the_windows_backend_here_fails_with_the_platform_reason() {
        // 装配层不为平台写分支：错误来自后端，且要说清为什么。
        let Err(fault) = open(Backend::Zlg, "zlg:0") else {
            panic!("本机不该打开周立功后端");
        };
        assert!(fault.message().contains("只在 Windows"), "{fault}");
    }

    #[test]
    fn a_malformed_zlg_channel_is_refused_before_touching_anything() {
        let Err(fault) = open(Backend::Zlg, "vcan0") else {
            panic!("不是周立功通道名，不该打开");
        };
        assert!(
            fault.hint().is_some_and(|hint| hint.contains("zlg:0")),
            "{fault}"
        );
    }
}
