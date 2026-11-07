//! Vector ASC 文本日志的解析。
//!
//! 只认经典 CAN 的数据帧行：`<时间秒> <通道> Tx|Rx <id> <dlc> [字节...]`。
//! 认不出的行（统计行、注释、错误帧、FD 帧）跳过——日志里混着我们不
//! 关心的行是常态；但"认出是帧行却解析不了"要报错并带行号，不静默丢帧。
//! 时间相对首帧归零：回放的起点是第一帧，不是文件的 date 头。

use crate::ReplayFrame;

use openape_bus::{CanFrame, CanId};

use crate::ReplayError;

/// 把一份 ASC 文本解成回放帧序列。
pub fn parse(text: &str) -> Result<Vec<ReplayFrame>, ReplayError> {
    let mut base_seconds: Option<f64> = None;
    let mut frames = Vec::new();
    for (index, raw) in text.lines().enumerate() {
        let line = raw.trim();
        let words: Vec<&str> = line.split_whitespace().collect();
        // 帧行的形状：时间、通道、方向。方向不是 Tx/Rx 的行不是帧，跳过。
        if !matches!(words.get(2), Some(&"Tx") | Some(&"Rx")) {
            continue;
        }
        let line_no = index + 1;
        let bad = |detail: String| ReplayError::Parse {
            line: line_no,
            detail,
        };
        let seconds: f64 = words[0]
            .parse()
            .map_err(|_| bad(format!("时间戳不是数字：{}", words[0])))?;
        let id_text = words[3].trim_start_matches("0x").trim_start_matches("0X");
        let id_value = u32::from_str_radix(id_text, 16)
            .map_err(|_| bad(format!("帧 ID 不是十六进制：{id_text}")))?;
        let dlc: usize = words[4]
            .parse()
            .map_err(|_| bad(format!("DLC 不是数字：{}", words[4])))?;
        if dlc > 8 {
            return Err(bad(format!(
                "DLC {dlc} 超出经典 CAN 的 8 字节（FD 帧暂不支持）"
            )));
        }
        let mut data = Vec::with_capacity(dlc);
        for word in words.iter().skip(5).take(dlc) {
            let byte = u8::from_str_radix(word, 16)
                .map_err(|_| bad(format!("数据字节不是十六进制：{word}")))?;
            data.push(byte);
        }
        if data.len() != dlc {
            return Err(bad(format!("DLC 写 {dlc}，但数据只有 {} 字节", data.len())));
        }
        // ID 列超过 3 个十六进制位即扩展帧（标准帧是 11 位 = 最多 3 位）。
        let id = if id_text.len() > 3 {
            CanId::extended(id_value).ok_or_else(|| bad(format!("扩展 ID {id_text} 超出 29 位")))?
        } else {
            let raw = u16::try_from(id_value)
                .map_err(|_| bad(format!("标准 ID {id_text} 超出 16 位")))?;
            CanId::standard(raw).ok_or_else(|| bad(format!("标准 ID {id_text} 超出 11 位")))?
        };
        let frame = CanFrame::new(id, data).ok_or_else(|| bad("数据长度超出帧上限".to_owned()))?;
        let base = *base_seconds.get_or_insert(seconds);
        let offset_us = ((seconds - base) * 1e6).round().max(0.0) as u64;
        frames.push(ReplayFrame { offset_us, frame });
    }
    Ok(frames)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(id: u16, data: &[u8]) -> CanFrame {
        CanFrame::new(CanId::standard(id).expect("合法"), data.to_vec()).expect("帧合法")
    }

    #[test]
    fn sample_lines_become_frames_with_offsets_from_the_first() {
        let text = "\
date Sun Nov 09 12:00:00 2026
base hex timestamps absolute
internal events logged
Begin Triggerblock Sun Nov 09 12:00:00 2026
   0.000000 1  Tx   600   3   40 01 02
   0.010000 1  Rx   601   8   FF 05 00 08 00 08 00 7F
   0.025500 1  Tx   7FF   1   42
End TriggerBlock
";
        let frames = parse(text).expect("样本必须能解");
        assert_eq!(frames.len(), 3, "只认三行帧，头尾与陌生行跳过");
        assert_eq!(frames[0].offset_us, 0);
        assert_eq!(frames[0].frame, frame(0x600, &[0x40, 0x01, 0x02]));
        assert_eq!(frames[1].offset_us, 10_000);
        assert_eq!(frames[2].offset_us, 25_500);
        assert_eq!(frames[2].frame, frame(0x7FF, &[0x42]));
    }

    #[test]
    fn frame_line_with_a_broken_payload_reports_the_line() {
        let text = "0.000000 1  Tx   600   2   11 zz\n";
        let error = parse(text).expect_err("坏字节必须报错");
        let ReplayError::Parse { line, detail } = error else {
            panic!("要的是 Parse");
        };
        assert_eq!(line, 1);
        assert!(detail.contains("zz"), "{detail}");
    }

    #[test]
    fn dlc_beyond_classic_can_is_named_not_panic() {
        let text = "0.000000 1  Tx   600   64   11\n";
        let error = parse(text).expect_err("FD 长度必须报错");
        assert!(error.to_string().contains("FD"), "{error}");
    }

    #[test]
    fn unknown_lines_are_skipped_silently() {
        let text = "\
0.000000 Statistic: whatever
ErrorFrame 0.000100
0.000200 1  Tx   100   1   01
";
        let frames = parse(text).expect("陌生行跳过，帧行照收");
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].offset_us, 0, "首帧即回放起点");
    }
}
