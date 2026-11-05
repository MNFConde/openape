//! XCP 帧编解码，master 与从站共享。
//!
//! CAN 传输层的帧约定（M0-2 实测）：byte0 是 PID——`0xFF` 命令与响应、
//! `0xFE` 错误、`0xFD` 事件、`0xFC` 服务请求，`0x00..=0xFB` 为 DAQ 的
//! DTO。本 crate 当前只覆盖 CTO（命令与响应）；DTO 与 DAQ 随 M2-07 落地。
//!
//! 与 M0-2 spike 的两处偏差是**刻意的**，不是遗留：
//!
//! 1. spike 的命令响应是 `[PID, 数据]`，省掉了命令码回显；标准是
//!    `[PID, 命令码, 参数]`。本 crate 按标准，让真 ECU 与第三方 master
//!    都能对上。
//! 2. spike 把 `ERR_CMD_UNKNOWN` 记成 `0x22`，标准是 `0x10`（`0x22` 是
//!    `ERR_ACCESS_LOCKED`）。本 crate 按标准表。

#![forbid(unsafe_code)]

mod codec;
mod command;

pub use command::{Command, CommandCode};

/// CAN 传输层单帧上限（`maxCto`）。超过要走块传输，M1 未实现。
pub const MAX_CTO: usize = 8;

/// 载荷最大字节数：整帧减去 PID。
pub const MAX_CTO_PAYLOAD: usize = MAX_CTO - 1;

/// PID：命令与正响应共用。
pub const PID_CMD_RES: u8 = 0xFF;
/// PID：错误响应。
pub const PID_ERR: u8 = 0xFE;
/// PID：事件。
pub const PID_EVENT: u8 = 0xFD;
/// PID：服务请求。
pub const PID_SERV: u8 = 0xFC;
/// DAQ DTO 的 PID 上限（含）。
pub const PID_DAQ_MAX: u8 = 0xFB;

/// 协议版本（主版本, 次版本）。
pub const PROTOCOL_VERSION: (u8, u8) = (1, 1);
/// 传输层版本（主版本, 次版本）。
pub const TRANSPORT_VERSION: (u8, u8) = (1, 1);

/// 从站资源位（CONNECT 响应第二字节）。
pub mod resource {
    /// 标定页切换。
    pub const CAL_PAG: u8 = 0x01;
    /// DAQ。
    pub const DAQ: u8 = 0x04;
}

/// 错误码。数值按 ASAM 标准表。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum ErrorCode {
    /// 同步中，命令暂不可执行。
    CmdSynch = 0x00,
    /// 从站忙。
    CmdBusy = 0x01,
    /// DAQ 正在运行。
    DaqActive = 0x02,
    /// 命令不存在。
    CmdUnknown = 0x10,
    /// 命令语法错误。
    CmdSyntax = 0x11,
    /// 参数越界。
    OutOfRange = 0x12,
    /// 页号无效。
    PageNotValid = 0x23,
}

impl ErrorCode {
    /// 从字节还原错误码；未知值保留原始字节，不猜。
    pub const fn from_u8(byte: u8) -> Option<Self> {
        Some(match byte {
            0x00 => Self::CmdSynch,
            0x01 => Self::CmdBusy,
            0x02 => Self::DaqActive,
            0x10 => Self::CmdUnknown,
            0x11 => Self::CmdSyntax,
            0x12 => Self::OutOfRange,
            0x23 => Self::PageNotValid,
            _ => return None,
        })
    }
}

/// 解码失败的原因。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodeError {
    /// 载荷比命令要求的短。
    Truncated {
        /// 需要的字节数。
        need: usize,
        /// 实际拿到的字节数。
        got: usize,
    },
    /// 命令码不在已知表里。
    UnknownCommand {
        /// 原始字节。
        code: u8,
    },
}

impl std::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Truncated { need, got } => {
                write!(f, "载荷被截断：需要 {need} 字节，只有 {got} 字节")
            }
            Self::UnknownCommand { code } => write!(f, "未知 CTO 命令码 {code:#04x}"),
        }
    }
}

impl std::error::Error for DecodeError {}

