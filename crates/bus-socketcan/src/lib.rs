//! Linux SocketCAN 后端。
//!
//! 经典 CAN 与 vcan 走同一条路径；接口 MTU ≥ [`CANFD_MTU`] 时开 FD 模式的
//! socket。一个 socket 收两种帧形态（FD socket 兼容经典帧），不设第二个
//! 接收队列——两个队列之间的到达顺序无法保证，采集泵就得多做归并。

#![forbid(unsafe_code)]

pub mod testing;

use std::future::Future;
use std::time::Duration;

use async_trait::async_trait;
use openape_bus::{Bus, BusCapabilities, BusError, CanFrame, CanId, FrameKinds};
use socketcan::id::FdFlags;
use socketcan::tokio::{CanFdSocket, CanSocket};
use socketcan::{CanAnyFrame, CanFdFrame, EmbeddedFrame as _};

/// 后端名，用于故障上下文与设备锁。
pub const BACKEND: &str = "socketcan";

/// CAN-FD 接口的 MTU。内核在发送路径上按 `dev->mtu >= CANFD_MTU` 放行 FD
/// 帧，能力探测用同一条判据，避免"报了 FD 但内核拒发"。
pub const CANFD_MTU: u32 = 72;

/// 经典帧的载荷上限。超过它的帧只能是 FD 帧。
const CLASSIC_MAX_DATA_LEN: usize = 8;

/// 一条通道的接收队列。形态在打开时定死：能力位与发送路径必须一致。
#[derive(Debug)]
enum Link {
    Classic(CanSocket),
    Fd(CanFdSocket),
}

/// 一条 SocketCAN 通道。
#[derive(Debug)]
pub struct SocketCanBus {
    link: Link,
    channel: String,
}

impl SocketCanBus {
    /// 打开通道。通道不存在或无权限时返回 [`BusError::ChannelUnavailable`]。
    pub fn open(channel: impl Into<String>) -> Result<Self, BusError> {
        let channel = channel.into();
        let link = open_link(&channel)?;
        Ok(Self { link, channel })
    }
}

fn open_link(channel: &str) -> Result<Link, BusError> {
    let map = |err| map_open_error(channel, err);
    if fd_capable(channel) {
        CanFdSocket::open(channel).map(Link::Fd).map_err(map)
    } else {
        CanSocket::open(channel).map(Link::Classic).map_err(map)
    }
}

