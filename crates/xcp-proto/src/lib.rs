//! XCP 帧编解码，master 与从站共享。
//!
//! CAN 传输层的帧约定（M0-2 实测）：byte0 是 PID——`0xFF` 命令与响应、
//! `0xFE` 错误、`0xFD` 事件、`0xFC` 服务请求，`0x00..=0xFB` 为 DAQ 的
//! DTO。本 crate 当前只覆盖 CTO（命令与响应）；DTO 与 DAQ 随 M2-07 落地。
//!
//! 与 M0-2 spike 的两处偏差是**刻意的**，不是遗留：
//!
//! 1. 响应是 `[PID, 数据]`，**不回显命令码**。早期实现以为标准要求回显，
//!    给响应加了命令码字节，实测抓帧证明那是错的：`FF 11 11` 就是两字节
//!    数据响应。
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

/// 一条 DAQ DTO。
///
/// DAQ 帧没有命令码：byte0 是**绝对 ODT 编号**，从 0 起跨 DAQ list 连续分配
/// （M0-2 实测：spike 里 list 0 的首个 PID 为 0）。因此 `0x00..=0xFB` 全部
/// 被 DTO 占用，命令与响应用 `0xFC..=0xFF`——这也是 master 能在一堆 DAQ 帧里
/// 认出响应的依据。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DaqFrame<'a> {
    /// 绝对 ODT 编号，等于帧的字节 0。
    pub odt: u8,
    /// 该 ODT 承载的数据，已剥掉 ODT 编号。
    pub payload: &'a [u8],
}

impl<'a> DaqFrame<'a> {
    /// 解析一帧。首字节 > [`PID_DAQ_MAX`] 说明这是命令或响应，不是 DAQ 帧。
    ///
    /// 调用方要**先按标识符过滤**：CRO 上的命令帧 byte0 也是命令码，同样
    /// 落在 0x00..=0xFB 区间，只看 byte0 会把命令当数据。
    pub fn parse(frame: &'a [u8]) -> Option<Self> {
        let odt = *frame.first()?;
        (odt <= PID_DAQ_MAX).then_some(Self {
            odt,
            payload: &frame[1..],
        })
    }
}

/// 绝对 ODT 编号分配器。
///
/// 编号跨 DAQ list 连续：第一个 list 的首个 ODT 拿 0，第二个 list 接着上一个
/// list 的编号往下排。从站按这张表发 DTO，master 按同一张表把 PID 还原成
/// (list, odt)——两侧共用这一份算术，错一处就整体错位。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OdtIds {
    next: u16,
}

impl Default for OdtIds {
    fn default() -> Self {
        Self::new()
    }
}

impl OdtIds {
    /// 从 0 开始分配。
    pub const fn new() -> Self {
        Self { next: 0 }
    }

    /// 为下一个 DAQ list 的 `odt_count` 个 ODT 分配编号，返回该 list 的首个
    /// 编号。编号用尽（超过 [`PID_DAQ_MAX`]）返回 `None`。
    pub fn allocate(&mut self, odt_count: u8) -> Option<u8> {
        let first = u8::try_from(self.next)
            .ok()
            .filter(|pid| *pid <= PID_DAQ_MAX)?;
        if odt_count == 0 {
            return Some(first);
        }
        let last = self.next + u16::from(odt_count) - 1;
        if last > u16::from(PID_DAQ_MAX) {
            return None;
        }
        self.next = last + 1;
        Some(first)
    }

