//! XCP master。
//!
//! 传输介质差异收在 [`XcpConnection`] 后面，上层不认 CAN。

#![forbid(unsafe_code)]

pub mod can;
pub mod connection;

pub use can::{CanXcpMaster, XcpConfig};
pub use connection::{ConnectInfo, SessionStatus, XcpConnection};
