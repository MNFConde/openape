//! XCP-on-CAN master。帧编解码与从 `bus` 收发之间的胶水。

use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use openape_bus::{Bus, BusError, CanFrame, CanId};
use openape_diag::{Fault, Severity};
use openape_xcp_proto::{Command, DecodeError, ErrorCode, MAX_CTO, PID_CMD_RES, Response};

use crate::connection::{self, ConnectInfo, SessionStatus, XcpConnection, seq};

/// 会话参数。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct XcpConfig {
    /// 命令帧标识符（CRO）。
    pub command_id: CanId,
    /// 响应帧标识符（DTO/响应）。
    pub response_id: CanId,
    /// 单次等待响应的时限。
    pub timeout: Duration,
    /// 超时后的重发次数。
    pub retries: u8,
}

impl XcpConfig {
    /// 常用默认值：`0x600` / `0x601` 与 M0-2 spike 一致，超时 200 ms，重试 2 次。
    pub fn new(command_id: CanId, response_id: CanId) -> Self {
        Self {
            command_id,
            response_id,
            timeout: Duration::from_millis(200),
            retries: 2,
        }
    }
}

/// 基于 CAN 的 XCP master。
pub struct CanXcpMaster {
    bus: Arc<dyn Bus>,
    config: XcpConfig,
    connected: bool,
}

impl CanXcpMaster {
    /// 新建。建链前 `read` 会被拒绝。
    pub fn new(bus: Arc<dyn Bus>, config: XcpConfig) -> Self {
        Self {
            bus,
            config,
            connected: false,
        }
    }

    /// 当前配置。
    pub fn config(&self) -> &XcpConfig {
        &self.config
    }

    /// 发一条命令、等一条响应，返回响应参数（已剥 PID 与命令码）。
    ///
    /// 等待期间收到其它标识符的帧（例如 DAQ 的 DTO）直接丢弃——采集与
    /// 命令在同一条总线上并存，不能因为来了数据帧就判命令失败。
    async fn transact(&self, command: &Command) -> Result<Vec<u8>, Fault> {
        let code = command.code() as u8;
        let payload = command
            .encode()
            .map_err(|err| connection::fault(seq::CODEC, Severity::Error, err.to_string()))?;

        let mut data = Vec::with_capacity(MAX_CTO);
        data.push(PID_CMD_RES);
        data.extend_from_slice(&payload);
        data.resize(MAX_CTO, 0);
        let frame = CanFrame::new(self.config.command_id, data).ok_or_else(|| {
            connection::fault(seq::CODEC, Severity::Error, "命令帧长超过 CAN 上限")
        })?;

        for attempt in 0..=self.config.retries {
            self.bus
                .send(frame.clone())
                .await
                .map_err(|err| bus_fault(&err))?;

            let deadline = Instant::now() + self.config.timeout;
            loop {
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    break;
                }
                match self.bus.recv(remaining).await {
                    Ok(received) if received.id == self.config.response_id => {
                        // 同一标识符上可能混着 DAQ 的 DTO 与事件帧，它们不是
                        // 命令响应，跳过继续等——不能因为来了数据就判命令失败。
                        let pid = received.data.first().copied().unwrap_or(0);
                        if pid != openape_xcp_proto::PID_CMD_RES
                            && pid != openape_xcp_proto::PID_ERR
                        {
                            continue;
                        }
                        return match Response::decode(&received.data) {
                            Ok(Response::Ok {
                                command: echoed,
                                payload,
                            }) => {
                                if echoed != code {
                                    Err(connection::fault(
                                        seq::PROTOCOL,
                                        Severity::Error,
                                        format!(
                                            "响应回显命令码 {echoed:#04x}，与请求 {code:#04x} 不符"
                                        ),
                                    ))
                                } else {
                                    Ok(payload)
                                }
                            }
                            Ok(Response::Err { code }) => Err(slave_error(code)),
                            Err(err) => Err(decode_fault(err)),
                        };
                    }
                    Ok(_) => continue,
                    Err(BusError::Timeout { .. }) => break,
                    Err(err) => return Err(bus_fault(&err)),
                }
            }

            if attempt == self.config.retries {
                return Err(connection::fault(
                    seq::TIMEOUT,
                    Severity::Error,
                    format!(
                        "等待从站响应超时（{} ms × {} 次重发）",
                        self.config.timeout.as_millis(),
                        self.config.retries
                    ),
                )
                .with_context("command", format!("{code:#04x}"))
                .with_hint("确认从站已启动、通道与两侧标识符一致"));
            }
        }
        Err(connection::fault(seq::TIMEOUT, Severity::Error, "重试耗尽"))
    }
}

