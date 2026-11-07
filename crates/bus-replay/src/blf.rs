//! Vector BLF 二进制日志的读取——ablf 之上的薄转换。
//!
//! 分工同 format-mf4：容器解析（压缩、块链接、对齐）交给 ablf（MIT/
//! Apache 双许可的 clean-room 实现），本模块只管语义——CAN_MESSAGE2
//! 对象变成回放帧。ablf 的迭代器静默跳过损坏对象继续读，这里不补救：
//! 日志是观测数据，残件按残件处理，坏到读不下去时迭代自然结束，已有
//! 的帧照常回放。
//!
//! 时间：BLF 对象头的时间戳是相对测量起点的纳秒，这里以首个 CAN 帧
//! 为回放零点折成微秒。尚未经真实 BLF 样本核对（公开样本许可不干净，
//! 见 input-tiers 的三道门），样本入手后补一轮读回验证。

use std::io::{BufRead, Seek};

use ablf::{BlfFile, ObjectTypes};

use crate::{ReplayError, ReplayFrame};

use openape_bus::{CanFrame, CanId};

/// 从一个 BLF 读端解出回放帧序列。
pub fn frames<R: BufRead + Seek>(reader: R) -> Result<Vec<ReplayFrame>, ReplayError> {
    let file = BlfFile::from_reader(reader).map_err(|(error, _)| ReplayError::Blf(error))?;
    let mut base_ns: Option<u64> = None;
    let mut frames = Vec::new();
    for object in file {
        let ObjectTypes::CanMessage86(message) = object.data else {
            continue;
        };
        let base = *base_ns.get_or_insert(message.header.timestamp_ns);
        frames.push(replay_frame(
            base,
            message.id,
            message.header.timestamp_ns,
            message.data.clone(),
        )?);
    }
    Ok(frames)
}

/// 一条 CAN_MESSAGE2 的四样语义（ID、时间、数据）→ 回放帧。
///
/// 拆成裸参数是因为 ablf 的对象带私有字段，测试没法手造。BLF 的 ID
/// 位 31 置位表示扩展帧，去掉标记后按 29 位建；标准帧按 11 位。
fn replay_frame(
    base_ns: u64,
    id: u32,
    timestamp_ns: u64,
    data: Vec<u8>,
) -> Result<ReplayFrame, ReplayError> {
    let id_value = id & 0x1FFF_FFFF;
    let extended = id & 0x8000_0000 != 0;
    let can_id = if extended {
        CanId::extended(id_value).ok_or_else(|| ReplayError::Parse {
            line: 0,
            detail: format!("扩展 ID {id_value:#x} 超出 29 位"),
        })?
    } else {
        let narrow = u16::try_from(id_value).map_err(|_| ReplayError::Parse {
            line: 0,
            detail: format!("标准 ID {id_value:#x} 超出 16 位"),
        })?;
        CanId::standard(narrow).ok_or_else(|| ReplayError::Parse {
            line: 0,
            detail: format!("标准 ID {id_value:#x} 超出 11 位"),
        })?
    };
    let frame = CanFrame::new(can_id, data).ok_or_else(|| ReplayError::Parse {
        line: 0,
        detail: "数据长度超出帧上限".to_owned(),
    })?;
    Ok(ReplayFrame {
        offset_us: timestamp_ns.saturating_sub(base_ns) / 1_000,
        frame,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standard_frame_becomes_a_replay_frame_with_relative_time() {
        // 基准 1 ms，对象时间 3.5 ms → 偏移 2.5 ms。
        let replay =
            replay_frame(1_000_000, 0x601, 3_500_000, vec![0x11, 0x22]).expect("标准帧必须能转");
        assert_eq!(replay.offset_us, 2_500);
        assert_eq!(
            replay.frame,
            CanFrame::new(CanId::standard(0x601).expect("合法"), vec![0x11, 0x22]).expect("帧合法")
        );
    }

    #[test]
    fn first_object_is_the_replay_zero() {
        let replay =
            replay_frame(7_000_000_000, 0x100, 7_000_000_000, vec![0x01]).expect("首对象必须能转");
        assert_eq!(replay.offset_us, 0, "回放零点就是首帧");
    }

    #[test]
    fn extended_flag_in_bit31_is_stripped_for_the_29_bit_id() {
        let replay = replay_frame(0, 0x8000_0001, 0, vec![0xAA]).expect("扩展帧必须能转");
        assert_eq!(
            replay.frame,
            CanFrame::new(CanId::extended(1).expect("合法"), vec![0xAA]).expect("帧合法")
        );
    }
}
