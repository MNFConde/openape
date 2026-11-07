//! 验收（M4-07）：CDF 档二 golden 三件套——输入夹具、变量名配置、期望导入结果。
//!
//! 三份文件互为约束：夹具改了而期望没重生成、配置里的名字与夹具对不上、解析
//! 或映射的写法漂移了，都会在这里亮红灯。
//!
//! 为什么要字节级而不是只看"能读"：导入结果决定了"哪个名字拿到哪一列值"。
//! 把 map 的两个维度写反、把结构体成员的路径前缀丢掉、把某条提示吞掉，功能
//! 测试常常仍然是绿的，而到了真机上就是一次错误的标定写入。
//!
//! 重新生成：`OPENAPE_WRITE_GOLDEN=1 cargo test -p openape-format-cdf --test golden`
//! 是一个显式动作——顺手改期望会让比对失去意义。

use std::path::{Path, PathBuf};

use openape_format_cdf::{Category, Scalar, parse_file};
use serde_json::{Value, json};

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/cdf")
}

/// 变量名配置：TOML，带版本字段（与工程配置同一套纪律）。
fn names() -> Vec<String> {
    let path = fixtures().join("golden.names.toml");
    let text = std::fs::read_to_string(&path).expect("golden 配置必须入库");
    let parsed: toml::Value = toml::from_str(&text).expect("golden 配置必须是合法 TOML");
    assert_eq!(
        parsed
            .get("schema_version")
            .and_then(toml::Value::as_integer),
        Some(1),
        "配置版本不认识就别猜字段含义"
    );
    parsed
        .get("names")
        .and_then(toml::Value::as_array)
        .expect("配置要有 names")
        .iter()
        .map(|value| value.as_str().expect("names 里只能放字符串").to_string())
        .collect()
}

/// 导入结果的稳定投影：只留对外承诺的那几项，字段名即契约。
fn projection() -> String {
    let doc = parse_file(fixtures().join("minimal.cdf")).expect("golden 输入必须能读");
    let names = names();
    let assigned = doc.assign(names.iter().map(String::as_str));

    let entries: Vec<Value> = doc
        .entries()
        .iter()
        .map(|entry| {
            json!({
                "path": entry.path(),
                "system": entry.system(),
                "category": entry.category().map(Category::as_str),
                "unit": entry.unit(),
                "dims": entry.dims(),
                "values": entry
                    .values()
                    .iter()
                    .map(|value| match value {
                        Scalar::Number(number) => json!(number),
                        Scalar::Text(text) => json!(text),
                    })
                    .collect::<Vec<_>>(),
                "line": entry.line(),
            })
        })
        .collect();

    let matched: Vec<Value> = assigned
        .matched()
        .iter()
        .map(|(name, entry)| json!([name, entry.path()]))
        .collect();

    let notes: Vec<Value> = doc
        .notes()
        .iter()
        .map(|note| {
            json!({
                "severity": format!("{:?}", note.severity()),
                "subject": note.subject(),
                "message": note.message(),
            })
        })
        .collect();

    let document = json!({
        "schema_version": 1,
        "entries": entries,
        "assignment": {
            "matched": matched,
            "cdf_only": assigned.cdf_only().iter().map(|e| e.path()).collect::<Vec<_>>(),
            "name_only": assigned.name_only(),
        },
        "notes": notes,
    });

    let mut text = serde_json::to_string_pretty(&document).expect("投影必须能序列化");
    text.push('\n');
    text
}

#[test]
fn import_matches_the_golden_file_byte_for_byte() {
    let path = fixtures().join("golden.import.json");
    let actual = projection();
    if std::env::var("OPENAPE_WRITE_GOLDEN").is_ok() {
        std::fs::write(&path, &actual).expect("写 golden");
        return;
    }
    let expected = std::fs::read_to_string(&path).expect("golden 必需入库");
    assert_eq!(actual, expected, "导入结果与 golden 不一致");
}

#[test]
fn golden_config_fills_all_three_buckets() {
    // 配置若全命中或全不命中，字节比对仍然会过，但那份 golden 什么都没证明。
    let doc = parse_file(fixtures().join("minimal.cdf")).expect("golden 输入必须能读");
    let names = names();
    let assigned = doc.assign(names.iter().map(String::as_str));
    assert!(!assigned.matched().is_empty(), "要有对上的");
    assert!(!assigned.cdf_only().is_empty(), "要有只在 CDF 里的");
    assert!(!assigned.name_only().is_empty(), "要有只在我们这边的");
}
