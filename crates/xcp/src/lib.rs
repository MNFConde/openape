//! XCP master。
//!
//! 传输介质差异收在 [`XcpConnection`] 后面，上层不认 CAN。

#![forbid(unsafe_code)]

pub mod can;
pub mod connection;
pub mod daq;

pub use can::{CanXcpMaster, MAX_SHORT_UPLOAD_LEN, XcpConfig};
pub use connection::{ConnectInfo, SessionStatus, XcpConnection};
pub use daq::{DaqLayout, DaqListSpec, DaqSignal};
