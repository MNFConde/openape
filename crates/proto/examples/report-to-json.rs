//! 契约验收（D-20）：只用公开 API 走通"构造一份报告 → serde 往返"。
//!
//! 这锁的是 proto 的两条约定：字段保持 snake_case、宽地址走字符串。
//! 跑法：`cargo run -p openape-proto --example report-to-json`

fn main() {
    let report = openape_proto::BusReport {
        channels: vec!["can0".to_owned(), "vcan0".to_owned()],
        probed: None,
    };
    let json = match serde_json::to_string_pretty(&report) {
        Ok(json) => json,
        Err(error) => {
            eprintln!("序列化失败：{error}");
            std::process::exit(1);
        }
    };
    println!("{json}");
    let back: openape_proto::BusReport = match serde_json::from_str(&json) {
        Ok(back) => back,
        Err(error) => {
            eprintln!("反序列化失败：{error}");
            std::process::exit(1);
        }
    };
    assert_eq!(back.channels, report.channels, "往返必须无损");
    println!("往返无损");
}
