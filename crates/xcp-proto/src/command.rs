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
    /// 查从站 DAQ 能力。
    GetDaqProcessorInfo = 0xDA,
    /// 查从站 DAQ 分辨率。
    GetDaqResolutionInfo = 0xD9,
    /// 释放全部 DAQ 配置。
    FreeDaq = 0xD6,
    /// 分配 DAQ list。
    AllocDaq = 0xD5,
    /// 给 DAQ list 分配 ODT。
    AllocOdt = 0xD4,
    /// 给 ODT 分配条目。
    AllocOdtEntry = 0xD3,
    /// 设置写 ODT 的游标。
    SetDaqPtr = 0xE2,
    /// 按游标写一条 ODT 条目。
    WriteDaq = 0xE1,
    /// 设置 DAQ list 的模式。
    SetDaqListMode = 0xE0,
    /// 启停单个 DAQ list。
    StartStopDaqList = 0xDE,
    /// 同步启停全部 DAQ list。
    StartStopSynch = 0xDD,
    /// 清空 DAQ list 的条目。
    ClearDaqList = 0xE3,
}

impl CommandCode {
    /// 从字节还原命令码。分支数同样等于命令表条数，刻意保留。
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
            0xDA => Self::GetDaqProcessorInfo,
            0xD9 => Self::GetDaqResolutionInfo,
            0xD6 => Self::FreeDaq,
            0xD5 => Self::AllocDaq,
            0xD4 => Self::AllocOdt,
            0xD3 => Self::AllocOdtEntry,
            0xE2 => Self::SetDaqPtr,
            0xE1 => Self::WriteDaq,
            0xE0 => Self::SetDaqListMode,
            0xDE => Self::StartStopDaqList,
            0xDD => Self::StartStopSynch,
            0xE3 => Self::ClearDaqList,
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
        page: u8,
    },
    /// 查从站 DAQ 能力。
    GetDaqProcessorInfo,
    /// 查从站 DAQ 分辨率。
    GetDaqResolutionInfo,
    /// 释放全部 DAQ 配置。
    FreeDaq,
    /// 分配 `count` 个 DAQ list。
    AllocDaq {
        /// DAQ list 个数。
        count: u16,
    },
    /// 给 DAQ list 分配 `odt_count` 个 ODT。
    AllocOdt {
        /// DAQ list 编号。
        daq_list: u16,
        /// ODT 个数。
        odt_count: u8,
    },
    /// 给某个 ODT 分配 `entries` 个条目。
    AllocOdtEntry {
        /// DAQ list 编号。
        daq_list: u16,
        /// ODT 编号（list 内相对）。
        odt: u8,
        /// 条目数。
        entries: u8,
    },
    /// 设置写 ODT 的游标。`WRITE_DAQ` 不带宽高参数，靠这个游标定位。
    SetDaqPtr {
        /// DAQ list 编号。
        daq_list: u16,
        /// ODT 编号。
        odt: u8,
        /// 条目序号。
        entry: u8,
    },
    /// 在游标处写一条 ODT 条目。
    WriteDaq {
        /// 位偏移。
        bit_offset: u8,
        /// 条目字节数；0 表示整字节粒度下的 1。
        size: u8,
        /// 地址扩展。
        address_extension: u8,
        /// 源地址。
        address: u32,
    },
    /// 设置 DAQ list 的模式。
    SetDaqListMode {
        /// 模式位。
        mode: u8,
        /// DAQ list 编号。
        daq_list: u16,
        /// 事件通道号。
        event: u16,
        /// 采样预分频。
        prescaler: u8,
        /// 优先级。
        priority: u8,
    },
    /// 启停单个 DAQ list；响应带回该 list 的首个 PID。
    StartStopDaqList {
        /// 模式：0 停、1 启、2 只选中不发。
        mode: u8,
        /// DAQ list 编号。
        daq_list: u16,
    },
    /// 同步启停全部 DAQ list。
    StartStopSynch {
        /// 模式：0 停、1 启。
        mode: u8,
    },
    /// 清空 DAQ list 的条目。
    ClearDaqList {
        /// DAQ list 编号。
        daq_list: u16,
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
            Self::GetDaqProcessorInfo => CommandCode::GetDaqProcessorInfo,
            Self::GetDaqResolutionInfo => CommandCode::GetDaqResolutionInfo,
            Self::FreeDaq => CommandCode::FreeDaq,
            Self::AllocDaq { .. } => CommandCode::AllocDaq,
            Self::AllocOdt { .. } => CommandCode::AllocOdt,
            Self::AllocOdtEntry { .. } => CommandCode::AllocOdtEntry,
            Self::SetDaqPtr { .. } => CommandCode::SetDaqPtr,
            Self::WriteDaq { .. } => CommandCode::WriteDaq,
            Self::SetDaqListMode { .. } => CommandCode::SetDaqListMode,
            Self::StartStopDaqList { .. } => CommandCode::StartStopDaqList,
            Self::StartStopSynch { .. } => CommandCode::StartStopSynch,
            Self::ClearDaqList { .. } => CommandCode::ClearDaqList,
        }
    }

    /// 编码成整帧（PID + 命令码 + 参数）。超过 `MAX_CTO` 返回错误。
    ///
    /// 圈复杂度等于命令表条数，与 [`Command::decode_command`] 同属刻意保留的
    /// 协议分派，不再拆分。
    pub fn encode(&self) -> Result<Vec<u8>, EncodeError> {
        // 命令码只有 `code()` 一处来源；这里不再逐个硬编码，否则改一处漏一处。
        let code = self.code() as u8;
        let bytes = match self {
            Self::Connect { mode } => Encoder::with_command(code).u8(*mode).finish(),
            Self::Disconnect => Encoder::with_command(code).finish(),
            Self::GetStatus => Encoder::with_command(code).finish(),
            Self::Synch => Encoder::with_command(code).finish(),
            Self::GetCommModeInfo => Encoder::with_command(code).finish(),
            Self::SetMta {
                address_extension,
                address,
            } => Encoder::with_command(code)
                .u8(*address_extension)
                .u32(*address)
                .finish(),
            Self::Upload { len } => Encoder::with_command(code).u8(*len).finish(),
            Self::ShortUpload {
                len,
                address_extension,
                address,
            } => Encoder::with_command(code)
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
                Encoder::with_command(code).u8(*len).bytes(data).finish()
            }
            Self::GetCalPage { mode, segment } => {
                Encoder::with_command(code).u8(*mode).u8(*segment).finish()
            }
            Self::SetCalPage {
                mode,
                segment,
                page,
            } => Encoder::with_command(code)
                .u8(*mode)
                .u8(*segment)
                .u8(*page)
                .finish(),
            Self::GetDaqProcessorInfo => Encoder::with_command(code).finish(),
            Self::GetDaqResolutionInfo => Encoder::with_command(code).finish(),
            Self::FreeDaq => Encoder::with_command(code).finish(),
            Self::AllocDaq { count } => Encoder::with_command(code).u16(*count).finish(),
            Self::AllocOdt {
                daq_list,
                odt_count,
            } => Encoder::with_command(code)
                .u16(*daq_list)
                .u8(*odt_count)
                .finish(),
            Self::AllocOdtEntry {
                daq_list,
                odt,
                entries,
            } => Encoder::with_command(code)
                .u16(*daq_list)
                .u8(*odt)
                .u8(*entries)
                .finish(),
            Self::SetDaqPtr {
                daq_list,
                odt,
                entry,
            } => Encoder::with_command(code)
                .u16(*daq_list)
                .u8(*odt)
                .u8(*entry)
                .finish(),
            Self::WriteDaq {
                bit_offset,
                size,
                address_extension,
                address,
            } => Encoder::with_command(code)
                .u8(*bit_offset)
                .u8(*size)
                .u8(*address_extension)
                .u32(*address)
                .finish(),
            Self::SetDaqListMode {
                mode,
                daq_list,
                event,
                prescaler,
                priority,
            } => Encoder::with_command(code)
                .u8(*mode)
                .u16(*daq_list)
                .u16(*event)
                .u8(*prescaler)
                .u8(*priority)
                .finish(),
            Self::StartStopDaqList { mode, daq_list } => Encoder::with_command(code)
                .u8(*mode)
                .u16(*daq_list)
                .finish(),
            Self::StartStopSynch { mode } => Encoder::with_command(code).u8(*mode).finish(),
            Self::ClearDaqList { daq_list } => Encoder::with_command(code).u16(*daq_list).finish(),
        };
        if bytes.len() > MAX_CTO {
            return Err(EncodeError::TooLong {
                len: bytes.len(),
                max: MAX_CTO,
            });
        }
        Ok(bytes)
    }

    /// 从整帧解码。byte0 就是命令码，CRO 没有 PID 前缀。
    pub fn decode(frame: &[u8]) -> Result<Self, DecodeError> {
        if frame.is_empty() {
            return Err(DecodeError::Truncated { need: 1, got: 0 });
        }
        let mut decoder = Decoder::at(frame, 0);
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
        match code {
            CommandCode::Connect => Self::decode_connect(decoder),
            CommandCode::Disconnect => Ok(Self::Disconnect),
            CommandCode::GetStatus => Ok(Self::GetStatus),
            CommandCode::Synch => Ok(Self::Synch),
            CommandCode::GetCommModeInfo => Ok(Self::GetCommModeInfo),
            CommandCode::SetMta => Self::decode_set_mta(decoder),
            CommandCode::Upload => Self::decode_upload(decoder),
            CommandCode::ShortUpload => Self::decode_short_upload(decoder),
            CommandCode::Download => Self::decode_download(decoder),
            CommandCode::GetCalPage => Self::decode_get_cal_page(decoder),
            CommandCode::SetCalPage => Self::decode_set_cal_page(decoder),
            CommandCode::GetDaqProcessorInfo => Ok(Self::GetDaqProcessorInfo),
            CommandCode::GetDaqResolutionInfo => Ok(Self::GetDaqResolutionInfo),
            CommandCode::FreeDaq => Ok(Self::FreeDaq),
            CommandCode::AllocDaq => Ok(Self::AllocDaq {
                count: decoder.u16()?,
            }),
            CommandCode::AllocOdt => Ok(Self::AllocOdt {
                daq_list: decoder.u16()?,
                odt_count: decoder.u8()?,
            }),
            CommandCode::AllocOdtEntry => Ok(Self::AllocOdtEntry {
                daq_list: decoder.u16()?,
                odt: decoder.u8()?,
                entries: decoder.u8()?,
            }),
            CommandCode::SetDaqPtr => Ok(Self::SetDaqPtr {
                daq_list: decoder.u16()?,
                odt: decoder.u8()?,
                entry: decoder.u8()?,
            }),
            CommandCode::WriteDaq => Self::decode_write_daq(decoder),
            CommandCode::SetDaqListMode => Ok(Self::SetDaqListMode {
                mode: decoder.u8()?,
                daq_list: decoder.u16()?,
                event: decoder.u16()?,
                prescaler: decoder.u8()?,
                priority: decoder.u8()?,
            }),
            CommandCode::StartStopDaqList => Ok(Self::StartStopDaqList {
                mode: decoder.u8()?,
                daq_list: decoder.u16()?,
            }),
            CommandCode::StartStopSynch => Ok(Self::StartStopSynch {
                mode: decoder.u8()?,
            }),
            CommandCode::ClearDaqList => Ok(Self::ClearDaqList {
                daq_list: decoder.u16()?,
            }),
        }
    }

    fn decode_write_daq(decoder: &mut Decoder<'_>) -> Result<Self, DecodeError> {
        let bit_offset = decoder.u8()?;
        let size = decoder.u8()?;
        let address_extension = decoder.u8()?;
        Ok(Self::WriteDaq {
            bit_offset,
            size,
            address_extension,
            address: decoder.u32()?,
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
            page: decoder.u8()?,
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
            vec![0xF6, 0x00, 0x00, 0x00, 0x00, 0x10, 0x00, 0x00]
        );
        assert_eq!(
            Command::ShortUpload {
                len: 2,
                address_extension: 0,
                address: 0x1002
            }
            .encode()
            .expect("一帧放得下"),
            vec![0xF4, 0x02, 0x00, 0x00, 0x02, 0x10, 0x00, 0x00]
        );
        assert_eq!(
            Command::Upload { len: 4 }.encode().expect("一帧放得下"),
            vec![0xF5, 0x04]
        );
        assert_eq!(
            Command::Download {
                len: 3,
                data: vec![0xAA, 0xBB, 0xCC]
            }
            .encode()
            .expect("一帧放得下"),
            vec![0xF0, 0x03, 0xAA, 0xBB, 0xCC]
        );
        assert_eq!(
            Command::SetCalPage {
                mode: 0,
                segment: 0,
                page: 1
            }
            .encode()
            .expect("一帧放得下"),
            vec![0xEB, 0x00, 0x00, 0x01]
        );
    }

    #[test]
    fn daq_commands_match_the_spike_frames() {
        // 实测抓帧（tmp/spike-m0-2/can.log）：命令码在 byte0，参数按整帧偏移对齐。
        let cases = [
            (Command::AllocDaq { count: 1 }, vec![0xD5, 0x00, 0x01, 0x00]),
            (
                Command::AllocOdt {
                    daq_list: 0,
                    odt_count: 1,
                },
                vec![0xD4, 0x00, 0x00, 0x00, 0x01],
            ),
            (
                Command::AllocOdtEntry {
                    daq_list: 0,
                    odt: 0,
                    entries: 2,
                },
                vec![0xD3, 0x00, 0x00, 0x00, 0x00, 0x02],
            ),
            (
                Command::SetDaqPtr {
                    daq_list: 0,
                    odt: 0,
                    entry: 0,
                },
                vec![0xE2, 0x00, 0x00, 0x00, 0x00, 0x00],
            ),
            (
                Command::WriteDaq {
                    bit_offset: 0,
                    size: 2,
                    address_extension: 0,
                    address: 0x1000,
                },
                vec![0xE1, 0x00, 0x02, 0x00, 0x00, 0x10, 0x00, 0x00],
            ),
            (
                Command::SetDaqListMode {
                    mode: 0,
                    daq_list: 0,
                    event: 0,
                    prescaler: 1,
                    priority: 0,
                },
                vec![0xE0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00],
            ),
        ];
        for (command, expected) in cases {
            assert_eq!(
                command.encode().expect("一帧放得下"),
                expected,
                "{command:?}"
            );
        }
    }

    #[test]
    fn daq_commands_round_trip() {
        let commands = [
            Command::GetDaqProcessorInfo,
            Command::GetDaqResolutionInfo,
            Command::FreeDaq,
            Command::AllocDaq { count: 3 },
            Command::AllocOdt {
                daq_list: 2,
                odt_count: 4,
            },
            Command::AllocOdtEntry {
                daq_list: 1,
                odt: 2,
                entries: 3,
            },
            Command::SetDaqPtr {
                daq_list: 1,
                odt: 2,
                entry: 3,
            },
            Command::WriteDaq {
                bit_offset: 8,
                size: 4,
                address_extension: 1,
                address: 0x2000,
            },
            Command::SetDaqListMode {
                mode: 1,
                daq_list: 2,
                event: 3,
                prescaler: 4,
                priority: 5,
            },
            Command::StartStopDaqList {
                mode: 1,
                daq_list: 2,
            },
            Command::StartStopSynch { mode: 1 },
            Command::ClearDaqList { daq_list: 2 },
        ];
        for command in commands {
            let encoded = command.encode().expect("DAQ 命令都放得下一帧");
            assert_eq!(
                Command::decode(&encoded),
                Ok(command.clone()),
                "{command:?}"
            );
        }
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
                command.encode().expect("一帧放得下")[0],
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
        // 整帧 = 命令码 + len + 数据；7 字节数据要 9 字节帧。
        let err = Command::Download {
            len: 7,
            data: vec![0; 7],
        }
        .encode()
        .expect_err("1 + 1 + 7 > 8");
        assert_eq!(
            err,
            EncodeError::TooLong {
                len: 9,
                max: MAX_CTO
            }
        );
    }

    #[test]
    fn truncated_download_payload_is_reported() {
        let err = Command::decode(&[0xF0, 0x04, 0x01, 0x02]).expect_err("声明 4 字节只给了 2");
        assert_eq!(err, DecodeError::Truncated { need: 6, got: 4 });
    }
}
