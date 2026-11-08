//! 日志回放后端（M5-10）：把 Vector 日志（ASC 文本 / BLF 二进制）当成
//! 一条 CAN 总线放给上层。
//!
//! 回放与实时走同一条采集通路：[`ReplayBus`] 实现 [`Bus`]，上层（采集、
//! 观测、会话）拿它与拿 SocketCanBus 是同一个接口、同一批代码路径——
//! 差异只在帧从哪来。按用户请求触发：daemon 给 `--replay` 才进回放，
//! 没给就照常开硬件通道。
//!
//! 时间语义：按日志里的相对时间真实回放（100 ms 的间隔就等 100 ms），
//! `recv` 阻塞到点才返回帧；全部帧放完返回 `Closed`。帧的时间戳按
//! 「起表时刻 + 日志偏移」盖入（D-25：保留日志时间、禁墙钟混源）。
//! 回放是单向观察，`send` 报不支持——往日志里"写帧"没有意义。

pub mod asc;
pub mod blf;

use std::sync::Mutex;
use std::time::Duration;

use async_trait::async_trait;
use openape_bus::{Bus, BusCapabilities, BusError, CanFrame};
use tokio::time::Instant;

/// 日志里的一帧：相对回放起点的微秒偏移 + 帧本体。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplayFrame {
    /// 相对回放起点（日志首帧）的微秒。
    pub offset_us: u64,
    /// 帧本体。
    pub frame: CanFrame,
}

/// 回放总线：按日志时间把帧依次吐给 `recv`。
pub struct ReplayBus {
    frames: Mutex<Vec<ReplayFrame>>,
    started: Instant,
}

impl ReplayBus {
    /// 用一份帧序列建回放总线。时间基准在第一次 `recv` 时起表。
    pub fn new(frames: Vec<ReplayFrame>) -> Self {
        Self {
            frames: Mutex::new(frames),
            started: Instant::now(),
        }
    }
}

#[async_trait]
impl Bus for ReplayBus {
    fn backend(&self) -> &'static str {
        "replay"
    }

    fn channel(&self) -> &str {
        "replay0"
    }

    fn capabilities(&self) -> BusCapabilities {
        // 日志时间戳是软件记的，精度同软件时间戳档。
        BusCapabilities::CLASSIC_SOFT_TIMESTAMP
    }

    async fn send(&self, _frame: CanFrame) -> Result<(), BusError> {
        Err(BusError::Unsupported {
            kind: "回放写入"
        })
    }

    /// 锁内算好"发帧还是再等"，等在锁外做——std 锁的 guard 不能跨 await。
    async fn recv(&self, timeout: Duration) -> Result<CanFrame, BusError> {
        enum Step {
            Deliver(CanFrame),
            Wait(Duration),
        }
        let step = {
            let mut frames = self.frames.lock().expect("回放队列锁");
            let Some(next) = frames.first() else {
                return Err(BusError::Closed);
            };
            let due = self.started + Duration::from_micros(next.offset_us);
            let now = Instant::now();
            if due <= now {
                let ReplayFrame { offset_us, frame } = frames.remove(0);
                // 保留日志时间（D-25）：时间戳 = 起表 + 日志偏移，不是接收
                // 时刻。采集泵据此推样本时间，绝不回退到墙钟。
                Step::Deliver(
                    frame.with_timestamp((self.started + Duration::from_micros(offset_us)).into()),
                )
            } else {
                Step::Wait((due - now).min(timeout))
            }
        };
        match step {
            Step::Deliver(frame) => Ok(frame),
            Step::Wait(wait) => {
                tokio::time::sleep(wait).await;
                Err(BusError::Timeout {
                    waited_ms: wait.as_millis() as u64,
                })
            }
        }
    }
}

/// 回放面共用的错误。
#[derive(Debug, thiserror::Error)]
pub enum ReplayError {
    /// 帧行认出来了但解析不了（id/DLC/字节不在域内）。
    #[error("第 {line} 行解析不了：{detail}")]
    Parse {
        /// 出错行号（1 起）。
        line: usize,
        /// 人话原因。
        detail: String,
    },
    /// BLF 底层读失败（ablf 返回读错误与 reader）。
    #[error("BLF 读失败：{0}")]
    Blf(#[from] std::io::Error),
}
