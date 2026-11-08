//! XCP-on-Ethernet 的 UDP 传输（M5-12）。
//!
//! 走 `Bus` trait 的形状：UDP 包被包成"无标识符差别的 CAN 帧"喂给现有
//! 的 master、采集泵与 DAQ 解码——三层零改动复用，XCP-on-Ethernet 与
//! XCP-on-CAN 是同一条代码路径。时序与 CAN 同构：DAQ 配置阶段采集泵
//! 还没起，命令响应独占收包；泵起之后 master 静默，不存在互吃。
//!
//! 包格式两种（[`HeaderMode`]）：无头模式自研从站两侧一致；CTR 头模
//! 式是 XCPlite 等第三方实现的默认。头的编解码在 `xcp-proto` 一处
//! （[`openape_xcp_proto::ethernet`]），两侧共用。CRO 会按 CAN 习惯补
//! 齐到 8 字节：尾部零字节不影响按定义消费参数的解码器，互通时若对
//! 端严格按包长解析，改这里的补齐即可。

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU16, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use openape_bus::{Bus, BusCapabilities, BusError, CanFrame, CanId};
pub use openape_xcp_proto::HeaderMode;

/// 一条 XCP-on-Ethernet 的 UDP 传输，呈现成总线视图。
pub struct UdpXcpBus {
    socket: Arc<tokio::net::UdpSocket>,
    response_id: CanId,
    mode: HeaderMode,
    /// 出向 CTR 头的计数器：只增不校验，给对端抓包时对序用。
    counter: AtomicU16,
}

impl UdpXcpBus {
    /// 连一个从站地址：本端随机端口，收包只认这个对端。
    pub async fn open(
        peer: SocketAddr,
        response_id: CanId,
        mode: HeaderMode,
    ) -> std::io::Result<Self> {
        let socket = tokio::net::UdpSocket::bind("0.0.0.0:0").await?;
        socket.connect(peer).await?;
        Ok(Self {
            socket: Arc::new(socket),
            response_id,
            mode,
            counter: AtomicU16::new(0),
        })
    }

    /// 对端地址。
    pub fn peer(&self) -> std::io::Result<SocketAddr> {
        self.socket.peer_addr()
    }
}

#[async_trait]
impl Bus for UdpXcpBus {
    fn backend(&self) -> &'static str {
        "udp"
    }

    fn channel(&self) -> &str {
        "xcp-udp"
    }

    fn capabilities(&self) -> BusCapabilities {
        // UDP 按声明长度收帧：不定宽、不补齐。
        BusCapabilities::CLASSIC_SOFT_TIMESTAMP_UNPADDED
    }

    async fn send(&self, frame: CanFrame) -> Result<(), BusError> {
        let counter = self.counter.fetch_add(1, Ordering::Relaxed);
        let packet = self.mode.wrap(&frame.data, counter);
        let count = self
            .socket
            .send(&packet)
            .await
            .map_err(|err| BusError::Transport(err.to_string()))?;
        if count != packet.len() {
            return Err(BusError::Transport(format!(
                "CRO 只发出去 {count}/{} 字节",
                packet.len()
            )));
        }
        Ok(())
    }

    async fn recv(&self, timeout: Duration) -> Result<CanFrame, BusError> {
        let mut buffer = vec![0u8; 2048];
        let received = tokio::time::timeout(timeout, self.socket.recv(&mut buffer)).await;
        let count = match received {
            Ok(Ok(count)) => count,
            Ok(Err(err)) => return Err(BusError::Transport(err.to_string())),
            Err(_) => {
                return Err(BusError::Timeout {
                    waited_ms: timeout.as_millis() as u64,
                });
            }
        };
        // UDP 上命令响应与 DTO 同端口到达且没有 CAN 标识符：统一按响应
        // 标识符呈现，靠 PID 区分——master 的 await_answer 与 DAQ 解码
        // 各自的过滤在 CAN 上就是这么干的，这里原样复用。
        let message = self
            .mode
            .unwrap(&buffer[..count])
            .map_err(BusError::Transport)?;
        CanFrame::new(self.response_id, message.to_vec())
            .map(|frame| frame.with_timestamp(std::time::Instant::now()))
            .ok_or_else(|| BusError::Transport(format!("包长 {count} 超出帧上限")))
    }
}
