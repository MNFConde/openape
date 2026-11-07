//! 字节串与十六进制文本互转。
//!
//! dataset 与审计日志都要把原始字节写成人能读、能 diff 的文本，两处共用
//! 这一份实现——各写一份迟早一处接受大写一处不接受。

/// 编码为小写十六进制，无前缀、无分隔。
pub fn encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(char::from_digit(u32::from(byte >> 4), 16).unwrap_or('0'));
        out.push(char::from_digit(u32::from(byte & 0x0F), 16).unwrap_or('0'));
    }
    out
}

/// 解析十六进制。奇数长度或含非十六进制字符返回 `None`——半个字节没有含义。
pub fn decode(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) {
        return None;
    }
    text.as_bytes()
        .chunks(2)
        .map(|pair| {
            let high = char::from(pair[0]).to_digit(16)?;
            let low = char::from(pair[1]).to_digit(16)?;
            u8::try_from(high * 16 + low).ok()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_lowercase_hex() {
        assert_eq!(encode(&[0x00, 0xFF, 0x10]), "00ff10");
        assert_eq!(decode("00ff10"), Some(vec![0x00, 0xFF, 0x10]));
        assert_eq!(decode("00FF10"), Some(vec![0x00, 0xFF, 0x10]), "大写也接受");
    }

    #[test]
    fn rejects_odd_length_and_non_hex() {
        assert_eq!(decode("abc"), None);
        assert_eq!(decode("zz"), None);
    }

    #[test]
    fn empty_is_empty() {
        assert_eq!(encode(&[]), "");
        assert_eq!(decode(""), Some(Vec::new()));
    }
}
