//! 验收（M3-07）：A2L 与 ELF 的字段冲突在加载阶段就能看见。
//!
//! 这类错误的代价很高：地址差一个字节，写入就会落到相邻变量上，而总线
//! 不会报错。所以校验要在 `probe source` 阶段跑，不能等到用户按下写入。

// 集成测试里的 expect 是断言手段：失败即测试失败，不走生产那道门。
#![allow(clippy::expect_used)]

use std::path::Path;

use openape_assembly::{Field, cross_check};
use openape_cal_core::VariableTree;
use openape_format_elf::load;

fn fixture(name: &str) -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures")
        .join(name)
}

fn elf_tree() -> VariableTree {
    let source = load(&fixture("elf/test.elf")).expect("ELF 必须能读");
    source.to_ir().expect("ELF 必须能进 IR").0
}

fn a2l_tree(name: &str) -> VariableTree {
    let text = std::fs::read_to_string(fixture(name)).expect("A2L 夹具必须入库");
    openape_format_a2l::load_str(&text, None)
        .expect("A2L 必须能读")
        .tree()
        .clone()
}

#[test]
fn the_same_ecu_in_two_files_has_no_conflicts() {
    // 档二 golden 就是这份 ELF 导出的，两边必须完全对得上。
    let report = cross_check(&elf_tree(), &a2l_tree("a2l/elf-export-golden.a2l"));
    assert!(
        report.is_consistent(),
        "同一来源的两份文件不该有冲突：{:?}",
        report.conflicts()
    );
}

#[test]
fn a_moved_address_is_reported_with_both_values() {
    // 改一个地址字节是真实会发生的事故：A2L 是上一版 ECU 导出的。
    let text = std::fs::read_to_string(fixture("a2l/elf-export-golden.a2l")).expect("夹具");
    let moved = text.replace("ECU_ADDRESS 0x00002020", "ECU_ADDRESS 0x00002024");
    assert_ne!(text, moved, "替换必须真的改到了内容");
    let tampered = openape_format_a2l::load_str(&moved, None).expect("仍可解析");
    let report = cross_check(&elf_tree(), tampered.tree());
    assert!(report.has_conflicts(), "地址不同必须报出来");
    let conflict = report
        .conflicts()
        .iter()
        .find(|conflict| conflict.field() == Field::Address)
        .expect("要有地址冲突");
    assert!(!conflict.elf().is_empty() && !conflict.a2l().is_empty());
    assert_ne!(conflict.elf(), conflict.a2l(), "两边值要能对比");
}

#[test]
fn conflicts_name_the_symbol_so_it_can_be_fixed() {
    let text = std::fs::read_to_string(fixture("a2l/elf-export-golden.a2l")).expect("夹具");
    let moved = text.replace("ECU_ADDRESS 0x00002020", "ECU_ADDRESS 0x00002024");
    let tampered = openape_format_a2l::load_str(&moved, None).expect("仍可解析");
    let report = cross_check(&elf_tree(), tampered.tree());
    let text = report.conflicts()[0].to_string();
    assert!(text.contains("地址"), "报告要点明是哪个字段：{text}");
    assert!(
        text.contains("0x"),
        "报告要带上两边的实际值，不然用户只能去翻文件：{text}"
    );
}

#[test]
fn variables_missing_on_one_side_are_listed_but_not_called_conflicts() {
    // A2L 常比 ELF 少（用户只关心部分变量），也常比 ELF 多（记着已被
    // 优化掉的变量）。当成冲突会让报告全是噪音。
    let partial = r#"
ASAP2_VERSION 1 71
/begin PROJECT P ""
  /begin MODULE ECU ""
    /begin MOD_COMMON ""
      BYTE_ORDER MSB_FIRST
    /end MOD_COMMON
    /begin RECORD_LAYOUT RL
      FNC_VALUES 1 FLOAT32_IEEE ROW_DIR DIRECT
    /end RECORD_LAYOUT
    /begin MEASUREMENT ghost_var "" FLOAT32_IEEE NO_COMPU_METHOD 1 0.0 -1.0E9 1.0E9
      ECU_ADDRESS 0x00009000
    /end MEASUREMENT
  /end MODULE
/end PROJECT
"#;
    let a2l = openape_format_a2l::load_str(partial, None).expect("能解析");
    let report = cross_check(&elf_tree(), a2l.tree());
    assert!(!report.has_conflicts(), "单边缺失不算冲突");
    assert!(!report.only_in_elf().is_empty(), "ELF 多数变量要在清单里");
    assert_eq!(
        report
            .only_in_a2l()
            .iter()
            .map(|symbol| symbol.as_str())
            .collect::<Vec<_>>(),
        vec!["ghost_var"],
        "A2L 记着 ELF 已没有的变量，也算清单"
    );
}

#[test]
fn element_type_mismatch_is_its_own_field() {
    // 宽度不同比地址不同更隐蔽：读回来的数值"看着像"，但小数位全错。
    let text = std::fs::read_to_string(fixture("a2l/elf-export-golden.a2l")).expect("夹具");
    let widened = text.replace(
        "/begin MEASUREMENT cal_folded \"\" FLOAT32_IEEE",
        "/begin MEASUREMENT cal_folded \"\" UWORD",
    );
    assert_ne!(text, widened, "替换必须真的改到了内容");
    let tampered = openape_format_a2l::load_str(&widened, None).expect("仍可解析");
    let report = cross_check(&elf_tree(), tampered.tree());
    assert!(
        report
            .conflicts()
            .iter()
            .any(|conflict| conflict.field() == Field::Elem),
        "元素类型不同必须报出来：{:?}",
        report.conflicts()
    );
}
