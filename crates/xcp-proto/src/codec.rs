//! XCP 参数的对齐编解码。
//!
//! 帧约定全部取自 M0-2 的实测抓帧（`tmp/spike-m0-2/can.log`，pyXCP 作
//! master）：
//!
//! - **CRO 帧 byte0 是命令码，没有 PID**。`F6 00 00 00 02 10 00 00` 是
//!   `SET_MTA(ext=0, addr=0x1002)`——命令码紧跟参数。
//! - **参数按整帧偏移对齐**：`u16` 前跳到 2 的倍数、`u32` 前跳到 4 的倍数，
//!   中间填 0。`ALLOC_DAQ` 因此是 `D5 00 01 00` 而不是 `D5 01 00`。
//! - 数值按**小端**排。
//!
//! 早期实现给 CRO 加了一个 `0xFF` 前缀，导致真 ECU 把 `0xF6` 当成未知 PID；
//! 自研 master 与自研从站双方自洽，测试全绿也发现不了。这份约定只有与第三方
//! master 对照才验得出，见 `cairn/xcp-on-can.md`。

use crate::DecodeError;

/// 解码器。偏移以帧内绝对位置计算，因为对齐基准就是帧起点。
pub(crate) struct Decoder<'a> {
    buf: &'a [u8],
    off: usize,
}

impl<'a> Decoder<'a> {
    /// 从整帧的某一偏移开始读。
    pub(crate) fn at(buf: &'a [u8], start: usize) -> Self {
        Self { buf, off: start }
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

    /// 已消费到的偏移，用于与实测字节布局对照。
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
    /// 从命令码起帧：CRO 没有 PID 前缀，命令码占 byte0，参数按整帧偏移对齐。
    pub(crate) fn with_command(code: u8) -> Self {
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

    // u16 与 u32 结构同构（对齐 + 小端 + 追加），dryer 报相似度 1.00。
    // **刻意保留**：要消掉得引入自定义 trait 或宏，为一个 4 方法的小工具造
    // 抽象层，比三行重复更难读。触发修改的条件（任一满足即改）：
    // 1. 出现第三种宽度（u64 时间戳）或 Decoder 侧也变成同构，重复份数到 4；
    // 2. 抽象后调用点仍保留语义（`enc.int(x)` 会丢宽度信息，`enc.u16(x)` 不会）；
    // 3. dryer 进 CI 门禁，逐条争论例外的成本高于写一次 trait。
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
    fn command_starts_at_byte_zero_without_pid() {
        let frame = Encoder::with_command(0xFD).finish();
        assert_eq!(frame, vec![0xFD], "CRO 没有 PID 前缀");
    }

    #[test]
    fn u32_is_aligned_to_four_from_the_frame_start() {
        // 实测：SET_MTA(ext=0, addr=0x1002) → F6 00 00 00 02 10 00 00
        let frame = Encoder::with_command(0xF6).u8(0).u32(0x1002).finish();
        assert_eq!(frame, vec![0xF6, 0x00, 0x00, 0x00, 0x02, 0x10, 0x00, 0x00]);
        assert_eq!(frame.len(), 8, "正好一帧");
    }

    #[test]
    fn u16_is_aligned_to_two() {
        // 实测：ALLOC_DAQ(1) → D5 00 01 00
        let frame = Encoder::with_command(0xD5).u16(1).finish();
        assert_eq!(frame, vec![0xD5, 0x00, 0x01, 0x00]);
    }

    #[test]
    fn decoder_reads_aligned_fields_and_reports_offset() {
        let frame = [0xF6, 0x00, 0x00, 0x00, 0x02, 0x10, 0x00, 0x00];
        let mut decoder = Decoder::at(&frame, 0);
        assert_eq!(decoder.u8().expect("命令码"), 0xF6);
        assert_eq!(decoder.u8().expect("扩展地址"), 0);
        assert_eq!(decoder.u32().expect("地址"), 0x1002);
        assert_eq!(decoder.offset(), 8);
    }

    #[test]
    fn truncated_input_reports_how_much_was_needed() {
        let frame = [0xF6, 0x00, 0x00, 0x00];
        let mut decoder = Decoder::at(&frame, 0);
        let _ = decoder.u8().expect("命令码");
        let _ = decoder.u8().expect("扩展地址");
        let err = decoder.u32().expect_err("地址被截断");
        assert_eq!(err, DecodeError::Truncated { need: 8, got: 4 });
    }
}
