//! 验收（M3-08）：与 `can-dbc` 逐条对照。
//!
//! DBC 没有公开规范，`can-dbc` 只是另一个实现，不是标准答案。所以判据不是
//! "完全一致"，而是"差异被逐条列出来，并说明采纳哪边为什么"——把无关边角
//! 卡成阻塞，比实现差异本身更浪费时间。
//!
//! 目前两边在夹具上零差异，因此下面的断言取"无差异"这个更强的形式。夹具
//! 一旦覆盖到边角情形而产生分歧，就在这里改成分项列举。

use std::path::Path;

use can_dbc::{ByteOrder as TheirOrder, DBC, ValueType};
use openape_format_dbc::{ByteOrder as OurOrder, Database, load_str};

fn fixture() -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/dbc/typical.dbc");
    std::fs::read_to_string(path).expect("DBC 夹具必须入库")
}

fn ours() -> Database {
    load_str(&fixture()).expect("我们要能读")
}

fn theirs() -> DBC {
    DBC::from_slice(fixture().as_bytes()).expect("can-dbc 必须能读同一份夹具")
}

#[test]
fn message_ids_names_and_lengths_agree() {
    let ours = ours();
    let theirs = theirs();
    for message in theirs.messages() {
        let id = message.message_id().raw();
        let mine = ours
            .message(id)
            .unwrap_or_else(|| panic!("我们没有消息 {id:#X}"));
        assert_eq!(mine.name(), message.message_name(), "消息 {id:#X} 名字不同");
        assert_eq!(
            mine.dlc(),
            *message.message_size() as usize,
            "消息 {id:#X} 长度不同"
        );
    }
    assert_eq!(
        ours.messages().count(),
        theirs.messages().len(),
        "消息条数不同：多出来的那边说明解析漏了或多吃了"
    );
}

#[test]
fn signal_geometry_agrees_signal_by_signal() {
    let ours = ours();
    let theirs = theirs();
    let mut diffs = Vec::new();
    for message in theirs.messages() {
        let mine = ours.message(message.message_id().raw()).expect("消息在");
        for signal in message.signals() {
            let name = signal.name();
            let Some(ours_signal) = mine.signal(name) else {
                diffs.push(format!("{name} 我们没解析出来"));
                continue;
            };
            let their_order = match signal.byte_order() {
                TheirOrder::LittleEndian => OurOrder::Intel,
                TheirOrder::BigEndian => OurOrder::Motorola,
            };
            let their_signed = matches!(signal.value_type(), ValueType::Signed);
            if ours_signal.start_bit() as u64 != signal.start_bit {
                diffs.push(format!(
                    "{name} 起始位：我们 {} / 他们 {}",
                    ours_signal.start_bit(),
                    signal.start_bit
                ));
            }
            if u64::from(ours_signal.len()) != signal.signal_size {
                diffs.push(format!(
                    "{name} 位宽：我们 {} / 他们 {}",
                    ours_signal.len(),
                    signal.signal_size
                ));
            }
            if ours_signal.byte_order() != their_order {
                diffs.push(format!("{name} 字节序不同"));
            }
            if ours_signal.is_signed() != their_signed {
                diffs.push(format!("{name} 符号性不同"));
            }
            for (field, mine_value, their_value) in [
                ("因子", ours_signal.factor(), signal.factor),
                ("偏移", ours_signal.offset(), signal.offset),
                ("下限", ours_signal.min(), signal.min),
                ("上限", ours_signal.max(), signal.max),
            ] {
                if (mine_value - their_value).abs() > f64::EPSILON {
                    diffs.push(format!(
                        "{name} {field}：我们 {mine_value} / 他们 {their_value}"
                    ));
                }
            }
        }
    }
    assert!(
        diffs.is_empty(),
        "与 can-dbc 存在差异：\n{}",
        diffs.join("\n")
    );
}

#[test]
fn our_decoder_covers_every_signal_they_report() {
    let ours = ours();
    let theirs = theirs();
    for message in theirs.messages() {
        let mine = ours.message(message.message_id().raw()).expect("消息在");
        assert_eq!(
            mine.signals().len(),
            message.signals().len(),
            "消息 {} 的信号条数不同",
            message.message_name()
        );
        for signal in message.signals() {
            assert!(
                mine.signal(signal.name()).is_some(),
                "消息 {} 少了信号 {}",
                message.message_name(),
                signal.name()
            );
        }
    }
}

#[test]
fn units_agree_where_they_declare_one() {
    // 单位是展示用的，但两边读出不同的单位，说明解析在引号与转义上跑偏了。
    let ours = ours();
    let theirs = theirs();
    for message in theirs.messages() {
        let mine = ours.message(message.message_id().raw()).expect("消息在");
        for signal in message.signals() {
            let ours_signal = mine.signal(signal.name()).expect("信号在");
            assert_eq!(
                ours_signal.unit(),
                signal.unit(),
                "{} 的单位不同",
                signal.name()
            );
        }
    }
}
