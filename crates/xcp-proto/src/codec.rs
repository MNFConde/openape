//! XCP 参数的对齐编解码。
//!
//! XCP 规定参数按自身宽度对齐：读 `u16` 前先跳到 2 的倍数、`u32` 前跳到 4
//! 的倍数，master 确实会插填充字节（M0-2 实测，见 `cairn/xcp-on-can.md`
//! 第 1 条坑）。按固定偏移硬解析会整串错位。

use crate::DecodeError;

/// 解码器。偏移从命令码字节起算，与帧内绝对偏移一致。
pub(crate) struct Decoder<'a> {
    buf: &'a [u8],
    off: usize,
}

impl<'a> Decoder<'a> {
    pub(crate) fn new(buf: &'a [u8]) -> Self {
        Self { buf, off: 0 }
    }

    fn align(&mut self, width: usize) {
        while !self.off.is_multiple_of(width) {
            self.off += 1;
        }
    }

    pub(crate) fn u8(&mut self) -> Result<u8, DecodeError> {
        let byte = *self.buf.get(self.off).ok_or(DecodeError::Truncated {
            need: self.off + 1,
            got: self.buf.len(),
        })?;
        self.off += 1;
        Ok(byte)
    }

    pub(crate) fn u16(&mut self) -> Result<u16, DecodeError> {
        self.align(2);
        let bytes = self.take(2)?;
        Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
    }

    pub(crate) fn u32(&mut self) -> Result<u32, DecodeError> {
        self.align(4);
        let bytes = self.take(4)?;
        Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    pub(crate) fn take(&mut self, n: usize) -> Result<&'a [u8], DecodeError> {
        let end = self.off + n;
        if end > self.buf.len() {
            return Err(DecodeError::Truncated {
                need: end,
                got: self.buf.len(),
            });
        }
        let slice = &self.buf[self.off..end];
        self.off = end;
        Ok(slice)
    }

    /// 已消费到的偏移，用于测试对照字节布局。
    #[cfg(test)]
    pub(crate) fn offset(&self) -> usize {
        self.off
    }
}

/// 编码器。填充字节一律写 0。
#[derive(Debug, Default)]
pub(crate) struct Encoder {
    buf: Vec<u8>,
}

impl Encoder {
    /// 以命令码开头。
    pub(crate) fn with_code(code: u8) -> Self {
        Self { buf: vec![code] }
    }

    fn align(&mut self, width: usize) {
        while !self.buf.len().is_multiple_of(width) {
            self.buf.push(0);
        }
    }

    pub(crate) fn u8(mut self, value: u8) -> Self {
        self.buf.push(value);
        self
    }

    pub(crate) fn u16(mut self, value: u16) -> Self {
        self.align(2);
        self.buf.extend_from_slice(&value.to_le_bytes());
        self
    }

    pub(crate) fn u32(mut self, value: u32) -> Self {
        self.align(4);
        self.buf.extend_from_slice(&value.to_le_bytes());
        self
    }

    pub(crate) fn bytes(mut self, value: &[u8]) -> Self {
        self.buf.extend_from_slice(value);
        self
    }

    pub(crate) fn finish(self) -> Vec<u8> {
        self.buf
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn u32_is_aligned_to_four_after_a_single_byte() {
        // 与 spike 的 SET_MTA 帧一致：命令码 + ext 之后补 2 个填充字节。
        let frame = Encoder::with_code(0xF6).u8(0).u32(0x1000).finish();
        assert_eq!(frame, vec![0xF6, 0x00, 0x00, 0x00, 0x00, 0x10, 0x00, 0x00]);
    }

    #[test]
    fn u16_is_aligned_to_two() {
        let frame = Encoder::with_code(0xEB).u8(0).u8(0).u16(1).finish();
        assert_eq!(frame, vec![0xEB, 0x00, 0x00, 0x00, 0x01, 0x00]);
    }

    #[test]
    fn decoder_reads_aligned_fields_and_reports_offset() {
        let frame = [0xF6, 0x00, 0x00, 0x00, 0x00, 0x10, 0x00, 0x00];
        let mut decoder = Decoder::new(&frame);
        assert_eq!(decoder.u8().expect("命令码"), 0xF6);
        assert_eq!(decoder.u8().expect("扩展地址"), 0);
        assert_eq!(decoder.u32().expect("地址"), 0x1000);
        assert_eq!(decoder.offset(), 8);
    }

    #[test]
    fn truncated_input_reports_how_much_was_needed() {
        let mut decoder = Decoder::new(&[0xF6, 0x00, 0x00, 0x00]);
        let _ = decoder.u8().expect("命令码");
        let _ = decoder.u8().expect("扩展地址");
        let err = decoder.u32().expect_err("地址被截断");
        assert_eq!(err, DecodeError::Truncated { need: 8, got: 4 });
    }
}
