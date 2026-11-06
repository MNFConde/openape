//! Linux SocketCAN 后端。
//!
//! 经典 CAN 与 vcan 走同一条路径；CAN-FD 走 `CanFdSocket`，M1 只实现经典帧，
//! FD 在能力位上报为不支持，避免上层按 FD 组帧后被静默丢弃。

#![forbid(unsafe_code)]

use std::time::Duration;

use async_trait::async_trait;
use openape_bus::{Bus, BusCapabilities, BusError, CanFrame, CanId};
use socketcan::EmbeddedFrame as _;
use socketcan::tokio::CanSocket;

/// 后端名，用于故障上下文与设备锁。
pub const BACKEND: &str = "socketcan";

/// 一条 SocketCAN 通道。
#[derive(Debug)]
pub struct SocketCanBus {
    socket: CanSocket,
    channel: String,
}

impl SocketCanBus {
    /// 打开通道。通道不存在或无权限时返回 [`BusError::ChannelUnavailable`]。
    pub fn open(channel: impl Into<String>) -> Result<Self, BusError> {
        let channel = channel.into();
        let socket = CanSocket::open(&channel).map_err(|err| map_open_error(&channel, err))?;
        Ok(Self { socket, channel })
    }
}

/// 内核在接口不存在时返回 `ENODEV`，它不属于任何标准 `ErrorKind`。
const ENODEV: i32 = 19;

fn map_open_error(channel: &str, err: std::io::Error) -> BusError {
    use std::io::ErrorKind;
    let channel_missing = matches!(
        err.kind(),
        ErrorKind::NotFound | ErrorKind::PermissionDenied
    ) || err.raw_os_error() == Some(ENODEV);
    if channel_missing {
        BusError::ChannelUnavailable {
            channel: channel.to_owned(),
            detail: err.to_string(),
        }
    } else {
        BusError::Transport(err.to_string())
    }
}

#[async_trait]
impl Bus for SocketCanBus {
    fn backend(&self) -> &'static str {
        BACKEND
    }

    fn channel(&self) -> &str {
        &self.channel
    }

    fn capabilities(&self) -> BusCapabilities {
        // M1 走软时间戳路径：内核能提供硬件时间戳，但本后端尚未接入读取
        // 路径，故对外报 false。接上 read_frame_with_hw_timestamp 之后才改。
        BusCapabilities::CLASSIC_SOFT_TIMESTAMP
    }

    async fn send(&self, frame: CanFrame) -> Result<(), BusError> {
        let id = match frame.id {
            CanId::Standard(raw) => socketcan::CanId::standard(raw)
                .ok_or(BusError::Transport("标准帧标识符越界".into()))?,
            CanId::Extended(raw) => socketcan::CanId::extended(raw)
                .ok_or(BusError::Transport("扩展帧标识符越界".into()))?,
        };
        let raw = socketcan::CanFrame::new(id, &frame.data).ok_or(BusError::Unsupported {
            kind: "该载荷长度的经典 CAN 帧",
        })?;
        openape_diag::log::trace_frame(
            openape_diag::Direction::Tx,
            &self.channel,
            frame.id.raw(),
            &frame.data,
        );
        self.socket
            .write_frame(raw)
            .await
            .map_err(|err| BusError::Transport(err.to_string()))
    }

    async fn recv(&self, timeout: Duration) -> Result<CanFrame, BusError> {
        let raw = match tokio::time::timeout(timeout, self.socket.read_frame()).await {
            Err(_) => {
                return Err(BusError::Timeout {
                    waited_ms: timeout.as_millis() as u64,
                });
            }
            Ok(Err(err)) => return Err(BusError::Transport(err.to_string())),
            Ok(Ok(frame)) => frame,
        };

        let id = match raw.id() {
            socketcan::Id::Standard(sid) => CanId::standard(sid.as_raw())
                .ok_or(BusError::Transport("标准帧标识符越界".into()))?,
            socketcan::Id::Extended(eid) => CanId::extended(eid.as_raw())
                .ok_or(BusError::Transport("扩展帧标识符越界".into()))?,
        };
        openape_diag::log::trace_frame(
            openape_diag::Direction::Rx,
            &self.channel,
            id.raw(),
            raw.data(),
        );
        let frame = CanFrame::new(id, raw.data())
            .ok_or(BusError::Unsupported {
                kind: "该载荷长度的经典 CAN 帧",
            })?
            .with_timestamp(std::time::Instant::now());
        Ok(frame)
    }
}

/// ARPHRD_CAN：`/sys/class/net/<name>/type` 里 CAN 设备的类型号。
const ARPHRD_CAN: &str = "280";

/// 列出本机可用的 CAN 通道，按名字排序。
///
/// 走 `/sys/class/net` 而不是 udev：前者零依赖，也不受特性开关影响。
/// 读不到 sysfs 时返回空列表——"没有通道"本身就是探测结果，不是错误。
pub fn available_channels() -> Vec<String> {
    let mut channels: Vec<String> = std::fs::read_dir("/sys/class/net")
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| {
            std::fs::read_to_string(format!("/sys/class/net/{name}/type"))
                .is_ok_and(|kind| kind.trim() == ARPHRD_CAN)
        })
        .collect();
    channels.sort();
    channels
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capabilities_do_not_claim_fd() {
        // 能力位必须与实际发送路径一致：M1 只发经典帧。
        let caps = BusCapabilities::CLASSIC_SOFT_TIMESTAMP;
        assert!(caps.classic_can());
        assert!(!caps.can_fd());
    }

    #[test]
    fn missing_channel_maps_to_channel_unavailable() {
        let err = SocketCanBus::open("nosuchif0").expect_err("通道不存在");
        assert!(
            matches!(err, BusError::ChannelUnavailable { .. }),
            "实际得到: {err:?}"
        );
    }
}
