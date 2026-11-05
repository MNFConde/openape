//! 能力描述。上层据此降级，而不是判断后端类型。

/// 支持哪些帧形态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameKinds {
    /// 经典 CAN。
    pub classic: bool,
    /// CAN-FD。
    pub fd: bool,
}

impl FrameKinds {
    /// 只支持经典 CAN。
    pub const fn classic_only() -> Self {
        Self {
            classic: true,
            fd: false,
        }
    }

    /// 两者都支持。
    pub const fn both() -> Self {
        Self {
            classic: true,
            fd: true,
        }
    }
}

/// 后端能力。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BusCapabilities {
    /// 支持的帧形态。
    pub backends: FrameKinds,
    /// 是否提供硬件时间戳。为 `false` 时上层失去微秒级到达精度，
    /// 只能按接收时刻计——这是降级点，不是错误。
    pub hardware_timestamp: bool,
}

impl BusCapabilities {
    /// 是否能发经典 CAN 帧。
    pub fn classic_can(&self) -> bool {
        self.backends.classic
    }

    /// 是否能发 CAN-FD 帧。
    pub fn can_fd(&self) -> bool {
        self.backends.fd
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classic_only_backend_degrades_fd() {
        let caps = BusCapabilities {
            backends: FrameKinds::classic_only(),
            hardware_timestamp: false,
        };
        assert!(caps.classic_can());
        assert!(!caps.can_fd());
    }

    #[test]
    fn fd_backend_reports_both_kinds() {
        let caps = BusCapabilities {
            backends: FrameKinds::both(),
            hardware_timestamp: true,
        };
        assert!(caps.classic_can() && caps.can_fd());
        assert!(caps.hardware_timestamp);
    }
}
