//! 性质测试（RH-01 第 6 条）：任意文本不 panic；导出件回读字段一致。
//!
//! 回读那条是这里最值钱的一条：A2L 的写入端与解析端共用一套字段名，
//! 写错一处（维序、地址、轴引用）导出的 Map 在工具里打开才发现，代价最高。

// 集成测试里的 expect 是断言手段：失败即测试失败，不走生产那道门。
#![allow(clippy::expect_used)]

use std::path::{Path, PathBuf};

use openape_cal_core::{AxisBinding, ProjectConfig, Role, VariableTree};
use openape_format_a2l::{load_str, write_a2l};
use openape_format_elf::load;
use proptest::prelude::*;

/// 任意字符（限长 4 KB 左右），含 `/`、`(`、`"` 这些 A2L 结构字符。
fn arb_text() -> impl Strategy<Value = String> {
    prop::collection::vec(any::<char>(), 0..=1024).prop_map(|chars| chars.into_iter().collect())
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 64, ..ProptestConfig::default() })]

    #[test]
    fn load_str_never_panics_on_arbitrary_text(text in arb_text()) {
        let _ = load_str(&text, None);
    }
}

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures")
        .join(name)
}

/// ELF → IR（按 golden 配置指定角色）+ 轴绑定，与 axis_binding_golden 同源。
fn golden_tree_and_binding() -> (VariableTree, AxisBinding) {
    let source = load(&fixture("elf/test.elf")).expect("ELF 必须能读");
    let (tree, _) = source.to_ir().expect("ELF 必须能进 IR");
    let assigned: Vec<_> = tree
        .iter()
        .map(|var| {
            let role = match var.symbol().as_str() {
                "cal_map" | "cal_axis_x" | "cal_axis_y" => Role::Characteristic,
                _ => Role::Measurement,
            };
            var.clone().with_role(role)
        })
        .collect();
    let tree = VariableTree::new(assigned).expect("符号不重复");
    let text = std::fs::read_to_string(fixture("config/axis-binding-golden.toml"))
        .expect("golden 配置必须入库");
    let config = ProjectConfig::from_toml(&text).expect("golden 配置必须能解析");
    let binding = AxisBinding::resolve(&tree, config.binding()).expect("配置要能落到变量上");
    (tree, binding)
}

#[test]
fn export_reads_back_with_the_same_symbols_and_dims() {
    let (tree, binding) = golden_tree_and_binding();
    let first = write_a2l(&tree, &binding).expect("导出件要能写出");
    let back = load_str(&first, None).expect("导出件要能读回");

    // 回读件里声明的对象数要与导出文本里的一致：漏一个就是字段在往返中丢了。
    // AXIS_PTS 也回读成变量，所以三种声明一起数。
    let declared: usize = [
        "/begin MEASUREMENT ",
        "/begin CHARACTERISTIC ",
        "/begin AXIS_PTS ",
    ]
    .iter()
    .map(|marker| first.matches(marker).count())
    .sum();
    assert_eq!(
        back.tree().iter().count(),
        declared,
        "回读件的变量数应等于导出文本声明的对象数"
    );

    // 再导出一次：两次文本必须逐字节相同。这是"字段一致"的强形式——
    // 维度、地址、轴引用任何一处错位都会让第二遍文本漂移。
    let second = write_a2l(back.tree(), back.binding()).expect("回读件要能再写出");
    assert_eq!(second, first, "导出→回读→再导出必须收敛到同一份文本");
}
