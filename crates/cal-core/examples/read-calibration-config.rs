//! 契约验收（D-20）：只用公开 API 走通"读配置 → 取绑定 → 换算一个值"。
//!
//! 这个流程写不出来即抽象不足——这是 L1 公开面的可用性检验。跑法：
//! `cargo run -p openape-cal-core --example read-calibration-config`

fn main() {
    let config_text = r#"
version = 1

[[binding]]
symbol = "engine_speed"
address = 0x1000
kind = "value"
elem = "u16"
range = { min = 0, max = 6000 }
"#;
    // 1. 配置解析：语法错误当场报，带位置。
    let config = match openape_cal_core::ProjectConfig::from_toml(config_text) {
        Ok(config) => config,
        Err(error) => {
            eprintln!("配置解析失败：{error}");
            std::process::exit(1);
        }
    };
    println!("schema 版本 {}", config.schema_version());

    // 2. 声明取出：轴与值两节，调用方按公开的访问器消费。
    let binding = config.into_binding();
    println!(
        "轴声明 {} 条，值声明 {} 条",
        binding.axes().len(),
        binding.values().len()
    );

    // 3. 数值换算：从一段内存按类型与字节序解物理值——换算链的唯一入口。
    let raw = 0x12C0_u16.to_be_bytes();
    match openape_cal_core::value::decode(
        &raw,
        openape_cal_core::ElemType::U16,
        openape_cal_core::ByteOrder::Big,
    ) {
        Some(physical) => println!("engine_speed = {physical}"),
        None => {
            eprintln!("换算失败：原始字节长度与元素宽度不符");
            std::process::exit(1);
        }
    }
}
