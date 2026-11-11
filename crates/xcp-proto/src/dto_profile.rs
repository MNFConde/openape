//! 对端 DTO 布局档案（D-27，M5-12）：差异是数据，解释器只此一份。
//!
//! 标准 DTO 是 `[PID][数据]`，PID 从 0 起跨 DAQ list 连续分配（绝对号）。
//! 第三方实现未必照这个来：XCPlite 的 DTO 头是私有布局
//! `[相对 ODT][0xAA][DAQ list u16 LE][时间戳 u32 LE][数据]`，比标准多 7
//! 字节，且编号是 list 内相对号。本模块把「头长什么样」写成字段表
//! （[`DtoProfile`]），把「怎么切」留成唯一一份解释器（[`split`]）——
//! 接一台新从站只往档案表里加一行，不改解码逻辑。
//!
//! 三处刻意的边界：
//!
//! 1. **不猜**：字段表缺项、报文不够头长、时间戳位宽越界，一律报错。
//!    宁可这一帧不进样本，不要解出错位的数——解码错位不像报错那样看得见。
//! 2. **不含出向参数**：地址扩展、CRO 是否补齐都不在这里。那是「跟从站
//!    怎么对话」，不是「回来的字节怎么切」，混进来会让本模块变成半个
//!    连接配置（D-27 的「遗留待定」）。
//! 3. **时间戳只提取、不解释**：`Decoded::timestamp` 是从站时间戳的原始
//!    值；要不要进样本时间轴由 D-25 的地盘决定，本模块不代答。
//!
//! 档案与它的证据同处：`dto_profiles!` 一次生成常量与黄金样本测试，加
//! profile 不会有「忘了配测试」的路。宏只放数据、不放逻辑——这里一旦
//! 出现「某 profile 特有的解码闭包」，就退化回 D-27 反对的分支群。

/// DTO 头里的 PID 语义。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PidKind {
    /// 标准：跨 DAQ list 连续分配的绝对 ODT 编号。
    Absolute,
    /// 第三方私有：DAQ list 内的相对 ODT 编号，需配合 [`Decoded::daq_list`]
    /// 才能还原成绝对编号。
    RelativeToDaqList,
}

/// 字段表里的一项。表按从帧首到帧尾的顺序排列，[`Field::Payload`] 之后
/// 全是载荷。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    /// 跳过若干字节（保留位、标记位、对齐填充）。
    Skip(usize),
    /// 1 字节 PID。
    Pid,
    /// 2 字节小端 DAQ list 号。
    DaqList,
    /// 若干字节小端从站时间戳，位宽限 1..=4（装进 u32）。
    Timestamp(usize),
    /// 载荷起点，其后整段都是载荷。
    Payload,
}

impl Field {
    /// 本项占几个字节。载荷项本身不占字节（它标记起点）。
    pub const fn width(self) -> usize {
        match self {
            Self::Skip(len) => len,
            Self::Pid => 1,
            Self::DaqList => 2,
            Self::Timestamp(width) => width,
            Self::Payload => 0,
        }
    }
}

/// 一份对端 DTO 布局档案。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DtoProfile {
    /// 档案名，只用于错误文本与测试名，不参与解码。
    pub name: &'static str,
    /// PID 语义。取绝对号时 `Decoded::pid` 直接就是 ODT 编号；取相对号时
    /// 还要配 [`Decoded::daq_list`] 才能定位（映射由接线的调用方做）。
    pub pid: PidKind,
    /// 字段表：从帧首到载荷起点。
    pub header: &'static [Field],
}

impl DtoProfile {
    /// 载荷起点相对帧首的偏移，也就是「至少要有多少字节」。
    pub fn header_len(&self) -> usize {
        self.header
            .iter()
            .take_while(|field| **field != Field::Payload)
            .map(|field| field.width())
            .sum()
    }
}

/// 一条 DTO 按档案切出来的结果。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Decoded<'a> {
    /// 原始 PID 字节（语义见 [`DtoProfile::pid`]）。
    pub pid: u8,
    /// 头部带了 DAQ list 号时给出。
    pub daq_list: Option<u16>,
    /// 头部带了从站时间戳时给出，已按小端还原。
    pub timestamp: Option<u32>,
    /// 载荷。
    pub payload: &'a [u8],
}

