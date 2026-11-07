//! 验收（M3-06）：轴绑定 golden 三件套——ELF 夹具、工程配置、期望输出。
//!
//! 三份文件互为约束：配置改了就与 ELF 的维度对不上，ELF 改了就与配置
//! 的地址对不上，导出模板改了就与期望 A2L 对不上。任一处漂移都会在这里
//! 亮红灯，而不是等到真机上导出的 Map 打不开。

use std::path::{Path, PathBuf};

use openape_cal_core::{AxisBinding, ProjectConfig, Role, VariableTree};
use openape_format_a2l::write_a2l;
use openape_format_elf::load;

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures")
        .join(name)
}

/// ELF → IR，并按 golden 配置的用法指定角色：被绑的值与轴都是标定量。
fn tree() -> VariableTree {
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
    VariableTree::new(assigned).expect("符号不重复")
}

fn binding(tree: &VariableTree) -> AxisBinding {
    let text = std::fs::read_to_string(fixture("config/axis-binding-golden.toml"))
        .expect("golden 配置必须入库");
    let config = ProjectConfig::from_toml(&text).expect("golden 配置必须能解析");
    AxisBinding::resolve(tree, config.binding()).expect("golden 配置必须能解析到变量上")
}

#[test]
fn golden_config_resolves_against_the_golden_elf() {
    let tree = tree();
    let binding = binding(&tree);
    assert_eq!(binding.axes().len(), 2);
    assert_eq!(binding.values().len(), 1);
    let value = &binding.values()[0];
    assert_eq!(value.symbol().as_str(), "cal_map");
    assert_eq!(value.dims(), &[4, 3]);
}

#[test]
fn axes_keep_the_order_the_config_declares() {
    // axes 顺序是「内层优先」，与 C 声明一致；写反会让导出的 Map 转置，
    // 而转置在数值上完全说得通，是最难发现的一类错。
    let tree = tree();
    let binding = binding(&tree);
    let ids: Vec<&str> = binding
        .values()
        .iter()
        .flat_map(|value| value.axes().iter())
        .map(|id| id.as_str())
        .collect();
    assert_eq!(ids, vec!["y_temp", "x_speed"]);
}

#[test]
fn export_uses_matrix_dim_in_reverse_and_axis_descr_in_declared_order() {
    let tree = tree();
    let binding = binding(&tree);
    let text = write_a2l(&tree, &binding).expect("要能导出");

    assert!(
        text.contains("MAP"),
        "绑了双轴的二维值应导出成 MAP：\n{text}"
    );
    assert!(
        text.contains("MATRIX_DIM 3 4"),
        "MATRIX_DIM 快速维在前，dims [4,3] 应写成 3 4：\n{text}"
    );

    // AXIS_DESCR 的排列顺序与 axes 相反（x、y、z 递增），这里 x_speed 在前。
    let descr_start = text.find("AXIS_DESCR").expect("必须有轴描述");
    let first = text[descr_start..]
        .find("AXIS_PTS_REF")
        .expect("必须有轴引用");
    let tail = &text[descr_start + first..];
    let x_at = tail.find("x_speed").expect("x 轴要出现");
    let y_at = tail.find("y_temp").expect("y 轴要出现");
    assert!(x_at < y_at, "AXIS_DESCR 顺序应与 axes 声明相反：\n{text}");
}

#[test]
fn golden_export_still_passes_strict_mode() {
    // 绑了轴之后导出件比档二 golden 复杂得多（多了 AXIS_PTS 与
    // AXIS_DESCR），严格模式照样要零告警。
    let tree = tree();
    let binding = binding(&tree);
    let text = write_a2l(&tree, &binding).expect("要能导出");
    let (_file, messages) = a2lfile::load_from_string(&text, None, true).expect("严格模式必须接受");
    assert!(messages.is_empty(), "严格模式零告警，实为 {messages:?}");
}

#[test]
fn binding_serializes_to_the_golden_snapshot() {
    let tree = tree();
    let binding = binding(&tree);
    let actual = serde_json::to_string_pretty(&binding).expect("可序列化");
    let path = fixture("config/axis-binding-golden.ir.json");
    if std::env::var("OPENAPE_WRITE_GOLDEN").is_ok() {
        std::fs::write(&path, format!("{actual}\n")).expect("写 golden");
        return;
    }
    let expected = std::fs::read_to_string(&path).expect("golden 快照必须入库");
    assert_eq!(actual.trim_end(), expected.trim_end(), "IR 快照不一致");
}
