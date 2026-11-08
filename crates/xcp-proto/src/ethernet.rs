//! XCP-on-Ethernet 的包层（M5-12）：无头与 CTR 头两种包头模式。
//!
//! UDP 一包一条 XCP 消息。CTR 头模式在每个包前加 4 字节头——载荷长
//! 度（LE u16）+ 计数器（LE u16）——是 XCPlite 等第三方实现的默认；
//! 无头模式的载荷就是裸 XCP 消息。收端按长度头分帧，**计数器不校
//! 验**：对端逐类递增还是全局递增，不在接收假设里。

/// CTR 头的字节数：长度 2 + 计数器 2。
pub const CTR_HEADER_LEN: usize = 4;

/// UDP 包的包头模式。master 与从站必须同模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeaderMode {
    /// 载荷就是裸 XCP 消息。自研两侧一致的默认。
    Headerless,
    /// 每包前加 CTR 头。
    Headered,
}

impl HeaderMode {
    /// 按模式把一条 XCP 消息装进 UDP 载荷。计数器由调用方管理（带头的
    /// 模式下通常按包递增），这里只负责编码。
    pub fn wrap(&self, message: &[u8], counter: u16) -> Vec<u8> {
        match self {
            Self::Headerless => message.to_vec(),
            Self::Headered => {
                let mut packet = Vec::with_capacity(CTR_HEADER_LEN + message.len());
                packet.extend_from_slice(&(message.len() as u16).to_le_bytes());
                packet.extend_from_slice(&counter.to_le_bytes());
                packet.extend_from_slice(message);
                packet
            }
        }
    }

    /// 按模式从 UDP 载荷里取出 XCP 消息。长度头与实际载荷不符时给
    /// 人话错误——多半是对端包头模式不符或包坏了，点名不猜。
    pub fn unwrap<'a>(&self, packet: &'a [u8]) -> Result<&'a [u8], String> {
        match self {
            Self::Headerless => Ok(packet),
            Self::Headered => {
                let head = packet
                    .get(..2)
                    .and_then(|raw| <[u8; 2]>::try_from(raw).ok())
                    .ok_or_else(|| "CTR 头不完整".to_owned())?;
                let payload_len = u16::from_le_bytes(head) as usize;
                let rest = packet
                    .get(CTR_HEADER_LEN..)
                    .ok_or_else(|| "CTR 头长超出包边界".to_owned())?;
                if payload_len != rest.len() {
                    return Err(format!(
                        "CTR 头报长 {payload_len}，包里载荷 {} 字节——对端包头模式不符？",
                        rest.len()
                    ));
                }
                Ok(rest)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn headerless_round_trip_is_the_bare_message() {
        let message = [0xFF, 0x00, 0x01];
        let packet = HeaderMode::Headerless.wrap(&message, 7);
        assert_eq!(packet, message, "无头模式的载荷就是消息本身");
        assert_eq!(HeaderMode::Headerless.unwrap(&packet), Ok(&message[..]));
    }

    #[test]
    fn headered_round_trip_carries_length_and_counter() {
        let message = [0xFF, 0x00];
        let packet = HeaderMode::Headered.wrap(&message, 42);
        assert_eq!(packet.len(), CTR_HEADER_LEN + message.len());
        assert_eq!(&packet[..2], &2u16.to_le_bytes(), "头的前两字节是载荷长");
        assert_eq!(&packet[2..4], &42u16.to_le_bytes(), "头的后两字节是计数器");
        assert_eq!(packet[4..], message);
        assert_eq!(HeaderMode::Headered.unwrap(&packet), Ok(&message[..]));
    }

    #[test]
    fn headered_unwrap_names_a_mode_mismatch_instead_of_guessing() {
        // 无头消息被按带头解：包短于头长，报边界错误。
        let short = HeaderMode::Headered
            .unwrap(&[0xFF, 0x00])
            .expect_err("头不完整");
        assert!(short.contains("包边界"), "{short}");
        // 长度头与实际载荷不符：报长与实长都点名——对端模式不符的指纹。
        let mut packet = HeaderMode::Headered.wrap(&[0xFF, 0x00], 1);
        packet[..2].copy_from_slice(&5u16.to_le_bytes());
        let error = HeaderMode::Headered.unwrap(&packet).expect_err("长度不符");
        assert!(error.contains("CTR 头报长"), "{error}");
        assert!(error.contains("对端包头模式不符"), "{error}");
    }
}