/// 切帧失败的原因。全部是「档案或报文不对」，没有「大概是……」的分支。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfileError {
    /// 档案里没有 PID 项（或在载荷之前没有）。
    MissingPid {
        /// 档案名。
        name: &'static str,
    },
    /// 档案里没有载荷起点。
    MissingPayload {
        /// 档案名。
        name: &'static str,
    },
    /// 报文不够档案声明的头长。
    TooShort {
        /// 档案名。
        name: &'static str,
        /// 头需要的字节数。
        need: usize,
        /// 实收字节数。
        len: usize,
    },
    /// 时间戳位宽超出 u32：档案写错了。
    TimestampWidth {
        /// 档案名。
        name: &'static str,
        /// 档案里写的位宽。
        width: usize,
    },
}

impl std::fmt::Display for ProfileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingPid { name } => {
                write!(f, "DTO 档案 {name} 的字段表里没有 Pid 项")
            }
            Self::MissingPayload { name } => {
                write!(f, "DTO 档案 {name} 的字段表里没有 Payload 项")
            }
            Self::TooShort { name, need, len } => write!(
                f,
                "DTO 档案 {name} 要 {need} 字节的头，实收 {len} 字节——多半是包被截断或档案配错"
            ),
            Self::TimestampWidth { name, width } => write!(
                f,
                "DTO 档案 {name} 的时间戳位宽 {width} 字节，只支持 1..=4（要装进 u32）"
            ),
        }
    }
}

impl std::error::Error for ProfileError {}

/// 按档案把一条 DTO 切成 PID、可选的 list 号与时间戳、以及载荷。
///
/// 缺字节就报 [`ProfileError::TooShort`]，不补零、不猜长度；档案缺项也报错。
pub fn split<'a>(profile: &DtoProfile, message: &'a [u8]) -> Result<Decoded<'a>, ProfileError> {
    let name = profile.name;
    if !profile.header.contains(&Field::Pid) {
        return Err(ProfileError::MissingPid { name });
    }
    if !profile.header.contains(&Field::Payload) {
        return Err(ProfileError::MissingPayload { name });
    }
    for field in profile.header {
        if let Field::Timestamp(width) = *field
            && !(1..=4).contains(&width)
        {
            return Err(ProfileError::TimestampWidth { name, width });
        }
    }

    let need = profile.header_len();
    if message.len() < need {
        return Err(ProfileError::TooShort {
            name,
            need,
            len: message.len(),
        });
    }

    let (mut pid, mut daq_list, mut timestamp) = (None, None, None);
    let mut at = 0;
    for field in profile.header {
        match *field {
            Field::Skip(len) => at += len,
            Field::Pid => {
                pid = Some(message[at]);
                at += 1;
            }
            Field::DaqList => {
                daq_list = Some(u16::from_le_bytes([message[at], message[at + 1]]));
                at += 2;
            }
            Field::Timestamp(width) => {
                timestamp = Some(le_u32(&message[at..at + width]));
                at += width;
            }
            Field::Payload => {
                let pid = pid.ok_or(ProfileError::MissingPid { name })?;
                return Ok(Decoded {
                    pid,
                    daq_list,
                    timestamp,
                    payload: &message[at..],
                });
            }
        }
    }
    // 上面已确认表里有 Payload，循环必然在那一项返回。
    Err(ProfileError::MissingPayload { name })
}

/// 小端还原成 u32，位宽按实际字节数（1..=4）。
fn le_u32(bytes: &[u8]) -> u32 {
    bytes
        .iter()
        .rev()
        .fold(0, |acc, byte| (acc << 8) | u32::from(*byte))
}

/// 一次声明若干 DTO 档案，并为每个档案生成黄金样本测试。
///
/// 写法：`常量名 => "档案名", PID 语义, [字段表] { 用例名 { [输入字节] => (pid, list, 时间戳, [载荷]) }, .. };`
///
/// 约束：每个档案的用例名在本档案内唯一即可（测试落在 `golden::<常量名>`
/// 子模块里，跨档案同名不冲突）；宏体只做转发，不许往里塞逻辑。
macro_rules! dto_profiles {
    ($(
        $konst:ident => $name:literal, $pid:ident, [$($field:expr),*] {
            $(
                $case:ident { [$($byte:expr),*] => ($w_pid:expr, $w_list:expr, $w_ts:expr, [$($want:expr),*]) }
            ),* $(,)?
        }
    );* $(;)?) => {
        $(
            /// 见本模块文档与 D-27。
            pub const $konst: DtoProfile = DtoProfile {
                name: $name,
                pid: PidKind::$pid,
                header: &[$($field),*],
            };
        )*

        #[cfg(test)]
        mod golden {
            use super::*;
            $(
                // 模块名取自 $konst（档案名本身全大写），改不成 snake_case。
                // 这里保留 allow 而不是统一成 expect：它落在 macro_rules! 的展开
                // 体内，某次展开若没触发这条 lint，expect 会报 unfulfilled，收益为零。
                #[allow(non_snake_case)]
                mod $konst {
                    use super::*;
                    $(
                        #[test]
                        fn $case() {
                            let input: &[u8] = &[$($byte),*];
                            let want = Decoded {
                                pid: $w_pid,
                                daq_list: $w_list,
                                timestamp: $w_ts,
                                payload: &[$($want),*],
                            };
                            let got = split(&$konst, input).unwrap_or_else(|err| {
                                panic!("档案 {} 的黄金样本必须能解：{err}；输入 {input:02X?}", $name)
                            });
                            assert_eq!(got, want, "档案 {} 的布局与黄金样本不符", $name);
                        }
                    )*
                }
            )*
        }
    };
}

