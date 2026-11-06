//! CTO 命令码与命令载荷编解码。载荷不含 PID，PID 由传输层加。

use crate::codec::{Decoder, Encoder};
use crate::{DecodeError, EncodeError, MAX_CTO};

/// 命令码。数值取自 ASAM XCP 标准；M0-2 spike 里有两条错值，见模块文档。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum CommandCode {
    /// 建立会话。
    Connect = 0xFF,
    /// 结束会话。
    Disconnect = 0xFE,
    /// 查会话状态。
    GetStatus = 0xFD,
    /// 同步。
    Synch = 0xFC,
    /// 查通信模式附信息。
    GetCommModeInfo = 0xFB,
    /// 设置内存传输地址。
    SetMta = 0xF6,
    /// 从当前地址续读。
    Upload = 0xF5,
    /// 按地址短读。
    ShortUpload = 0xF4,
    /// 向当前地址写。
    Download = 0xF0,
    /// 查标定页。
    GetCalPage = 0xEA,
    /// 切标定页。
    SetCalPage = 0xEB,
}

impl CommandCode {
    /// 从字节还原命令码。
    pub const fn from_u8(byte: u8) -> Option<Self> {
        Some(match byte {
            0xFF => Self::Connect,
            0xFE => Self::Disconnect,
            0xFD => Self::GetStatus,
            0xFC => Self::Synch,
            0xFB => Self::GetCommModeInfo,
            0xF6 => Self::SetMta,
            0xF5 => Self::Upload,
            0xF4 => Self::ShortUpload,
            0xF0 => Self::Download,
            0xEA => Self::GetCalPage,
            0xEB => Self::SetCalPage,
            _ => return None,
        })
    }
}

/// 一条 CTO 命令。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// 建立会话，`mode` 为标准保留位，通常为 0。
    Connect {
        /// 会话模式字节。
        mode: u8,
    },
    /// 结束会话。
    Disconnect,
    /// 查会话状态。
    GetStatus,
    /// 同步命令。
    Synch,
    /// 查通信模式附信息。
    GetCommModeInfo,
    /// 设置内存传输地址。
    SetMta {
        /// 地址扩展（分段）。
        address_extension: u8,
        /// 目标地址。
        address: u32,
    },
    /// 从当前传输地址续读 `len` 字节，地址自增。
    Upload {
        /// 字节数。
        len: u8,
    },
    /// 按地址短读，不动传输地址。
    ShortUpload {
        /// 字节数。
        len: u8,
        /// 地址扩展。
        address_extension: u8,
        /// 目标地址。
        address: u32,
    },
    /// 向当前传输地址写，地址自增。
    Download {
        /// 字节数。
        len: u8,
        /// 数据，长度与 `len` 一致。
        data: Vec<u8>,
    },
    /// 查标定页。
    GetCalPage {
        /// 访问模式。
        mode: u8,
        /// 逻辑段号。
        segment: u8,
    },
    /// 切标定页。
    SetCalPage {
        /// 访问模式。
        mode: u8,
        /// 逻辑段号。
        segment: u8,
        /// 页号。
        page: u16,
    },
}

impl Command {
    /// 命令码。分支数等于命令表条数，CRAP 刻意保留在阈值之上，见
    /// [`Command::decode_command`]。
    pub const fn code(&self) -> CommandCode {
        match self {
            Self::Connect { .. } => CommandCode::Connect,
            Self::Disconnect => CommandCode::Disconnect,
            Self::GetStatus => CommandCode::GetStatus,
            Self::Synch => CommandCode::Synch,
            Self::GetCommModeInfo => CommandCode::GetCommModeInfo,
            Self::SetMta { .. } => CommandCode::SetMta,
            Self::Upload { .. } => CommandCode::Upload,
            Self::ShortUpload { .. } => CommandCode::ShortUpload,
            Self::Download { .. } => CommandCode::Download,
            Self::GetCalPage { .. } => CommandCode::GetCalPage,
            Self::SetCalPage { .. } => CommandCode::SetCalPage,
        }
    }

