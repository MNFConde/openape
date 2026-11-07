//! 验收（M3-01）：fixtures ELF 的抽取结果与 `fixtures/elf/*.c` 的声明一致。
//!
//! 断言的是用户看得见的事实——"这个变量在 ELF 里是什么形状"，不碰内部
//! 实现怎么遍历 DWARF。

use openape_format_elf::{ElfSource, Field, Scalar, Shape, load};

fn golden() -> ElfSource {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/elf/test.elf");
    load(std::path::Path::new(path)).expect("golden ELF 必须能读")
}

fn scalar(name: &str) -> Scalar {
    match golden().get(name).expect("变量必须存在").shape() {
        Shape::Scalar(inner) => *inner,
        other => panic!("{name} 应为标量，实为 {other:?}"),
    }
}

#[test]
fn scalar_constants_carry_their_element_type() {
    assert_eq!(scalar("cal_kp"), Scalar::F32);
    assert_eq!(scalar("cal_folded"), Scalar::F32);
    assert_eq!(golden().get("cal_kp").expect("cal_kp 存在").size(), 4);
}

#[test]
fn arrays_carry_every_dimension_in_declaration_order() {
    let map = golden();
    let var = map.get("cal_map").expect("二维 Map 必须抽到");
    assert_eq!(
        var.shape(),
        &Shape::Array {
            elem: Scalar::F32,
            dims: vec![4, 3]
        },
        "cal_map[4][3] 的维度顺序必须与声明一致"
    );
    assert_eq!(var.size(), 4 * 4 * 3);

    let axis = map.get("cal_axis_x").expect("轴数组必须抽到");
    assert_eq!(
        axis.shape(),
        &Shape::Array {
            elem: Scalar::F32,
            dims: vec![3]
        }
    );
}

#[test]
fn structs_carry_member_offsets() {
    let source = golden();
    let gains = source.get("cal_gains").expect("结构体必须抽到");
    let Shape::Struct { members } = gains.shape() else {
        panic!("cal_gains 应为结构体，实为 {:?}", gains.shape());
    };
    assert_eq!(members.len(), 2);
    assert_eq!(
        members[0],
        Field::new("kp", 0, Shape::Scalar(Scalar::F32)).expect("成员名非空")
    );
    assert_eq!(
        members[1],
        Field::new("ki", 4, Shape::Scalar(Scalar::F32)).expect("成员名非空")
    );
    assert_eq!(gains.size(), 8);
}

#[test]
fn enums_carry_their_value_table() {
    let source = golden();
    let mode = source.get("cal_mode").expect("枚举必须抽到");
    let Shape::Enum { values, signed } = mode.shape() else {
        panic!("cal_mode 应为枚举，实为 {:?}", mode.shape());
    };
    assert!(!signed, "mode_t 的底层是无符号的");
    assert_eq!(
        values,
        &vec![
            ("MODE_IDLE".to_owned(), 0),
            ("MODE_RUN".to_owned(), 1),
            ("MODE_FAULT".to_owned(), 2),
        ]
    );
}

#[test]
fn bitfields_carry_their_bit_position() {
    let source = golden();
    let bits = source.get("obs_bits").expect("位域结构体必须抽到");
    let Shape::Struct { members } = bits.shape() else {
        panic!("obs_bits 应为结构体，实为 {:?}", bits.shape());
    };
    let shapes: Vec<&Shape> = members.iter().map(Field::shape).collect();
    assert_eq!(
        shapes,
        vec![
            &Shape::Bits {
                base: Scalar::U32,
                bit_offset: 0,
                bit_width: 3
            },
            &Shape::Bits {
                base: Scalar::U32,
                bit_offset: 3,
                bit_width: 5
            },
            &Shape::Bits {
                base: Scalar::U32,
                bit_offset: 8,
                bit_width: 8
            },
        ]
    );
}

#[test]
fn variables_folded_away_by_the_optimizer_are_absent() {
    // s_folded 是 -O2 下被完全折叠的 static const：符号表与 DWARF 里都没有。
    // 它缺席不是缺陷，而是"抽取只报存在的东西"的证据。
    assert!(
        golden().get("s_folded").is_none(),
        "被折叠的变量不该凭空出现"
    );
}

#[test]
fn cross_unit_duplicates_are_flagged_not_merged() {
    let source = golden();
    let same: Vec<&openape_format_elf::RawVar> = source
        .vars()
        .iter()
        .filter(|var| var.name() == "s_private_gain")
        .collect();
    assert_eq!(same.len(), 2, "calib.c 与 other.c 各有一份同名 static");
    assert!(
        same.iter().all(|var| !var.is_unique()),
        "同名跨 CU 必须标为不唯一，交给上层加前缀"
    );
}

#[test]
fn vars_are_sorted_by_symbol() {
    let source = golden();
    let names: Vec<&str> = source.vars().iter().map(|var| var.name()).collect();
    let mut sorted = names.clone();
    sorted.sort_unstable();
    assert_eq!(names, sorted, "输出顺序必须稳定，否则 golden 无法比对");
}
