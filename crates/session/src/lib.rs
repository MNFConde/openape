//! 编排引擎：装配、会话状态与设备锁。
//!
//! 这是三层之上唯一同时知道具体后端与协议实现的地方。CLI 与 daemon 都
//! 是它的外壳，行为因此天然一致（D-18）。

#![forbid(unsafe_code)]

pub mod audit;
pub mod capture;
pub mod daq_stream;
pub mod dbc_observer;
pub mod engine;
pub mod lock;
pub mod pages;
pub mod seq;
pub mod write;

pub use audit::{AUDIT_FILE, AuditLog, AuditRecord};
pub use capture::{CaptureConfig, DaqCapture};
pub use daq_stream::{DaqSample, DaqStats, DaqStream, DaqStreamReader};
pub use dbc_observer::{CanObserver, DecodedFrame, DecodedSignal};
pub use engine::{ChannelProbe, Session, SessionConfig, list_channels, probe_channel};
pub use lock::DeviceLock;
pub use pages::calibration_variables;
pub use write::{RangeVerdict, WriteContext, Written, persist_snapshot, read_snapshot};