    /// 编码成整帧（PID + 命令码 + 参数）。超过 `MAX_CTO` 返回错误。
    pub fn encode(&self) -> Result<Vec<u8>, EncodeError> {
        // 命令码只有 `code()` 一处来源；这里不再逐个硬编码，否则改一处漏一处。
        let code = self.code() as u8;
        let bytes = match self {
            Self::Connect { mode } => Encoder::with_frame(code).u8(*mode).finish(),
            Self::Disconnect => Encoder::with_frame(code).finish(),
            Self::GetStatus => Encoder::with_frame(code).finish(),
            Self::Synch => Encoder::with_frame(code).finish(),
            Self::GetCommModeInfo => Encoder::with_frame(code).finish(),
            Self::SetMta {
                address_extension,
                address,
            } => Encoder::with_frame(code)
                .u8(*address_extension)
                .u32(*address)
                .finish(),
            Self::Upload { len } => Encoder::with_frame(code).u8(*len).finish(),
            Self::ShortUpload {
                len,
                address_extension,
                address,
            } => Encoder::with_frame(code)
                .u8(*len)
                .u8(*address_extension)
                .u32(*address)
                .finish(),
            Self::Download { len, data } => {
                if data.len() != *len as usize {
                    return Err(EncodeError::LenMismatch {
                        declared: *len,
                        actual: data.len(),
                    });
                }
                Encoder::with_frame(code).u8(*len).bytes(data).finish()
            }
            Self::GetCalPage { mode, segment } => {
                Encoder::with_frame(code).u8(*mode).u8(*segment).finish()
            }
            Self::SetCalPage {
                mode,
                segment,
                page,
            } => Encoder::with_frame(code)
                .u8(*mode)
                .u8(*segment)
                .u16(*page)
                .finish(),
        };
        if bytes.len() > MAX_CTO {
            return Err(EncodeError::TooLong {
                len: bytes.len(),
                max: MAX_CTO,
            });
        }
        Ok(bytes)
    }

    /// 从整帧解码（校验 PID，然后从命令码开始读）。
    pub fn decode(frame: &[u8]) -> Result<Self, DecodeError> {
        match frame.first() {
            Some(&crate::PID_CMD_RES) => {}
            Some(&other) => return Err(DecodeError::UnknownCommand { code: other }),
            None => return Err(DecodeError::Truncated { need: 1, got: 0 }),
        }
        let mut decoder = Decoder::at(frame, 1);
        let raw = decoder.u8()?;
        let code = CommandCode::from_u8(raw).ok_or(DecodeError::UnknownCommand { code: raw })?;
        Self::decode_command(code, &mut decoder)
    }

    /// 按命令码分派参数解析。
    ///
    /// CRAP 报告里本函数的圈复杂度是 11，与 [`Command::code`] 的 12 一样高于
    /// 阈值 10：这两个分支数是 XCP 命令表本身，不是设计缺陷——拆到底还是
    /// 十一条臂。**刻意保留**，不再拆分。
    fn decode_command(code: CommandCode, decoder: &mut Decoder<'_>) -> Result<Self, DecodeError> {
        Ok(match code {
            CommandCode::Connect => Self::decode_connect(decoder)?,
            CommandCode::Disconnect => Self::Disconnect,
            CommandCode::GetStatus => Self::GetStatus,
            CommandCode::Synch => Self::Synch,
            CommandCode::GetCommModeInfo => Self::GetCommModeInfo,
            CommandCode::SetMta => Self::decode_set_mta(decoder)?,
            CommandCode::Upload => Self::decode_upload(decoder)?,
            CommandCode::ShortUpload => Self::decode_short_upload(decoder)?,
            CommandCode::Download => Self::decode_download(decoder)?,
            CommandCode::GetCalPage => Self::decode_get_cal_page(decoder)?,
            CommandCode::SetCalPage => Self::decode_set_cal_page(decoder)?,
        })
    }

    fn decode_connect(decoder: &mut Decoder<'_>) -> Result<Self, DecodeError> {
        Ok(Self::Connect {
            mode: decoder.u8()?,
        })
    }

