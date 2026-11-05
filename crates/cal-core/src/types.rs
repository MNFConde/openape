//! 跨 crate 共享的基础类型。
//!
//! M1 的 `cal-core` 只有这三类：地址、长度、句柄。变量树、Curve/Map/轴
//! 与 `CalibrationSource` trait 等真实输入源出现后再落（见 M2-01、M2-02）。

use std::fmt;

use serde::{Deserialize, Serialize};

/// ECU 内存地址。
///
/// 任意 `u32` 都是合法地址——地址是否可读属于信息源与 ECU 的知识，
/// 不在类型层拒绝。越界判定发生在读通路，返回可分类的故障。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Address(u32);

impl Address {
    /// 从裸地址构造。
    pub const fn new(raw: u32) -> Self {
        Self(raw)
    }

    /// 裸地址。
    pub const fn get(self) -> u32 {
        self.0
    }

    /// 按长度推进到下一个地址；越过 `u32` 上限时返回 `None`。
    pub const fn checked_advance(self, len: ByteLen) -> Option<Self> {
        match self.0.checked_add(len.get() as u32) {
            Some(next) => Some(Self(next)),
            None => None,
        }
    }

    /// 加上字节偏移；溢出返回 `None`。
    pub const fn checked_offset(self, offset: u32) -> Option<Self> {
        match self.0.checked_add(offset) {
            Some(next) => Some(Self(next)),
            None => None,
        }
    }
}

impl fmt::Display for Address {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:#010x}", self.0)
    }
}

impl From<Address> for u32 {
    fn from(addr: Address) -> Self {
        addr.0
    }
}

/// 字节长度。零长度是非法状态（读 0 字节没有意义），在构造处就被拒绝。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "u16")]
pub struct ByteLen(u16);

impl ByteLen {
    /// 一个字节。
    pub const ONE: Self = Self(1);

    /// 构造长度；`raw == 0` 返回 `None`。
    pub const fn new(raw: u16) -> Option<Self> {
        if raw == 0 { None } else { Some(Self(raw)) }
    }

    /// 字节数。
    pub const fn get(self) -> u16 {
        self.0
    }
}

impl TryFrom<u16> for ByteLen {
    type Error = ZeroLen;

    fn try_from(raw: u16) -> Result<Self, Self::Error> {
        Self::new(raw).ok_or(ZeroLen)
    }
}

impl fmt::Display for ByteLen {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} B", self.0)
    }
}

/// 长度为零。只由 [`ByteLen`] 的转换产生。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ZeroLen;

impl fmt::Display for ZeroLen {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("字节长度不能为 0")
    }
}

impl std::error::Error for ZeroLen {}

/// 会话内句柄。不透明，由持有方分配，跨 crate 只用来指认对象。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Handle(u32);

impl Handle {
    /// 从原始值构造。
    pub const fn new(raw: u32) -> Self {
        Self(raw)
    }

    /// 原始值。
    pub const fn get(self) -> u32 {
        self.0
    }
}

impl fmt::Display for Handle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "#{}", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn byte_len_rejects_zero() {
        assert!(ByteLen::new(0).is_none());
        assert_eq!(ByteLen::new(1), Some(ByteLen::ONE));
        assert_eq!(ByteLen::new(4).map(ByteLen::get), Some(4));
    }

    #[test]
    fn byte_len_never_deserializes_to_zero() {
        assert!(
            serde_json::from_str::<ByteLen>("0").is_err(),
            "0 必须是非法输入"
        );
        let len: ByteLen = serde_json::from_str("4").expect("4 合法");
        assert_eq!(len.get(), 4);
        assert_eq!(serde_json::to_string(&len).expect("可序列化"), "4");
    }

    #[test]
    fn address_display_is_hex_with_full_width() {
        assert_eq!(Address::new(0x1000).to_string(), "0x00001000");
        assert_eq!(Address::new(0).to_string(), "0x00000000");
    }

    #[test]
    fn address_advance_saturates_into_none() {
        let len = ByteLen::new(4).expect("4 合法");
        assert_eq!(
            Address::new(0x1000).checked_advance(len),
            Some(Address::new(0x1004))
        );
        assert_eq!(Address::new(u32::MAX).checked_advance(len), None);
        assert_eq!(Address::new(u32::MAX).checked_offset(1), None);
    }

    #[test]
    fn handle_round_trips_through_serde() {
        let h = Handle::new(7);
        let json = serde_json::to_string(&h).expect("可序列化");
        assert_eq!(json, "7");
        assert_eq!(serde_json::from_str::<Handle>("7").expect("可反序列化"), h);
        assert_eq!(h.to_string(), "#7");
    }
}