/// 编码失败的原因。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EncodeError {
    /// 编码结果超过单帧上限。
    TooLong {
        /// 实际长度。
        len: usize,
        /// 上限。
        max: usize,
    },
    /// 声明的长度与实际数据不符。
    LenMismatch {
        /// 声明的字节数。
        declared: u8,
        /// 实际字节数。
        actual: usize,
    },
}

impl std::fmt::Display for EncodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooLong { len, max } => write!(f, "帧长 {len} 超过上限 {max}"),
            Self::LenMismatch { declared, actual } => {
                write!(f, "声明 {declared} 字节但给了 {actual} 字节")
            }
        }
    }
}

impl std::error::Error for EncodeError {}

/// 一条响应。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Response {
    /// 正响应，`payload` 已剥掉 PID 与命令码。
    Ok {
        /// 回显的命令码。
        command: u8,
        /// 响应参数。
        payload: Vec<u8>,
    },
    /// 错误响应。
    Err {
        /// 错误码原始字节。
        code: u8,
    },
}

impl Response {
    /// 编码成整帧（含 PID）。
    ///
    /// **不补齐到 8 字节**：补齐会抹掉 DLC，进而抹掉"响应是否被截断"这一
    /// 信息。是否填充由传输层决定——发往真 ECU 的请求帧要填满，从站回帧
    /// 保留真实长度，两者都不影响本函数的解码。
    pub fn encode(&self) -> Vec<u8> {
        let mut frame = Vec::with_capacity(MAX_CTO);
        match self {
            Self::Ok { command, payload } => {
                frame.push(PID_CMD_RES);
                frame.push(*command);
                frame.extend_from_slice(payload);
            }
            Self::Err { code } => {
                frame.push(PID_ERR);
                frame.push(*code);
            }
        }
        frame
    }

    /// 从整帧解码（含 PID）。
    pub fn decode(frame: &[u8]) -> Result<Self, DecodeError> {
        let pid = *frame
            .first()
            .ok_or(DecodeError::Truncated { need: 1, got: 0 })?;
        match pid {
            PID_CMD_RES => {
                let command = *frame
                    .get(1)
                    .ok_or(DecodeError::Truncated { need: 2, got: 1 })?;
                Ok(Self::Ok {
                    command,
                    payload: frame[2..].to_vec(),
                })
            }
            PID_ERR => {
                let code = *frame
                    .get(1)
                    .ok_or(DecodeError::Truncated { need: 2, got: 1 })?;
                Ok(Self::Err { code })
            }
            other => Err(DecodeError::UnknownCommand { code: other }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ok_response_echoes_command_and_keeps_real_length() {
        let frame = Response::Ok {
            command: CommandCode::Upload as u8,
            payload: vec![0xAA],
        }
        .encode();
        assert_eq!(
            frame,
            vec![0xFF, 0xF5, 0xAA],
            "保留真实长度，长度本身是信息"
        );
    }

    #[test]
    fn error_response_is_two_bytes() {
        let frame = Response::Err {
            code: ErrorCode::OutOfRange as u8,
        }
        .encode();
        assert_eq!(frame, vec![0xFE, 0x12]);
    }

    #[test]
    fn response_round_trips_ignoring_padding() {
        let response = Response::Ok {
            command: CommandCode::Connect as u8,
            payload: vec![0x04, 0x00, 8, 8, 1, 1],
        };
        let frame = response.encode();
        let decoded = Response::decode(&frame).expect("可解码");
        match decoded {
            Response::Ok { command, payload } => {
                assert_eq!(command, 0xFF, "CONNECT 回显自己的命令码");
                assert_eq!(payload[..6], [0x04, 0x00, 8, 8, 1, 1]);
            }
            other => panic!("应为正响应，实际 {other:?}"),
        }
    }

    #[test]
    fn empty_frame_is_reported_as_truncated() {
        assert_eq!(
            Response::decode(&[]),
            Err(DecodeError::Truncated { need: 1, got: 0 })
        );
    }

    #[test]
    fn standard_error_codes_differ_from_spike_values() {
        // spike 把未知命令记成 0x22；标准 0x22 是 ERR_ACCESS_LOCKED。
        assert_eq!(ErrorCode::from_u8(0x10), Some(ErrorCode::CmdUnknown));
        assert_eq!(ErrorCode::from_u8(0x22), None, "0x22 不在本表内，不猜");
    }
}