fn decode_fault(err: DecodeError) -> Fault {
    let message = err.to_string();
    connection::fault(seq::CODEC, Severity::Error, message)
        .with_hint("检查两侧 maxCto 与命令响应格式是否一致（标准要求回显命令码）")
}

fn slave_error(code: u8) -> Fault {
    let text = match ErrorCode::from_u8(code) {
        Some(known) => format!("从站返回错误 {code:#04x}（{known:?}）"),
        None => format!("从站返回未登记的错误码 {code:#04x}"),
    };
    connection::fault(seq::SLAVE_ERROR, Severity::Error, text)
        .with_hint("查看命令参数是否越界、目标段是否可写")
}

fn bus_fault(err: &BusError) -> Fault {
    let message = err.to_string();
    connection::fault(seq::BUS, Severity::Error, message).with_hint("先跑 `probe bus` 确认通道可用")
}

#[async_trait]
impl XcpConnection for CanXcpMaster {
    async fn connect(&mut self, mode: u8) -> Result<ConnectInfo, Fault> {
        let payload = self.transact(&Command::Connect { mode }).await?;
        self.connected = true;
        parse_connect_info(&payload)
    }

    async fn disconnect(&mut self) -> Result<(), Fault> {
        self.transact(&Command::Disconnect).await?;
        self.connected = false;
        Ok(())
    }

    async fn status(&mut self) -> Result<SessionStatus, Fault> {
        let payload = self.transact(&Command::GetStatus).await?;
        let running = payload.first().copied().unwrap_or(0) & 0x01 != 0;
        let protection = payload.get(2).copied().unwrap_or(0);
        Ok(SessionStatus {
            running,
            protection,
        })
    }
}

/// 解析 CONNECT 响应。CAN 上参数放不全时版本字段缺席，不臆造。
fn parse_connect_info(payload: &[u8]) -> Result<ConnectInfo, Fault> {
    // 帧定长 8 字节、短响应会被补零，所以"长度够"不代表"内容完整"。
    // 可靠判据是 maxCto 非零：真从站不可能报 0。
    if payload.len() < 6 {
        return Err(connection::fault(
            seq::PROTOCOL,
            Severity::Error,
            format!("CONNECT 响应只有 {} 字节，至少需要 6 字节", payload.len()),
        ));
    }
    let max_cto = u16::from_le_bytes([payload[2], payload[3]]);
    let max_dto = u16::from_le_bytes([payload[4], payload[5]]);
    if max_cto == 0 {
        return Err(connection::fault(
            seq::PROTOCOL,
            Severity::Error,
            "CONNECT 响应的 maxCto 为 0，说明响应被截断或格式不符",
        )
        .with_hint("确认从站按标准回的响应是 PID + 命令码 + 参数"));
    }
    let protocol_version = (payload.len() >= 8).then(|| (payload[6], payload[7]));
    let transport_version = (payload.len() >= 10).then(|| (payload[8], payload[9]));
    Ok(ConnectInfo {
        resource: payload[0],
        comm_mode_basic: payload[1],
        max_cto,
        max_dto,
        protocol_version,
        transport_version,
    })
}