dto_profiles! {
    STANDARD => "standard", Absolute, [Field::Pid, Field::Payload] {
        pid_then_payload { [0x01, 0xAA, 0xBB] => (0x01, None, None, [0xAA, 0xBB]) },
        empty_payload    { [0x07]                   => (0x07, None, None, []) },
    };
    XCPLITE => "xcplite", RelativeToDaqList,
        [Field::Pid, Field::Skip(1), Field::DaqList, Field::Timestamp(4), Field::Payload] {
        // 实测布局：[相对 ODT][0xAA 标记][DAQ list LE][时间戳 LE][载荷]
        private_header { [0x00, 0xAA, 0x00, 0x00, 0x03, 0x02, 0x01, 0x00, 0x2A, 0x00, 0x00, 0x00]
                         => (0x00, Some(0x0000), Some(0x0001_0203), [0x2A, 0x00, 0x00, 0x00]) },
        header_only    { [0x05, 0xAA, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00]
                         => (0x05, Some(0x0001), Some(0), []) },
    };
}

#[cfg(test)]
mod rejects {
    use super::*;

    #[test]
    fn truncated_xcplite_header_is_an_error() {
        assert_eq!(
            split(&XCPLITE, &[0x00, 0xAA, 0x00]),
            Err(ProfileError::TooShort {
                name: "xcplite",
                need: 8,
                len: 3,
            })
        );
    }

    #[test]
    fn empty_message_is_short_even_for_the_standard_profile() {
        assert_eq!(
            split(&STANDARD, &[]),
            Err(ProfileError::TooShort {
                name: "standard",
                need: 1,
                len: 0,
            })
        );
    }

    #[test]
    fn profile_without_payload_is_rejected() {
        let broken = DtoProfile {
            name: "broken",
            pid: PidKind::Absolute,
            header: &[Field::Pid],
        };
        assert_eq!(
            split(&broken, &[0x01, 0x02]),
            Err(ProfileError::MissingPayload { name: "broken" })
        );
    }

    #[test]
    fn profile_without_pid_is_rejected() {
        let broken = DtoProfile {
            name: "broken",
            pid: PidKind::Absolute,
            header: &[Field::DaqList, Field::Payload],
        };
        assert_eq!(
            split(&broken, &[0x01, 0x02]),
            Err(ProfileError::MissingPid { name: "broken" })
        );
    }

    #[test]
    fn pid_after_payload_is_not_silently_accepted() {
        // 字段表把 Payload 写在 Pid 之前：到载荷时还没取到 PID，报错而非补默认值。
        let broken = DtoProfile {
            name: "broken",
            pid: PidKind::Absolute,
            header: &[Field::Payload, Field::Pid],
        };
        assert_eq!(
            split(&broken, &[0x01, 0x02]),
            Err(ProfileError::MissingPid { name: "broken" })
        );
    }

    #[test]
    fn timestamp_wider_than_u32_is_rejected() {
        let broken = DtoProfile {
            name: "broken",
            pid: PidKind::Absolute,
            header: &[Field::Pid, Field::Timestamp(8), Field::Payload],
        };
        assert_eq!(
            split(&broken, &[0; 12]),
            Err(ProfileError::TimestampWidth {
                name: "broken",
                width: 8,
            })
        );
    }

    #[test]
    fn header_len_matches_the_walk() {
        assert_eq!(STANDARD.header_len(), 1, "标准头只有 PID");
        assert_eq!(XCPLITE.header_len(), 8, "XCPlite 私有头 8 字节");
    }
}
