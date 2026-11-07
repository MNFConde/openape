//! 用入库的样本走一遍"读文件 → 按名对齐"，验收口径来自 M4-06。

use openape_format_cdf::{Category, Scalar, parse_file};

fn sample() -> openape_format_cdf::CdfDocument {
    parse_file("fixtures/minimal.cdf").expect("入库样本必须能读")
}

#[test]
fn reads_every_shape_in_the_sample() {
    let doc = sample();
    let paths: Vec<&str> = doc.entries().iter().map(|e| e.path()).collect();
    assert_eq!(
        paths,
        vec![
            "n_max",
            "tq_offset",
            "kf_curve",
            "map_ign",
            "label_text",
            "ctrl.gain"
        ],
        "顺序要跟文件一致，导入结果才能对着原文件核"
    );

    let map = doc.by_path("map_ign").expect("map 在");
    assert_eq!(map.category(), Some(Category::Map));
    assert_eq!(map.unit(), Some("degCA"));
    assert_eq!(map.dims(), &[2, 2]);
    assert_eq!(map.values().len(), 4);
    assert!(map.shape_matches());

    let text = doc.by_path("label_text").expect("字符串在");
    assert_eq!(text.category(), Some(Category::Ascii));
    assert_eq!(text.unit(), None, "没写单位就是没有，不猜");
    assert_eq!(text.values(), &[Scalar::Text("OpenAPE".to_string())]);

    let member = doc.by_path("ctrl.gain").expect("结构体成员用点路径");
    assert_eq!(member.category(), Some(Category::Value));
    assert_eq!(member.values()[0].as_number(), Some(1.25));
    assert!(
        doc.by_path("ctrl").is_none(),
        "结构体父本身没有取值，不产生条目"
    );
}

#[test]
fn notes_name_the_part_we_did_not_import() {
    let doc = sample();
    let history: Vec<&str> = doc
        .notes()
        .iter()
        .filter(|n| n.subject() == "n_max")
        .map(|n| n.message())
        .collect();
    assert!(
        history.iter().any(|m| m.contains("版次历史")),
        "合法但未解析的部分要给提示：{history:?}"
    );
}

#[test]
fn assign_against_a2l_names_splits_three_ways() {
    let doc = sample();
    let assigned = doc.assign(["n_max", "kf_curve", "ctrl.gain", "not_in_cdf"]);

    assert_eq!(assigned.matched().len(), 3);
    assert_eq!(assigned.name_only(), &["not_in_cdf"]);
    let cdf_only: Vec<&str> = assigned.cdf_only().iter().map(|e| e.path()).collect();
    assert_eq!(cdf_only, vec!["tq_offset", "map_ign", "label_text"]);
    assert!(!assigned.is_complete());
}

#[test]
fn missing_file_gives_actionable_error() {
    let err = parse_file("fixtures/does-not-exist.cdf").expect_err("文件不存在要报错");
    let message = err.to_string();
    assert!(
        message.contains("does-not-exist.cdf"),
        "错误要带路径：{message}"
    );
    assert!(message.contains("确认路径"), "错误要带下一步：{message}");
}
