//! socketcand 客户端（M5-11）：把 linux-can 的 socketcand 守护进程
//! 当成一条总线。
//!
//! socketcand 通过一条 TCP 连接暴露 host 上的 SocketCAN 接口，协议是
//! ASCII 报文（`< 命令 参数 >`）。本客户端建连后即切 RAW 模式——总线
//! 上每帧立即可收，不需要 BCM 的订阅/过滤状态机，`Bus` 形状与
//! SocketCanBus 完全同构：采集泵、master 会话对后端零特判。
//!
//! 报文分帧靠 `< ... >` 括号而不是行尾：参考实现实测（0.6.1）问候
//! `< hi >` 不带换行，按行读会永远等不到。所有收包都走字节缓冲，
//! 抽出完整括号段再解析。
//!
//! 能力位按服务端如实上报：socketcand 的帧时间戳是内核接收时刻（软
//! 时间戳档），帧按实际字节呈现（不定宽、不补齐）。

#![forbid(unsafe_code)]

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use openape_bus::{Bus, BusCapabilities, BusError, CanFrame, CanId};
use tokio::io::{AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;

/// 握手每步的超时。挂死的握手比超时报错更难排查。
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

/// 一条经 socketcand 访问的总线。
pub struct SocketcandBus {
    reader: Arc<tokio::sync::Mutex<BufReader<tokio::net::tcp::OwnedReadHalf>>>,
    /// 还没消费的输入（括号段的碎片）。
    buffer: Arc<tokio::sync::Mutex<String>>,
    writer: Arc<tokio::sync::Mutex<tokio::net::tcp::OwnedWriteHalf>>,
    channel: String,
}

/// 从缓冲里抽出第一个完整 `< ... >` 段。没有完整段时返回 `None`。
fn extract_message(buffer: &mut String) -> Option<String> {
    let start = buffer.find("< ")?;
    let end = start + buffer[start..].find('>')? + 1;
    let message = buffer[start..end].to_owned();
    buffer.drain(..end);
    Some(message)
}

impl SocketcandBus {
    /// 连一个 socketcand 并打开通道（RAW 模式）。
    ///
    /// 握手的三步任何一步失败都直接报错：半打开的连接比不连接更难排查。
    pub async fn open(address: SocketAddr, channel: &str) -> Result<Self, BusError> {
        let connect = tokio::time::timeout(HANDSHAKE_TIMEOUT, TcpStream::connect(address));
        let stream = connect
            .await
            .map_err(|_| BusError::Transport(format!("连 {address} 超时")))?
            .map_err(|err| BusError::Transport(format!("连 {address} 失败：{err}")))?;
        let (read, mut write) = stream.into_split();
        let mut reader = BufReader::new(read);
        let mut buffer = String::new();

        // 问候报文 `< hi >`。
        let greeting = read_message(&mut reader, &mut buffer).await?;
        if greeting != "< hi >" {
            return Err(BusError::Transport(format!(
                "socketcand 问候报文不是 `< hi >`：{greeting}"
            )));
        }

        // 选总线。通道名上限 16 字符是协议的约束，超了会被拒。
        if channel.len() > 16 {
            return Err(BusError::Transport(format!(
                "通道名 {channel} 超过协议上限 16 字符"
            )));
        }
        write
            .write_all(format!("< open {channel} >").as_bytes())
            .await
            .map_err(|err| BusError::Transport(err.to_string()))?;
        let open_reply = read_message(&mut reader, &mut buffer).await?;
        if !open_reply.starts_with("< ok") {
            return Err(BusError::Transport(format!(
                "open {channel} 被拒：{open_reply}"
            )));
        }

        // 切 RAW 模式：每帧立即可收。回执在这里验证——模式切换没生效
        // 的话帧流永远不来，宁可握手时失败。
        write
            .write_all(b"< rawmode >")
            .await
            .map_err(|err| BusError::Transport(err.to_string()))?;
        let raw_reply = read_message(&mut reader, &mut buffer).await?;
        if !raw_reply.starts_with("< ok") {
            return Err(BusError::Transport(format!("切 RAW 模式被拒：{raw_reply}")));
        }

        Ok(Self {
            reader: Arc::new(tokio::sync::Mutex::new(reader)),
            buffer: Arc::new(tokio::sync::Mutex::new(buffer)),
            writer: Arc::new(tokio::sync::Mutex::new(write)),
            channel: channel.to_owned(),
        })
    }
}

/// 读到下一个完整 `< ... >` 段（带超时；连接断开即报错）。
async fn read_message(
    reader: &mut BufReader<tokio::net::tcp::OwnedReadHalf>,
    buffer: &mut String,
) -> Result<String, BusError> {
    let read = tokio::time::timeout(HANDSHAKE_TIMEOUT, async {
        loop {
            if let Some(message) = extract_message(buffer) {
                return Ok(message);
            }
            let mut chunk = [0u8; 1024];
            let count = reader
                .read(&mut chunk)
                .await
                .map_err(|err| BusError::Transport(format!("socketcand 连接断开：{err}")))?;
            if count == 0 {
                return Err(BusError::Closed);
            }
            buffer.push_str(&String::from_utf8_lossy(&chunk[..count]));
        }
    });
    match read.await {
        Ok(result) => result,
        Err(_) => Err(BusError::Transport("socketcand 应答超时".to_owned())),
    }
}

/// 解析 RAW 模式的帧报文：`< frame can_id seconds.useconds HEXDATA >`。
///
/// 数据是无空格大写十六进制（长度即 DLC 的两倍）；其它报文（统计、
/// 问候、模式回执）返回 `None` 跳过。ID 列超 3 个十六进制位按扩展帧收。
pub fn parse_frame_message(message: &str) -> Option<Result<CanFrame, BusError>> {
    let inner = message.strip_prefix("< ")?.strip_suffix('>')?.trim_end();
    let mut words = inner.split_whitespace();
    if words.next()? != "frame" {
        return None;
    }
    let id_text = words.next()?;
    let id_value = u32::from_str_radix(id_text, 16).ok()?;
    // 第二段是内核时间戳（秒.微秒），只用于排序，跳过。
    let _timestamp = words.next()?;
    let data = words.next().unwrap_or("");
    if !data.is_empty() && (data.len() % 2 != 0 || data.len() > 16) {
        return Some(Err(BusError::Transport(format!(
            "frame 报文的数据段 {data} 不是偶数长度或超出经典帧"
        ))));
    }
    let mut bytes = Vec::with_capacity(data.len() / 2);
    for index in (0..data.len()).step_by(2) {
        bytes.push(u8::from_str_radix(&data[index..index + 2], 16).ok()?);
    }
    let id = if id_text.len() > 3 {
        CanId::extended(id_value)?
    } else {
        CanId::standard(u16::try_from(id_value).ok()?)?
    };
    Some(
        CanFrame::new(id, bytes)
            .ok_or_else(|| BusError::Transport("frame 报文的数据段长度超出帧上限".to_owned())),
    )
}

/// 把一帧编成 `< send >` 报文（数据小写无填充、空格分隔）。
pub fn encode_send_message(frame: &CanFrame) -> Result<String, BusError> {
    if frame.data.is_empty() || frame.data.len() > 8 {
        return Err(BusError::Transport(format!(
            "socketcand 的 send 至少 1 字节、至多 8 字节，拿到 {}",
            frame.data.len()
        )));
    }
    let id = match frame.id {
        CanId::Standard(value) => format!("{value:X}"),
        CanId::Extended(value) => format!("{value:X}"),
    };
    let data = frame
        .data
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<Vec<_>>()
        .join(" ");
    Ok(format!("< send {id} {} {data} >", frame.data.len()))
}

#[async_trait]
impl Bus for SocketcandBus {
    fn backend(&self) -> &'static str {
        "socketcand"
    }

    fn channel(&self) -> &str {
        &self.channel
    }

    fn capabilities(&self) -> BusCapabilities {
        // 服务端的时间戳是内核接收时刻，软时间戳档；帧按实际字节
        // 呈现，不定宽、不补齐。
        BusCapabilities::CLASSIC_SOFT_TIMESTAMP_UNPADDED
    }

    async fn send(&self, frame: CanFrame) -> Result<(), BusError> {
        let message = encode_send_message(&frame)?;
        let mut writer = self.writer.lock().await;
        writer
            .write_all(message.as_bytes())
            .await
            .map_err(|err| BusError::Transport(err.to_string()))
    }

    async fn recv(&self, timeout: Duration) -> Result<CanFrame, BusError> {
        let mut reader = self.reader.lock().await;
        let mut buffer = self.buffer.lock().await;
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            if let Some(message) = extract_message(&mut buffer) {
                if let Some(parsed) = parse_frame_message(&message) {
                    return parsed;
                }
                // 统计、问候与模式回执都跳过——RAW 模式下它们只是夹
                // 在帧流里的噪声。
                continue;
            }
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                return Err(BusError::Timeout {
                    waited_ms: timeout.as_millis() as u64,
                });
            }
            let mut chunk = [0u8; 1024];
            let read = tokio::time::timeout(remaining, reader.read(&mut chunk)).await;
            let count = match read {
                Ok(Ok(count)) => count,
                Ok(Err(err)) => {
                    return Err(BusError::Transport(format!("socketcand 连接断开：{err}")));
                }
                Err(_) => {
                    return Err(BusError::Timeout {
                        waited_ms: timeout.as_millis() as u64,
                    });
                }
            };
            if count == 0 {
                return Err(BusError::Closed);
            }
            buffer.push_str(&String::from_utf8_lossy(&chunk[..count]));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use openape_bus::CanId;

    fn standard(id: u16, data: &[u8]) -> CanFrame {
        CanFrame::new(CanId::standard(id).expect("合法"), data.to_vec()).expect("帧合法")
    }

    #[test]
    fn messages_are_extracted_by_brackets_not_line_endings() {
        // 参考实现的问候不带换行——分帧只认括号。
        let mut buffer = "< hi >< open vcan0 >< ".to_owned();
        assert_eq!(extract_message(&mut buffer), Some("< hi >".to_owned()));
        assert_eq!(
            extract_message(&mut buffer),
            Some("< open vcan0 >".to_owned())
        );
        assert_eq!(extract_message(&mut buffer), None, "半截报文留在缓冲");
        buffer.push_str("ok >");
        assert_eq!(extract_message(&mut buffer), Some("< ok >".to_owned()));
    }

    #[test]
    fn frame_messages_parse_with_timestamp_skipped() {
        let parsed = parse_frame_message("< frame 123 23.424242 1A220344 >")
            .expect("是帧报文")
            .expect("解析成功");
        assert_eq!(parsed.id, CanId::standard(0x123).expect("合法"));
        assert_eq!(
            parsed.data,
            vec![0x1A, 0x22, 0x03, 0x44],
            "大写 hex 按序还原"
        );
    }

    #[test]
    fn non_frame_messages_are_skipped() {
        for message in ["< hi >", "< stat 1 2 3 4 >", "< ok >", "< bcmmode >"] {
            assert!(
                parse_frame_message(message).is_none(),
                "非帧报文：{message}"
            );
        }
    }

    #[test]
    fn extended_ids_and_broken_data_are_named() {
        let parsed = parse_frame_message("< frame 1AAAAAAA 0 1A >")
            .expect("是帧报文")
            .expect("扩展帧解析成功");
        assert!(matches!(parsed.id, CanId::Extended(_)));
        assert!(
            parse_frame_message("< frame 123 0 1 >").is_some(),
            "奇数长度报错"
        );
    }

    #[test]
    fn zero_length_frames_come_through_without_a_data_field() {
        let parsed = parse_frame_message("< frame 123 5.000001 >")
            .expect("是帧报文")
            .expect("零长帧解析成功");
        assert_eq!(parsed.data, Vec::<u8>::new(), "无数据段即 DLC 0");
    }

    #[test]
    fn send_messages_are_lowercase_unpadded_with_spaces() {
        let message = encode_send_message(&standard(0x123, &[0x1A, 0xF1, 0x02])).expect("合法");
        assert_eq!(message, "< send 123 3 1a f1 02 >");
    }

    #[test]
    fn empty_frames_are_refused_by_the_protocol() {
        // 协议的 send 至少带 1 字节数据；零长帧在这里没有合法编码。
        assert!(encode_send_message(&standard(0x123, &[])).is_err());
    }
}
