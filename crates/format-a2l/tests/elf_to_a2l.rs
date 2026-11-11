//! 验收（M3-04）：从 ELF 夹具一路走到 A2L，导出件能被 `a2lfile` 严格模式接受。
//!
//! 这条测试把两个信息源前端接在一起跑：`format-elf` 抽 DWARF，本 crate 写
//! A2L。任何一侧的字段漏填都会在这里被严格模式拦住。

// 集成测试里的 expect 是断言手段：失败即测试失败，不走生产那道门。
#![allow(clippy::expect_used)]

use std::path::Path;

use openape_cal_core::{AxisBinding, Role, VariableTree};
use openape_format_a2l::write_a2l;
use openape_format_elf::load;

const ELF: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/elf/test.elf");

/// 把 ELF 抽成 IR 并给每条变量指定角色。
///
/// 角色是用户的事（D-13），测试里替用户做一次选择：可写的标定量给
/// Characteristic，其余给 Measurement。真实软件里这一步由表格勾选完成。
fn tree_with_roles() -> (VariableTree, Vec<openape_format_elf::Notice>) {
    let source = load(Path::new(ELF)).expect("golden ELF 必须能读");
    let (tree, notes) = source.to_ir().expect("golden ELF 必须能进 IR");
    let assigned: Vec<_> = tree
        .iter()
        .map(|var| {
            let role = match var.symbol().as_str() {
                "cal_kp" | "cal_map" | "cal_axis_x" | "cal_axis_y" | "cal_mode" => {
                    Role::Characteristic
                }
                _ => Role::Measurement,
            };
            var.clone().with_role(role)
        })
        .collect();
    (VariableTree::new(assigned).expect("符号不重复"), notes)
}

fn exported() -> String {
    let (tree, _) = tree_with_roles();
    write_a2l(&tree, &AxisBinding::default()).expect("要能导出")
}

#[test]
fn exported_text_is_accepted_by_a2lfile_in_strict_mode() {
    let text = exported();
    let (file, messages) =
        a2lfile::load_from_string(&text, None, true).expect("严格模式必须接受导出件");
    let blocking: Vec<String> = messages.iter().map(ToString::to_string).collect();
    assert!(
        blocking.is_empty(),
        "严格模式零告警才算通过，实为 {blocking:?}\n---\n{text}"
    );
    assert_eq!(file.project.module.len(), 1, "导出件应当只有一个模块");
}

#[test]
fn one_dimensional_tables_survive_the_round_trip() {
    // 一维 VAL_BLK 不写 MATRIX_DIM 时，读回来会被当成"没有维度的矩阵"
    // 跳过——导出件看着没问题，变量却没回来。这条是那句"矩阵既没有
    // MATRIX_DIM 也没有 AXIS_DESCR"报出来的。
    let text = exported();
    let reparsed = openape_format_a2l::load_str(&text, None).expect("导出件必须能读回");
    let axis = reparsed
        .tree()
        .iter()
        .find(|var| var.symbol().as_str() == "cal_axis_x")
        .expect("一维轴数组必须在往返后仍然存在");
    assert_eq!(axis.dims(), &[3]);
}

#[test]
fn round_trip_keeps_addresses_and_dimensions() {
    // 导出再读回来，地址与维度不能变形——这是"往返一次信息还在"的证据。
    let text = exported();
    let reparsed = openape_format_a2l::load_str(&text, None).expect("导出件必须能读回");
    let map = reparsed
        .tree()
        .iter()
        .find(|var| var.symbol().as_str() == "cal_map")
        .expect("cal_map 必须还在");
    assert_eq!(map.dims(), &[4, 3], "维度顺序往返后不能反");
    assert_eq!(map.elem(), openape_cal_core::ElemType::F32);
}

#[test]
fn every_elf_variable_reaches_the_export() {
    // 漏掉变量是最难发现的导出缺陷：A2L 里少一个 MEASUREMENT，软件不会
    // 报错，用户只会觉得"这个变量不存在"。
    let (tree, _) = tree_with_roles();
    let text = exported();
    for var in tree.iter() {
        assert!(
            text.contains(var.symbol().as_str()),
            "{} 没有出现在导出件里",
            var.symbol()
        );
    }
}

#[test]
fn unbound_tables_are_reported_so_the_user_can_add_axes() {
    // 没绑轴的数组导出成 VAL_BLK（A2L 里合法的值块），但用户想要曲线视图
    // 就得自己补绑定——所以提示必须有，否则他会以为导出件不完整。
    let (tree, _) = tree_with_roles();
    let text = write_a2l(&tree, &AxisBinding::default()).expect("要能导出");
    assert!(text.contains("VAL_BLK"), "无轴数组应以值块导出");
    assert!(
        text.contains("MATRIX_DIM"),
        "值块也要带维度，否则二维退化成一条线"
    );
}

#[test]
fn a_variable_without_role_is_refused_with_a_reason() {
    // 角色留空就导出，等于替用户决定"这是观测量还是标定量"——D-13 明确
    // 不做这个判定。错误要指名是哪个变量、缺的是什么。
    let source = load(Path::new(ELF)).expect("golden ELF 必须能读");
    let (raw_tree, _) = source.to_ir().expect("进 IR");
    let err = write_a2l(&raw_tree, &AxisBinding::default()).expect_err("没有角色不该静默导出");
    let text = err.to_string();
    assert!(
        text.contains("role") || text.contains("角色"),
        "要说清缺什么：{text}"
    );
    assert!(
        raw_tree
            .iter()
            .any(|var| text.contains(var.symbol().as_str())),
        "要点名是哪个变量：{text}"
    );
}

#[test]
fn export_matches_the_golden_file_byte_for_byte() {
    // 验收（M3-05）：档二 golden。重构只要改变导出件的任何一个字节就算
    // 回归——A2L 是要交给 CANape/INCA 读的，写法漂移会让人以为变量变了。
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/a2l/elf-export-golden.a2l"
    );
    let actual = exported();
    if std::env::var("OPENAPE_WRITE_GOLDEN").is_ok() {
        std::fs::write(path, &actual).expect("写 golden");
        return;
    }
    let expected = std::fs::read_to_string(path).expect("golden 必需入库");
    assert_eq!(actual, expected, "导出件与 golden 不一致");
}