/// 接口能不能发 FD 帧。判据与内核发送路径一致：`dev->mtu >= CANFD_MTU`。
/// 读不到 MTU 时按经典帧处理——宁可少报能力，不报假能力。
fn fd_capable(channel: &str) -> bool {
    std::fs::read_to_string(format!("/sys/class/net/{channel}/mtu"))
        .ok()
        .and_then(|raw| raw.trim().parse::<u32>().ok())
        .is_some_and(|mtu| mtu >= CANFD_MTU)
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

fn transport(err: std::io::Error) -> BusError {
    BusError::Transport(err.to_string())
}

/// 在期限内等一个结果。超时与传输错误走同一条出口，两个分支因此只需一句。
async fn within<T>(
    timeout: Duration,
    fut: impl Future<Output = Result<T, BusError>>,
) -> Result<T, BusError> {
    match tokio::time::timeout(timeout, fut).await {
        Err(_) => Err(BusError::Timeout {
            waited_ms: timeout.as_millis() as u64,
        }),
        Ok(result) => result,
    }
}

fn to_can_id(id: socketcan::Id) -> Result<CanId, BusError> {
    match id {
        socketcan::Id::Standard(sid) => {
            CanId::standard(sid.as_raw()).ok_or(BusError::Transport("标准帧标识符越界".into()))
        }
        socketcan::Id::Extended(eid) => {
            CanId::extended(eid.as_raw()).ok_or(BusError::Transport("扩展帧标识符越界".into()))
        }
    }
}

fn wire_id(id: CanId) -> Result<socketcan::CanId, BusError> {
    match id {
        CanId::Standard(raw) => {
            socketcan::CanId::standard(raw).ok_or(BusError::Transport("标准帧标识符越界".into()))
        }
        CanId::Extended(raw) => {
            socketcan::CanId::extended(raw).ok_or(BusError::Transport("扩展帧标识符越界".into()))
        }
    }
}

/// 收到的一帧：挂软时间戳，长度越界的拒收。
fn assemble(id: CanId, data: &[u8]) -> Result<CanFrame, BusError> {
    CanFrame::new(id, data)
        .map(|frame| frame.with_timestamp(std::time::Instant::now()))
        .ok_or(BusError::Unsupported {
            kind: "该载荷长度的 CAN 帧",
        })
}

fn classic_wire(frame: &CanFrame) -> Result<socketcan::CanFrame, BusError> {
    socketcan::CanFrame::new(wire_id(frame.id)?, &frame.data).ok_or(BusError::Unsupported {
        kind: "该载荷长度的经典 CAN 帧",
    })
}

/// FD 帧不带 BRS：是否切波特率是上层的事，后端只负责帧形态。
fn fd_wire(frame: &CanFrame) -> Result<CanFdFrame, BusError> {
    CanFdFrame::with_flags(wire_id(frame.id)?, &frame.data, FdFlags::empty()).ok_or(
        BusError::Unsupported {
            kind: "该载荷长度的 CAN-FD 帧",
        },
    )
}

/// FD 队列上的下一帧。远程帧与错误帧不是数据帧，跳过继续等（期限由外层管）。
async fn next_any_frame(socket: &CanFdSocket) -> Result<CanFrame, BusError> {
    loop {
        match socket.read_frame().await.map_err(transport)? {
            CanAnyFrame::Normal(raw) => return assemble(to_can_id(raw.id())?, raw.data()),
            CanAnyFrame::Fd(raw) => return assemble(to_can_id(raw.id())?, raw.data()),
            CanAnyFrame::Remote(_) | CanAnyFrame::Error(_) => continue,
        }
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
        match self.link {
            // M1 起走软时间戳路径：内核能提供硬件时间戳，但本后端尚未接入
            // 读取路径，故对外报 false。接上 read_frame_with_hw_timestamp 之后才改。
            Link::Classic(_) => BusCapabilities::CLASSIC_SOFT_TIMESTAMP,
            // FD 链路上短帧仍按经典帧宽补齐：帧形态由长度决定，补齐不改形态。
            Link::Fd(_) => BusCapabilities {
                backends: FrameKinds::BOTH,
                ..BusCapabilities::CLASSIC_SOFT_TIMESTAMP
            },
        }
    }

    async fn send(&self, frame: CanFrame) -> Result<(), BusError> {
        openape_diag::log::trace_frame(
            openape_diag::Direction::Tx,
            &self.channel,
            frame.id.raw(),
            &frame.data,
        );
        match &self.link {
            Link::Classic(socket) => socket
                .write_frame(classic_wire(&frame)?)
                .await
                .map_err(transport),
            Link::Fd(socket) if frame.len() > CLASSIC_MAX_DATA_LEN => socket
                .write_frame(&fd_wire(&frame)?)
                .await
                .map_err(transport),
            Link::Fd(socket) => socket
                .write_frame(&classic_wire(&frame)?)
                .await
                .map_err(transport),
        }
    }

    async fn recv(&self, timeout: Duration) -> Result<CanFrame, BusError> {
        let frame = match &self.link {
            Link::Classic(socket) => {
                let raw = within(timeout, async {
                    socket.read_frame().await.map_err(transport)
                })
                .await?;
                assemble(to_can_id(raw.id())?, raw.data())?
            }
            Link::Fd(socket) => within(timeout, next_any_frame(socket)).await?,
        };
        openape_diag::log::trace_frame(
            openape_diag::Direction::Rx,
            &self.channel,
            frame.id.raw(),
            &frame.data,
        );
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
    fn missing_channel_maps_to_channel_unavailable() {
        let err = SocketCanBus::open("nosuchif0").expect_err("通道不存在");
        assert!(
            matches!(err, BusError::ChannelUnavailable { .. }),
            "实际得到: {err:?}"
        );
    }

    #[test]
    fn unreadable_channel_does_not_claim_fd() {
        // 读不到 MTU（通道不存在）时按经典帧处理：宁可少报，不报假能力。
        assert!(!fd_capable("nosuchif0"));
    }
}
