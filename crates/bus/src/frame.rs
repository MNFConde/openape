//! CAN 帧与标识符。非法标识符在构造处被拒绝。

/// CAN 标识符。标准帧上限 `0x7FF`，扩展帧上限 `0x1FFF_FFFF`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum CanId {
    /// 11 位标识符。
    Standard(u16),
    /// 29 位标识符。
    Extended(u32),
}

impl CanId {
    /// 标准帧标识符上限。
    pub const STANDARD_MAX: u16 = 0x7FF;
    /// 扩展帧标识符上限。
    pub const EXTENDED_MAX: u32 = 0x1FFF_FFFF;

    /// 构造标准帧标识符；超过 11 位返回 `None`。
    pub const fn standard(raw: u16) -> Option<Self> {
        if raw > Self::STANDARD_MAX {
            None
        } else {
            Some(Self::Standard(raw))
        }
    }

    /// 构造扩展帧标识符；超过 29 位返回 `None`。
    pub const fn extended(raw: u32) -> Option<Self> {
        if raw > Self::EXTENDED_MAX {
            None
        } else {
            Some(Self::Extended(raw))
        }
    }

    /// 常量标识符的构造：超过 11 位在**编译期**就失败。
    ///
    /// 只在标识符是字面量或本仓常量时用——默认 ID 这类写死的值用它就不必
    /// 在调用点再写一次 expect。运行期算出来的值走 [`Self::standard`]。
    pub const fn standard_const(raw: u16) -> Self {
        match Self::standard(raw) {
            Some(id) => id,
            None => panic!("标准帧标识符必须在 11 位内"),
        }
    }

    /// 裸标识符值，按帧类型右对齐。
    pub const fn raw(self) -> u32 {
        match self {
            Self::Standard(v) => v as u32,
            Self::Extended(v) => v,
        }
    }

    /// 是否标准帧。
    pub const fn is_standard(self) -> bool {
        matches!(self, Self::Standard(_))
    }
}

/// 一帧 CAN 数据。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanFrame {
    /// 标识符。
    pub id: CanId,
    /// 数据字段，长度 0 到 64 字节（FD 上限）。
    pub data: Vec<u8>,
    /// 时间戳。后端能提供硬件时间戳时为硬件时刻，否则为接收时刻；
    /// 上层据 [`crate::BusCapabilities::hardware_timestamp`] 判断可精度。
    pub timestamp: Option<std::time::Instant>,
}

impl CanFrame {
    /// 数据字段长度上限（CAN-FD）。
    pub const MAX_DATA_LEN: usize = 64;

    /// 构造一帧。数据超过 64 字节返回 `None`。
    pub fn new(id: CanId, data: impl Into<Vec<u8>>) -> Option<Self> {
        let data = data.into();
        if data.len() > Self::MAX_DATA_LEN {
            return None;
        }
        Some(Self {
            id,
            data,
            timestamp: None,
        })
    }

    /// 挂上时间戳。
    pub fn with_timestamp(mut self, at: std::time::Instant) -> Self {
        self.timestamp = Some(at);
        self
    }

    /// 数据长度。
    pub fn len(&self) -> usize {
        self.data.len()
    }

    /// 是否零长度帧（合法的 CAN 帧形态）。
    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standard_id_rejects_more_than_11_bits() {
        assert_eq!(CanId::standard(0x7FF), Some(CanId::Standard(0x7FF)));
        assert_eq!(CanId::standard(0x800), None);
    }

    #[test]
    fn standard_const_matches_checked_constructor() {
        assert_eq!(CanId::standard_const(0x600), CanId::Standard(0x600));
        // 常量求值即校验：越界标识符在这里编译不过。
        const DEFAULT: CanId = CanId::standard_const(0x601);
        assert_eq!(DEFAULT, CanId::Standard(0x601));
    }

    #[test]
    fn extended_id_rejects_more_than_29_bits() {
        assert!(CanId::extended(CanId::EXTENDED_MAX).is_some());
        assert!(CanId::extended(CanId::EXTENDED_MAX + 1).is_none());
    }

    #[test]
    fn frame_rejects_payload_over_64_bytes() {
        assert!(CanFrame::new(CanId::Standard(0x201), vec![0; 8]).is_some());
        assert!(CanFrame::new(CanId::Standard(0x201), vec![0; 64]).is_some());
        assert!(CanFrame::new(CanId::Standard(0x201), vec![0; 65]).is_none());
    }

    #[test]
    fn raw_id_round_trips_per_kind() {
        assert_eq!(CanId::Standard(0x201).raw(), 0x201);
        assert_eq!(CanId::Extended(0x18DA_F110).raw(), 0x18DA_F110);
        assert!(CanId::Extended(1).is_standard().eq(&false));
    }
}
