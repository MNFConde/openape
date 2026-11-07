//! 周立功（ZLG）CAN-FD 卡后端。
//!
//! 只在 Windows 上能用：`zlgcan.dll` 与它的驱动只发布在 Windows。本 crate 的
//! 责任是**如实报告这一点**——在别的平台上打开通道会得到一条带建议的故障，
//! 而不是一个会在运行期炸掉的句柄。
//!
//! 装配层按配置选后端，不为平台写分支（[D-17]）：选了 zlg 而平台不支持时，
//! 错误来自这里，而不是散落在上层各处的 `cfg!`。
//!
//! 帧路径与 socketcan 后端对等：发送、接收、超时语义与能力位都走同一份
//! [`Bus`] 契约，`xcp` 与 `session` 一行都不用改。
//!
//! **验证状态**：Windows 侧的 FFI 实现无法在 Linux 开发机上编译，因此本 crate
//! 在开发机只验证非 Windows 路径与平台无关逻辑；真机面见
//! [12-hardware-checklist.md](../../../docs/12-hardware-checklist.md)。
//!
//! [D-17]: ../../../docs/decisions.md#d-17-crate-分档与依赖方向

#![forbid(unsafe_code)]

use std::time::Duration;

use async_trait::async_trait;
use openape_bus::{Bus, BusCapabilities, BusError, CanFrame, CanId};
use openape_diag::{Domain, Fault, FaultCode, Severity};

#[cfg(windows)]
mod win;

/// 后端名。设备锁与日志都用它，改名等于换后端。
pub const BACKEND: &str = "zlg";

/// 周立功设备的默认设备类型号：USB-CAN FD 系列。
///
/// 型号与设备的对应关系写在厂商头文件里，改这里要同步改真机清单。
pub const DEFAULT_DEVICE_TYPE: u32 = 33;

/// 打开通道时需要的参数。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ZlgConfig {
    /// 设备类型号（厂商头文件里的 `ZCAN_DEVICE_TYPE`）。
    pub device_type: u32,
    /// 同型号的第几台设备，从 0 起。
    pub device_index: u32,
    /// 通道下标，从 0 起。
    pub channel_index: u32,
    /// 仲裁段波特率（bit/s）。
    pub nominal_bitrate: u32,
    /// 数据段波特率（bit/s）。等于仲裁段时按经典 CAN 开通道。
    pub data_bitrate: u32,
}

impl ZlgConfig {
    /// 按通道名解析出的默认配置：`zlg:0:0` 是"第 0 台的第 0 通道"。
    ///
    /// CAN-FD 用 500 kbit/s 仲裁 + 2 Mbit/s 数据，这是最常见的一组时序；
    /// 真机要按实际线束改。
    pub fn default_for(channel_index: u32) -> Self {
        Self {
            device_type: DEFAULT_DEVICE_TYPE,
            device_index: 0,
            channel_index,
            nominal_bitrate: 500_000,
            data_bitrate: 2_000_000,
        }
    }

    /// 是否按 CAN-FD 开通道。
    pub const fn can_fd(&self) -> bool {
        self.data_bitrate != self.nominal_bitrate
    }
}

/// 一条周立功通道。
#[derive(Debug)]
pub struct ZlgBus {
    channel: String,
    #[cfg(windows)]
    handle: win::Device,
}

impl ZlgBus {
    /// 打开一条通道。非 Windows 平台按"后端不可用"如实拒绝。
    pub fn open(channel: &str, config: ZlgConfig) -> Result<Self, Fault> {
        #[cfg(windows)]
        {
            let handle = win::Device::open(channel, &config)?;
            return Ok(Self {
                channel: channel.to_owned(),
                handle,
            });
        }
        #[cfg(not(windows))]
        {
            let _ = config;
            Err(unavailable(channel))
        }
    }

    /// 通道名指向的通道下标：`zlg:0`、`0`、`zlgbus0` 都解析成 0。
    pub fn parse_channel(channel: &str) -> Option<u32> {
        let trimmed = channel.trim();
        let digits = trimmed
            .strip_prefix("zlg:")
            .or_else(|| trimmed.strip_prefix("zlgbus"))
            .unwrap_or(trimmed);
        let digits = if digits.is_empty() { "0" } else { digits };
        digits.parse().ok()
    }
}