    /// 下一个待分配的编号。
    pub const fn next_pid(self) -> u16 {
        self.next
    }
}

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
///
/// 响应帧是 `PID + 数据`：**不回显命令码**（M0-2 实测，`FF 05 00 08 08 00
/// 01 01` 是 CONNECT 的六字节响应）。master 靠"一次一条命令"的串行约定把
/// 响应对回请求，不靠回显。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Response {
    /// 正响应，`payload` 已剥掉 PID。
    Ok {
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
            Self::Ok { payload } => {
                frame.push(PID_CMD_RES);
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
            PID_CMD_RES => Ok(Self::Ok {
                payload: frame[1..].to_vec(),
            }),
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
    fn ok_response_is_pid_plus_payload_without_echo() {
        // 实测：SHORT_UPLOAD 两字节响应是 FF 11 11，没有命令码回显
        let frame = Response::Ok {
            payload: vec![0x11, 0x11],
        }
        .encode();
        assert_eq!(
            frame,
            vec![0xFF, 0x11, 0x11],
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
    fn connect_response_matches_the_spike_frame() {
        // 实测：601 [8] FF 05 00 08 08 00 01 01
        let response = Response::Ok {
            payload: vec![0x05, 0x00, 8, 8, 0, 1, 1],
        };
        let frame = response.encode();
        assert_eq!(frame, vec![0xFF, 0x05, 0x00, 0x08, 0x08, 0x00, 0x01, 0x01]);
        match Response::decode(&frame).expect("可解码") {
            Response::Ok { payload } => assert_eq!(payload, vec![0x05, 0x00, 8, 8, 0, 1, 1]),
            other => panic!("应为正响应，实际 {other:?}"),
        }
    }

    #[test]
    fn daq_frame_takes_every_pid_below_fc() {
        // 实测：601 [5] 00 A0 00 C8 00 —— PID 0 的 DTO 带 4 字节数据
        let frame = [0x00, 0xA0, 0x00, 0xC8, 0x00];
        let dto = DaqFrame::parse(&frame).expect("PID 0 是 DTO");
        assert_eq!(dto.odt, 0);
        assert_eq!(dto.payload, &[0xA0, 0x00, 0xC8, 0x00]);

        assert!(DaqFrame::parse(&[PID_DAQ_MAX]).is_some(), "0xFB 仍是 DTO");
        for pid in [PID_SERV, PID_EVENT, PID_ERR, PID_CMD_RES] {
            assert!(
                DaqFrame::parse(&[pid, 0x00]).is_none(),
                "{pid:#04x} 不是 DTO"
            );
        }
        assert!(DaqFrame::parse(&[]).is_none(), "空帧没有 PID");
    }

    #[test]
    fn odt_ids_run_continuously_across_lists() {
        let mut ids = OdtIds::new();
        assert_eq!(ids.allocate(2), Some(0), "第一个 list 从 0 起");
        assert_eq!(ids.allocate(1), Some(2), "跨 list 接着编号");
        assert_eq!(ids.allocate(0), Some(3), "零 ODT 不占号，也不出错");
        assert_eq!(ids.next_pid(), 3);
    }

    #[test]
    fn odt_ids_from_zero_handle_an_empty_list() {
        let mut ids = OdtIds::new();
        assert_eq!(ids.allocate(0), Some(0), "空 list 也报出自己的首个编号");
        assert_eq!(ids.next_pid(), 0, "没分配就不推进");
    }

    #[test]
    fn odt_ids_refuse_to_overrun_the_daq_pid_range() {
        let mut ids = OdtIds::new();
        assert_eq!(ids.allocate(252), Some(0), "0x00 到 0xFB 共 252 个号");
        assert_eq!(ids.next_pid(), 252);
        assert_eq!(ids.allocate(1), None, "0xFC 起被命令与响应占用");
    }

    #[test]
    fn response_payload_starts_right_after_the_pid() {
        match Response::decode(&[0xFF, 0x07, 0x08]).expect("可解码") {
            Response::Ok { payload } => assert_eq!(payload, vec![0x07, 0x08]),
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
    fn error_code_table_covers_every_known_value() {
        let table = [
            (0x00, ErrorCode::CmdSynch),
            (0x01, ErrorCode::CmdBusy),
            (0x02, ErrorCode::DaqActive),
            (0x10, ErrorCode::CmdUnknown),
            (0x11, ErrorCode::CmdSyntax),
            (0x12, ErrorCode::OutOfRange),
            (0x23, ErrorCode::PageNotValid),
        ];
        for (byte, code) in table {
            assert_eq!(ErrorCode::from_u8(byte), Some(code), "{byte:#04x}");
        }
        assert_eq!(ErrorCode::from_u8(0x7F), None, "未登记的错误码不猜");
    }

    #[test]
    fn standard_error_codes_differ_from_spike_values() {
        // spike 把未知命令记成 0x22；标准 0x22 是 ERR_ACCESS_LOCKED。
        assert_eq!(ErrorCode::from_u8(0x10), Some(ErrorCode::CmdUnknown));
        assert_eq!(ErrorCode::from_u8(0x22), None, "0x22 不在本表内，不猜");
    }
}
