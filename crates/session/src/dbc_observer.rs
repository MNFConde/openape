//! 总线观测：把 CAN 帧按 DBC 解成物理值，同时保留原始位。
//!
//! 与 DAQ 通路并列而不是取代它：XCP 的 DAQ 给的是 ECU 内部变量，DBC 给的是
//! 总线上一帧里各信号的位。两者的时间基准与精度都不同，混在一起会让人分不
//! 清"这个值是 ECU 算出来的还是总线上的"。
//!
//! 原始位一定要留着。物理值经过因子与偏移，一旦 DBC 的单位写错，只有原始位
//! 还说得清到底收到了什么。

use openape_format_dbc::{Database, Message};

/// 一个解出来的信号：原始位与物理值并排。
#[derive(Debug, Clone, PartialEq)]
pub struct DecodedSignal {
    name: String,
    raw: i64,
    physical: f64,
    unit: String,
}

impl DecodedSignal {
    /// 信号名。
    pub fn name(&self) -> &str {
        &self.name
    }

    /// 原始位（已做符号扩展）。DBC 写错单位时，只有它说得清收到了什么。
    pub fn raw(&self) -> i64 {
        self.raw
    }

    /// 物理值。
    pub fn physical(&self) -> f64 {
        self.physical
    }

    /// 单位。空串表示没有单位。
    pub fn unit(&self) -> &str {
        &self.unit
    }
}

/// 一帧解出来的全部信号。
#[derive(Debug, Clone, PartialEq)]
pub struct DecodedFrame {
    message_name: String,
    signals: Vec<DecodedSignal>,
}

impl DecodedFrame {
    /// 报文名。
    pub fn message_name(&self) -> &str {
        &self.message_name
    }

    /// 信号清单。
    pub fn signals(&self) -> &[DecodedSignal] {
        &self.signals
    }
}

/// 按 DBC 观测总线。
#[derive(Debug, Clone)]
pub struct CanObserver {
    database: Database,
}

impl CanObserver {
    /// 用一份 DBC 建观测器。
    pub fn new(database: Database) -> Self {
        Self { database }
    }

    /// 解一帧。DBC 里没有这个 id 时返回 `None`——总线上的报文远多于我们
    /// 关心的那些，未知 id 是常态而不是错误。
    pub fn decode(&self, id: u32, frame: &[u8]) -> Option<DecodedFrame> {
        let message: &Message = self.database.message(id)?;
        let signals = message
            .signals()
            .iter()
            .map(|signal| DecodedSignal {
                name: signal.name().to_owned(),
                raw: signal.sign_extended_raw(frame),
                physical: signal.decode(frame),
                unit: signal.unit().to_owned(),
            })
            .collect();
        Some(DecodedFrame {
            message_name: message.name().to_owned(),
            signals,
        })
    }

    /// 这份 DBC 描述了多少条报文。
    pub fn message_count(&self) -> usize {
        self.database.messages().count()
    }
}