#[async_trait]
impl Bus for ZlgBus {
    fn backend(&self) -> &'static str {
        BACKEND
    }

    fn channel(&self) -> &str {
        &self.channel
    }

    fn capabilities(&self) -> BusCapabilities {
        // 硬件支持 CAN-FD，但本后端只按经典帧路径收发：能力位要报实现能给出
        // 的，不报硬件手册上写着的。FD 帧收发在 M5-09。
        BusCapabilities::CLASSIC_SOFT_TIMESTAMP
    }

    async fn send(&self, frame: CanFrame) -> Result<(), BusError> {
        #[cfg(windows)]
        {
            return self.handle.send(&frame);
        }
        #[cfg(not(windows))]
        {
            let _ = frame;
            Err(BusError::Unsupported {
                kind: "周立功后端（仅 Windows 可用）",
            })
        }
    }

    async fn recv(&self, timeout: Duration) -> Result<CanFrame, BusError> {
        #[cfg(windows)]
        {
            return self.handle.recv(timeout);
        }
        #[cfg(not(windows))]
        {
            let _ = timeout;
            Err(BusError::Unsupported {
                kind: "周立功后端（仅 Windows 可用）",
            })
        }
    }
}

/// 本机没有这个后端时的故障。措辞要给下一步，而不是只说"不支持"。
fn unavailable(channel: &str) -> Fault {
    Fault::new(
        FaultCode::new(Domain::Bus, 40).expect("序号 40 合法"),
        Severity::Error,
        format!("{channel}: 周立功后端只在 Windows 上可用（zlgcan.dll 与驱动只发布在 Windows）"),
    )
    .with_context("backend", BACKEND.to_owned())
    .with_context("channel", channel.to_owned())
    .with_hint("用 socketcan 通道，或在 Windows 上启动 daemon")
}

/// 标准帧与扩展帧的标识符上限。超出即拒绝，别让厂商库去截断。
pub const STD_ID_MAX: u32 = 0x7FF;

/// 把 `CanId` 翻成厂商库要的 `(id, is_extended)`。
pub fn wire_id(id: CanId) -> Option<(u32, bool)> {
    match id {
        CanId::Standard(raw) => (u32::from(raw) <= STD_ID_MAX).then_some((u32::from(raw), false)),
        CanId::Extended(raw) => Some((raw & 0x1FFF_FFFF, true)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channel_names_resolve_to_indices() {
        assert_eq!(ZlgBus::parse_channel("zlg:0"), Some(0));
        assert_eq!(ZlgBus::parse_channel("zlg:2"), Some(2));
        assert_eq!(ZlgBus::parse_channel("zlgbus1"), Some(1));
        assert_eq!(ZlgBus::parse_channel("3"), Some(3));
        assert_eq!(ZlgBus::parse_channel("zlg:"), Some(0), "空下标按 0");
        assert_eq!(ZlgBus::parse_channel("vcan0"), None, "别人的通道不猜");
    }

    #[test]
    fn fd_is_on_when_the_two_bitrates_differ() {
        let classic = ZlgConfig {
            data_bitrate: 500_000,
            ..ZlgConfig::default_for(0)
        };
        assert!(!classic.can_fd());
        assert!(ZlgConfig::default_for(0).can_fd(), "500k/2M 是 FD");
    }

    #[test]
    fn capabilities_report_what_the_backend_can_do_not_the_datasheet() {
        // 硬件支持 FD，但本后端只走经典帧：能力位报实现能给的。
        let capabilities = BusCapabilities::CLASSIC_SOFT_TIMESTAMP;
        assert!(capabilities.classic_can());
        assert!(!capabilities.can_fd());
    }

    #[test]
    fn identifiers_outside_eleven_bits_are_refused_on_the_standard_branch() {
        assert_eq!(
            wire_id(CanId::standard(0x7FF).expect("合法")),
            Some((0x7FF, false))
        );
        // Extended 走另一支，标准分支不会把 0x800 当成标准帧。
        assert_eq!(
            wire_id(CanId::Extended(0x800)),
            Some((0x800, true)),
            "扩展帧走另一支"
        );
    }

    #[cfg(not(windows))]
    #[test]
    fn a_non_windows_host_gets_an_actionable_fault() {
        let fault = ZlgBus::open("zlg:0", ZlgConfig::default_for(0)).expect_err("本机没有该后端");
        assert_eq!(fault.code().domain(), Domain::Bus);
        assert!(fault.message().contains("只在 Windows"), "{fault}");
        let hint = fault.hint().unwrap_or_default();
        assert!(hint.contains("socketcan"), "提示要给出替代路径：{fault}");
    }

    #[cfg(not(windows))]
    #[tokio::test]
    async fn closing_time_operations_fail_with_the_same_reason() {
        // 即便绕过 open（例如装配层直接持有后端），收发也必须说清原因，
        // 不能返回一个看起来正常的空帧。
        let bus = ZlgBus {
            channel: "zlg:0".to_owned(),
        };
        let error = bus
            .send(CanFrame::new(CanId::standard(0x600).expect("合法"), vec![1]).expect("合法"))
            .await
            .expect_err("本机没有该后端");
        assert!(
            format!("{error}").contains("仅 Windows"),
            "错误要说清平台限制：{error}"
        );
    }
}
