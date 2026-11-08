//! XCP-on-Ethernet 的 UDP 传输（M5-12）。
//!
//! 走 `Bus` trait 的形状：UDP 包被包成"无标识符差别的 CAN 帧"喂给现有
//! 的 master、采集泵与 DAQ 解码——三层零改动复用，XCP-on-Ethernet 与
//! XCP-on-CAN 是同一条代码路径。时序与 CAN 同构：DAQ 配置阶段采集泵
//! 还没起，命令响应独占收包；泵起之后 master 静默，不存在互吃。
//!
//! 包格式按无头模式（headerless）：UDP 载荷就是裸 XCP 消息，不加 CTR
//! 头——自研从站两侧一致；与第三方（XCPlite 等）互通时要按对端的头
//! 模式适配，这一步攒真实对端后做。CRO 会按 CAN 习惯补齐到 8 字节：
//! 尾部零字节不影响按定义消费参数的解码器，互通时若对端严格按包长
//! 解析，改这里的补齐即可。

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use openape_bus::{Bus, BusCapabilities, BusError, CanFrame, CanId};

/// 一条 XCP-on-Ethernet 的 UDP 传输，呈现成总线视图。
pub struct UdpXcpBus {
    socket: Arc<tokio::net::UdpSocket>,
    response_id: CanId,
}

impl UdpXcpBus {
    /// 连一个从站地址：本端随机端口，收包只认这个对端。
    pub async fn open(peer: SocketAddr, response_id: CanId) -> std::io::Result<Self> {
        let socket = tokio::net::UdpSocket::bind("0.0.0.0:0").await?;
        socket.connect(peer).await?;
        Ok(Self {
            socket: Arc::new(socket),
            response_id,
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
        BusCapabilities::CLASSIC_SOFT_TIMESTAMP
    }

    async fn send(&self, frame: CanFrame) -> Result<(), BusError> {
        let count = self
            .socket
            .send(&frame.data)
            .await
            .map_err(|err| BusError::Transport(err.to_string()))?;
        if count != frame.data.len() {
            return Err(BusError::Transport(format!(
                "CRO 只发出去 {count}/{} 字节",
                frame.data.len()
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
        CanFrame::new(self.response_id, buffer[..count].to_vec())
            .map(|frame| frame.with_timestamp(std::time::Instant::now()))
            .ok_or_else(|| BusError::Transport(format!("包长 {count} 超出帧上限")))
    }
}
