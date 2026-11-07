//! 验收（M3-10）：`probe source` 报出变量与交叉校验结果。
//!
//! 仅 ELF 档的每条变量都要标 `[raw]`——缺单位、量程、换算时标出来，
//! 不填默认值假装知道。这是 D-13 那条"不提供语义"在界面上的样子。

use std::path::{Path, PathBuf};

use openape_cli::commands::probe_source;

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures")
        .join(name)
}

#[test]
fn elf_only_marks_every_variable_as_raw() {
    let text = probe_source(Some(&fixture("elf/test.elf")), None, false).expect("要能报");
    assert!(text.contains("[raw]"), "仅 ELF 档每条都要标 raw：\n{text}");
    assert!(
        text.contains("角色=未指定"),
        "角色留给用户，不替用户判定：\n{text}"
    );
}

#[test]
fn two_sources_agree_and_say_so() {
    let text = probe_source(
        Some(&fixture("elf/test.elf")),
        Some(&fixture("a2l/elf-export-golden.a2l")),
        false,
    )
    .expect("要能报");
    assert!(text.contains("交叉校验：0 条冲突"), "{text}");
    assert!(text.contains("仅 ELF 有 0 条"), "{text}");
}

#[test]
fn no_sources_is_a_usage_error_not_an_empty_report() {
    // 用户手上可能有十几份 A2L，猜错一份比报错更难查。
    let err = probe_source(None, None, false).expect_err("不能什么都不给");
    assert!(err.to_string().contains("--elf"), "错误要指出怎么修：{err}");
}

#[test]
fn json_output_carries_the_same_facts() {
    let text = probe_source(Some(&fixture("elf/test.elf")), None, true).expect("要能报");
    let value: serde_json::Value = serde_json::from_str(&text).expect("必须是合法 JSON");
    let variables = value["variables"].as_array().expect("要有变量数组");
    assert!(!variables.is_empty());
    let first = &variables[0];
    assert!(first["symbol"].is_string());
    assert!(first["address"].is_u64());
    assert_eq!(first["raw"], serde_json::Value::Bool(true));
    assert!(
        first.get("role").is_none(),
        "没有角色就不出这个字段，不填空字符串"
    );
}

#[test]
fn skipped_variables_are_listed_so_absence_is_explainable() {
    // 指针在 IR 里没有形状，被跳过。用户看到变量少了要能查到原因。
    let text = probe_source(Some(&fixture("elf/test.elf")), None, false).expect("要能报");
    assert!(
        text.contains("obs_ptr"),
        "被跳过的变量要出现在提示里：\n{text}"
    );
}