    fn decode_set_mta(decoder: &mut Decoder<'_>) -> Result<Self, DecodeError> {
        let address_extension = decoder.u8()?;
        Ok(Self::SetMta {
            address_extension,
            address: decoder.u32()?,
        })
    }

    fn decode_upload(decoder: &mut Decoder<'_>) -> Result<Self, DecodeError> {
        Ok(Self::Upload { len: decoder.u8()? })
    }

    fn decode_short_upload(decoder: &mut Decoder<'_>) -> Result<Self, DecodeError> {
        let len = decoder.u8()?;
        let address_extension = decoder.u8()?;
        Ok(Self::ShortUpload {
            len,
            address_extension,
            address: decoder.u32()?,
        })
    }

    fn decode_download(decoder: &mut Decoder<'_>) -> Result<Self, DecodeError> {
        let len = decoder.u8()?;
        let data = decoder.take(len as usize)?.to_vec();
        Ok(Self::Download { len, data })
    }

    fn decode_get_cal_page(decoder: &mut Decoder<'_>) -> Result<Self, DecodeError> {
        Ok(Self::GetCalPage {
            mode: decoder.u8()?,
            segment: decoder.u8()?,
        })
    }

    fn decode_set_cal_page(decoder: &mut Decoder<'_>) -> Result<Self, DecodeError> {
        let mode = decoder.u8()?;
        let segment = decoder.u8()?;
        Ok(Self::SetCalPage {
            mode,
            segment,
            page: decoder.u16()?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_match_the_spike_byte_layout() {
        // 这些字节直接来自 M0-2 spike 抓到的帧，改这里等于改协议。
        assert_eq!(
            Command::SetMta {
                address_extension: 0,
                address: 0x1000
            }
            .encode()
            .expect("一帧放得下"),
            vec![0xFF, 0xF6, 0x00, 0x00, 0x00, 0x10, 0x00, 0x00]
        );
        assert_eq!(
            Command::ShortUpload {
                len: 2,
                address_extension: 0,
                address: 0x1002
            }
            .encode()
            .expect("一帧放得下"),
            vec![0xFF, 0xF4, 0x02, 0x00, 0x02, 0x10, 0x00, 0x00]
        );
        assert_eq!(
            Command::Upload { len: 4 }.encode().expect("一帧放得下"),
            vec![0xFF, 0xF5, 0x04]
        );
        assert_eq!(
            Command::Download {
                len: 3,
                data: vec![0xAA, 0xBB, 0xCC]
            }
            .encode()
            .expect("一帧放得下"),
            vec![0xFF, 0xF0, 0x03, 0xAA, 0xBB, 0xCC]
        );
        assert_eq!(
            Command::SetCalPage {
                mode: 0,
                segment: 0,
                page: 1
            }
            .encode()
            .expect("一帧放得下"),
            vec![0xFF, 0xEB, 0x00, 0x00, 0x01, 0x00]
        );
    }

    #[test]
    fn every_command_round_trips() {
        let commands = [
            Command::Connect { mode: 0 },
            Command::Disconnect,
            Command::GetStatus,
            Command::Synch,
            Command::GetCommModeInfo,
            Command::SetMta {
                address_extension: 1,
                address: 0x2000,
            },
            Command::Upload { len: 7 },
            Command::ShortUpload {
                len: 1,
                address_extension: 0,
                address: 0x1004,
            },
            Command::Download {
                len: 2,
                data: vec![0x11, 0x22],
            },
            Command::GetCalPage {
                mode: 0,
                segment: 0,
            },
            Command::SetCalPage {
                mode: 0,
                segment: 0,
                page: 1,
            },
        ];
        for command in commands {
            let encoded = command.encode().expect("命令都放得下一帧");
            assert_eq!(
                Command::decode(&encoded),
                Ok(command.clone()),
                "{command:?}"
            );
        }
    }

    #[test]
    fn every_command_reports_its_own_code() {
        // 命令码只有 code() 一处来源，这里把整张表钉住。
        let pairs = [
            (Command::Connect { mode: 0 }, CommandCode::Connect),
            (Command::Disconnect, CommandCode::Disconnect),
            (Command::GetStatus, CommandCode::GetStatus),
            (Command::Synch, CommandCode::Synch),
            (Command::GetCommModeInfo, CommandCode::GetCommModeInfo),
            (
                Command::SetMta {
                    address_extension: 0,
                    address: 0,
                },
                CommandCode::SetMta,
            ),
            (Command::Upload { len: 1 }, CommandCode::Upload),
            (
                Command::ShortUpload {
                    len: 1,
                    address_extension: 0,
                    address: 0,
                },
                CommandCode::ShortUpload,
            ),
            (
                Command::Download {
                    len: 0,
                    data: vec![],
                },
                CommandCode::Download,
            ),
            (
                Command::GetCalPage {
                    mode: 0,
                    segment: 0,
                },
                CommandCode::GetCalPage,
            ),
            (
                Command::SetCalPage {
                    mode: 0,
                    segment: 0,
                    page: 0,
                },
                CommandCode::SetCalPage,
            ),
        ];
        for (command, expected) in pairs {
            assert_eq!(command.code(), expected, "{command:?}");
            assert_eq!(
                command.encode().expect("一帧放得下")[1],
                expected as u8,
                "编码出来的命令码必须与 code() 一致"
            );
        }
    }

    #[test]
    fn command_code_table_covers_every_wire_value() {
        let table = [
            (0xFF, CommandCode::Connect),
            (0xFE, CommandCode::Disconnect),
            (0xFD, CommandCode::GetStatus),
            (0xFC, CommandCode::Synch),
            (0xFB, CommandCode::GetCommModeInfo),
            (0xF6, CommandCode::SetMta),
            (0xF5, CommandCode::Upload),
            (0xF4, CommandCode::ShortUpload),
            (0xF0, CommandCode::Download),
            (0xEA, CommandCode::GetCalPage),
            (0xEB, CommandCode::SetCalPage),
        ];
        for (byte, code) in table {
            assert_eq!(CommandCode::from_u8(byte), Some(code), "{byte:#04x}");
        }
        assert_eq!(CommandCode::from_u8(0x00), None, "未登记的命令码不猜");
    }

    #[test]
    fn error_messages_state_the_problem_and_the_numbers() {
        assert_eq!(
            DecodeError::Truncated { need: 6, got: 4 }.to_string(),
            "载荷被截断：需要 6 字节，只有 4 字节"
        );
        assert_eq!(
            DecodeError::UnknownCommand { code: 0x22 }.to_string(),
            "未知 CTO 命令码 0x22"
        );
        assert_eq!(
            EncodeError::TooLong { len: 9, max: 8 }.to_string(),
            "帧长 9 超过上限 8"
        );
        assert_eq!(
            EncodeError::LenMismatch {
                declared: 4,
                actual: 2
            }
            .to_string(),
            "声明 4 字节但给了 2 字节"
        );
    }

    #[test]
    fn unknown_command_is_rejected_not_guessed() {
        let err = Command::decode(&[0x00, 0x00]).expect_err("0x00 不是 CTO 命令码");
        assert_eq!(err, DecodeError::UnknownCommand { code: 0x00 });
    }

    #[test]
    fn download_rejects_len_that_disagrees_with_data() {
        let err = Command::Download {
            len: 4,
            data: vec![1, 2],
        }
        .encode()
        .expect_err("长度与数据不一致");
        assert_eq!(
            err,
            EncodeError::LenMismatch {
                declared: 4,
                actual: 2
            }
        );
    }

    #[test]
    fn download_wider_than_a_frame_is_refused() {
        // 整帧 = PID + 命令码 + len + 数据；7 字节数据要 10 字节帧。
        let err = Command::Download {
            len: 7,
            data: vec![0; 7],
        }
        .encode()
        .expect_err("2 + 7 > 8");
        assert_eq!(
            err,
            EncodeError::TooLong {
                len: 10,
                max: MAX_CTO
            }
        );
    }

    #[test]
    fn truncated_download_payload_is_reported() {
        let err =
            Command::decode(&[0xFF, 0xF0, 0x04, 0x01, 0x02]).expect_err("声明 4 字节只给了 2");
        assert_eq!(err, DecodeError::Truncated { need: 7, got: 5 });
    }
}
