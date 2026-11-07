//! 最小编排引擎：装配总线、建链、读。
//!
//! 这是唯一指名具体后端 crate 的位置——L4 的 `assembly` 在 M1 空档，装配
//! 暂由本 crate 兼任（见 `docs/11-architecture-layers.md` 的后置项表）。

use std::sync::Arc;
use std::time::Duration;

use openape_bus::{Bus, BusCapabilities, CanFrame, CanId};
use openape_bus_socketcan::SocketCanBus;
use openape_cal_core::{Address, ByteLen};
use openape_diag::{Domain, Fault, FaultCode, Severity};
use openape_xcp::{CanXcpMaster, ConnectInfo, DaqLayout, DaqListSpec, XcpConfig, XcpConnection};

use crate::capture::{self, CaptureConfig, DaqCapture};
use crate::lock::DeviceLock;

/// 会话参数。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionConfig {
    /// 通道名，例如 `vcan0`。
    pub channel: String,
    /// 命令帧标识符。
    pub command_id: CanId,
    /// 响应帧标识符。
    pub response_id: CanId,
    /// 单次等待响应的时限。
    pub timeout: Duration,
    /// 超时重试次数。
    pub retries: u8,
}

impl SessionConfig {
    /// 只给通道时的默认值：`0x600` / `0x601`、200 ms、重试 2 次。
    pub fn new(channel: impl Into<String>) -> Self {
        Self {
            channel: channel.into(),
            command_id: CanId::standard(0x600).expect("0x600 在 11 位内"),
            response_id: CanId::standard(0x601).expect("0x601 在 11 位内"),
            timeout: Duration::from_millis(200),
            retries: 2,
        }
    }
}

/// 一次打开的会话。丢弃即释放设备锁。
pub struct Session {
    bus: Arc<dyn Bus>,
    master: CanXcpMaster,
    channel: String,
    _lock: DeviceLock,
}

impl std::fmt::Debug for Session {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Session")
            .field("channel", &self.channel)
            .field("lock", &self._lock.path().display().to_string())
            .finish_non_exhaustive()
    }
}

impl Session {
    /// 拿设备锁、打开总线。没有 `async` 需求：socket 打开是同步的。
    pub fn open(config: SessionConfig) -> Result<Self, Fault> {
        let lock = DeviceLock::acquire(openape_bus_socketcan::BACKEND, &config.channel)?;
        let bus: Arc<dyn Bus> = Arc::new(SocketCanBus::open(config.channel.clone())?);
        let mut xcp_config = XcpConfig::new(config.command_id, config.response_id);
        xcp_config.timeout = config.timeout;
        xcp_config.retries = config.retries;
        let master = CanXcpMaster::new(bus.clone(), xcp_config);
        Ok(Self {
            bus,
            master,
            channel: config.channel,
            _lock: lock,
        })
    }

    /// 通道名。
    pub fn channel(&self) -> &str {
        &self.channel
    }

    /// 后端能力。上层据此降级。
    pub fn capabilities(&self) -> BusCapabilities {
        self.bus.capabilities()
    }

    /// 建链。
    pub async fn connect(&mut self, mode: u8) -> Result<ConnectInfo, Fault> {
        self.master.connect(mode).await
    }

    /// 读一段内存。
    pub async fn read(&mut self, address: Address, len: ByteLen) -> Result<Vec<u8>, Fault> {
        self.master.read(address, len).await
    }

    /// 配置并启动 DAQ。从站随后按事件通道周期发帧。
    pub async fn configure_daq(&mut self, lists: &[DaqListSpec]) -> Result<DaqLayout, Fault> {
        self.master.configure_daq(lists).await
    }

    /// 起采集泵：把总线上的 DTO 帧推进样本缓冲。
    ///
    /// 采集期间本会话是总线的唯一读者，命令事务要让路（[`crate::capture`] 的
    /// 模块文档说明了原因）。
    pub fn start_capture(&self, layout: DaqLayout) -> DaqCapture {
        self.start_capture_with(layout, CaptureConfig::default())
    }

    /// 同 [`Self::start_capture`]，可指定缓冲容量。
    pub fn start_capture_with(&self, layout: DaqLayout, config: CaptureConfig) -> DaqCapture {
        capture::spawn(
            Arc::clone(&self.bus),
            self.master.config().response_id,
            layout,
            &config,
        )
    }

    /// 断链。
    pub async fn disconnect(&mut self) -> Result<(), Fault> {
        self.master.disconnect().await
    }

    /// 往通道发一帧裸 CAN 帧，供探针与测试注入。
    pub async fn send_raw(&self, frame: CanFrame) -> Result<(), Fault> {
        self.bus.send(frame).await.map_err(|err| {
            Fault::new(
                FaultCode::new(Domain::Session, 3).expect("序号 3 合法"),
                Severity::Error,
                err.to_string(),
            )
        })
    }
}

/// 通道探测结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelProbe {
    /// 通道名。
    pub channel: String,
    /// 后端名。
    pub backend: &'static str,
    /// 后端能力。
    pub capabilities: BusCapabilities,
}

/// 探测一条通道：能打开即报能力，打不开即故障。
///
/// 探测同样要拿设备锁——独占的硬件不该被探测临时抢走。
pub fn probe_channel(channel: &str) -> Result<ChannelProbe, Fault> {
    let _lock = DeviceLock::acquire(openape_bus_socketcan::BACKEND, channel)?;
    let bus = SocketCanBus::open(channel)?;
    Ok(ChannelProbe {
        channel: channel.to_owned(),
        backend: openape_bus_socketcan::BACKEND,
        capabilities: bus.capabilities(),
    })
}

/// 本机可用的 CAN 通道。
pub fn list_channels() -> Vec<String> {
    openape_bus_socketcan::available_channels()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_channel_is_reported_before_anything_else() {
        let config = SessionConfig::new("nosuchif0");
        let err = Session::open(config).expect_err("通道不存在");
        assert_eq!(err.code().domain(), Domain::Bus, "应归到总线域");
        assert!(err.hint().is_some());
    }

    #[test]
    fn probe_on_missing_channel_releases_the_lock() {
        let _ = probe_channel("nosuchif1").expect_err("不存在");
        // 失败路径不能把锁留下，否则后续探测永远撞锁。
        let _ = probe_channel("nosuchif1").expect_err("仍然不存在，但不能是锁冲突");
        let err = probe_channel("nosuchif1").expect_err("再试一次");
        assert!(
            !err.message().contains("占用"),
            "失败后不该残留锁，实际: {err}"
        );
    }

    #[test]
    fn default_config_uses_spike_ids() {
        let config = SessionConfig::new("vcan0");
        assert_eq!(config.command_id, CanId::standard(0x600).expect("合法"));
        assert_eq!(config.response_id, CanId::standard(0x601).expect("合法"));
        assert_eq!(config.timeout, Duration::from_millis(200));
    }
}
