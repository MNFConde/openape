//! 总线层错误，可转为统一故障。

use openape_diag::{Domain, Fault, Severity};

/// 总线操作失败的原因。
#[derive(Debug, thiserror::Error)]
pub enum BusError {
    /// 通道不存在或无权限打开。
    #[error("通道 {channel} 打不开：{detail}")]
    ChannelUnavailable {
        /// 通道名。
        channel: String,
        /// 底层给出的原因。
        detail: String,
    },
    /// 等待期限内没有收到帧。
    #[error("等待 {waited_ms} ms 未收到帧")]
    Timeout {
        /// 实际等待毫秒数。
        waited_ms: u64,
    },
    /// 后端已关闭（socket 被关闭或任务退出）。
    #[error("总线已关闭")]
    Closed,
    /// 后端不支持该帧形态。
    #[error("后端不支持{kind}")]
    Unsupported {
        /// 帧形态的人话名。
        kind: &'static str,
    },
    /// 底层驱动返回的错误。
    #[error("底层驱动错误：{0}")]
    Transport(String),
}

impl BusError {
    /// 故障编号序号。域内分段：1 通道、2 超时、3 关闭、4 不支持、5 驱动。
    const fn seq(&self) -> u32 {
        match self {
            Self::ChannelUnavailable { .. } => 1,
            Self::Timeout { .. } => 2,
            Self::Closed => 3,
            Self::Unsupported { .. } => 4,
            Self::Transport(_) => 5,
        }
    }
}

impl From<BusError> for Fault {
    fn from(err: BusError) -> Self {
        let severity = match err {
            BusError::Timeout { .. } => Severity::Warning,
            _ => Severity::Error,
        };
        let mut fault = Fault::new(
            openape_diag::FaultCode::new(Domain::Bus, err.seq()).expect("序号固定在 1..=999"),
            severity,
            err.to_string(),
        );
        if let BusError::ChannelUnavailable { channel, .. } = &err {
            fault = fault.with_context("channel", channel.clone());
        }
        fault.with_hint(match err {
            BusError::ChannelUnavailable { .. } => {
                "用 `ip link` 确认通道存在，并检查进程是否有 CAP_NET_RAW"
            }
            BusError::Timeout { .. } => "确认对端已启动、波特率一致",
            BusError::Closed => "重新打开会话",
            BusError::Unsupported { .. } => "换支持该帧形态的后端，或改用经典 CAN",
            BusError::Transport(_) => "查看维测日志中的帧级记录",
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_variant_maps_into_bus_domain() {
        let cases = [
            BusError::ChannelUnavailable {
                channel: "vcan0".into(),
                detail: "No such device".into(),
            },
            BusError::Timeout { waited_ms: 10 },
            BusError::Closed,
            BusError::Unsupported { kind: "CAN-FD" },
            BusError::Transport("EIO".into()),
        ];
        for err in cases {
            let fault = Fault::from(err);
            assert_eq!(fault.code().domain(), Domain::Bus);
            assert!(fault.hint().is_some(), "每个总线故障都要给可操作建议");
        }
    }

    #[test]
    fn timeout_is_warning_not_error() {
        let fault = Fault::from(BusError::Timeout { waited_ms: 5 });
        assert_eq!(fault.severity(), Severity::Warning);
    }

    #[test]
    fn channel_failure_carries_channel_context() {
        let fault = Fault::from(BusError::ChannelUnavailable {
            channel: "vcan9".into(),
            detail: "down".into(),
        });
        let json = serde_json::to_value(&fault).expect("可序列化");
        assert_eq!(json["context"]["channel"], "vcan9");
    }
}
