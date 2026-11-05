//! 总线抽象与能力描述。
//!
//! 这是三道屏蔽墙的第一道：上层只认 [`Bus`] 与 [`BusCapabilities`]，
//! 新增一种硬件 = 新建一个后端 crate + 装配层加一个分支，`xcp` 与
//! `session` 的源码不动。

#![forbid(unsafe_code)]

pub mod capability;
pub mod error;
pub mod frame;

pub use capability::{BusCapabilities, FrameKinds};
pub use error::BusError;
pub use frame::{CanFrame, CanId};

use std::time::Duration;

use async_trait::async_trait;

/// 一条 CAN 通道。
///
/// 实现方负责把后端差异收进内部；上层只用 `send` / `recv` 与能力描述。
#[async_trait]
pub trait Bus: Send + Sync {
    /// 后端名，用于日志、故障上下文与设备锁。
    fn backend(&self) -> &'static str;

    /// 通道名，例如 `can0`、`vcan0`。
    fn channel(&self) -> &str;

    /// 能力描述。上层据此降级，不靠 `cfg!` 或类型判断。
    fn capabilities(&self) -> BusCapabilities;

    /// 发送一帧。
    async fn send(&self, frame: CanFrame) -> Result<(), BusError>;

    /// 在超时内收一帧。
    async fn recv(&self, timeout: Duration) -> Result<CanFrame, BusError>;
}

#[cfg(test)]
pub(crate) mod test_support {
    //! 测试用的内存总线：不碰硬件，供上层 crate 复用。

    use std::sync::Mutex;
    use std::time::Duration;

    use async_trait::async_trait;

    use crate::{Bus, BusCapabilities, BusError, CanFrame, FrameKinds};

    /// 把发出的帧存在内存里，`recv` 从队列取。队列空时按超时返回。
    #[derive(Debug, Default)]
    pub struct FakeBus {
        outbox: Mutex<Vec<CanFrame>>,
        inbox: Mutex<Vec<CanFrame>>,
    }

    impl FakeBus {
        /// 新建。
        pub fn new() -> Self {
            Self::default()
        }

        /// 塞一帧给 `recv`。
        pub fn push_inbound(&self, frame: CanFrame) {
            self.inbox.lock().expect("测试锁未中毒").push(frame);
        }

        /// 取出所有已发送的帧。
        pub fn sent(&self) -> Vec<CanFrame> {
            self.outbox.lock().expect("测试锁未中毒").clone()
        }
    }

    #[async_trait]
    impl Bus for FakeBus {
        fn backend(&self) -> &'static str {
            "fake"
        }

        fn channel(&self) -> &str {
            "fake0"
        }

        fn capabilities(&self) -> BusCapabilities {
            BusCapabilities {
                backends: FrameKinds::classic_only(),
                hardware_timestamp: false,
            }
        }

        async fn send(&self, frame: CanFrame) -> Result<(), BusError> {
            self.outbox.lock().expect("测试锁未中毒").push(frame);
            Ok(())
        }

        async fn recv(&self, timeout: Duration) -> Result<CanFrame, BusError> {
            let mut inbox = self.inbox.lock().expect("测试锁未中毒");
            if inbox.is_empty() {
                return Err(BusError::Timeout {
                    waited_ms: timeout.as_millis() as u64,
                });
            }
            Ok(inbox.remove(0))
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::CanId;

        #[tokio::test]
        async fn fake_bus_round_trips_and_is_boxable_as_dyn() {
            let bus = FakeBus::new();
            let outbound = CanFrame::new(CanId::Standard(0x201), vec![1, 2]).expect("2 字节合法");
            bus.send(outbound.clone()).await.expect("发送成功");
            assert_eq!(bus.sent(), vec![outbound]);

            let inbound = CanFrame::new(CanId::Standard(0x202), vec![3]).expect("合法");
            bus.push_inbound(inbound.clone());
            let received = bus.recv(Duration::from_millis(5)).await.expect("能收到");
            assert_eq!(received, inbound);

            let boxed: Box<dyn Bus> = Box::new(bus);
            assert_eq!(boxed.backend(), "fake");
            assert_eq!(boxed.channel(), "fake0");
        }

        #[tokio::test]
        async fn empty_inbox_reports_timeout_with_waited_ms() {
            let bus = FakeBus::new();
            let err = bus
                .recv(Duration::from_millis(25))
                .await
                .expect_err("应为超时");
            assert!(matches!(err, BusError::Timeout { waited_ms: 25 }));
        }
    }
}
