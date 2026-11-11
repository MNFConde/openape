//! 性质测试（RH-01 第 6 条）：任意文本喂给 CDF 解析器不 panic。
//!
//! CDF 是外部工具导出的 XML，字段缺失、嵌套错位、注释里的怪字符都真实
//! 出现过；解析器该报错的地方报错，但不该崩。

// 集成测试里的 expect 是断言手段：失败即测试失败，不走生产那道门。
#![allow(clippy::expect_used)]

use openape_format_cdf::parse_str;
use proptest::prelude::*;

/// 任意字符（限长 4 KB 左右），含 `<`、`"`、`&` 这些 XML 结构字符。
fn arb_text() -> impl Strategy<Value = String> {
    prop::collection::vec(any::<char>(), 0..=1024).prop_map(|chars| chars.into_iter().collect())
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 64, ..ProptestConfig::default() })]

    #[test]
    fn parse_str_never_panics_on_arbitrary_text(text in arb_text()) {
        let _ = parse_str(&text);
    }

    #[test]
    fn parse_str_never_panics_on_wrapped_text(inner in arb_text()) {
        // 套一层像模像样的外壳：让解析器走过根节点、命名空间与子节点分支，
        // 而不是在第一个字符就返回。
        let text = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
             <MSRSW xmlns=\"http://www.asam.net/standards/XML/CDF/2.0.0\">\
             <SW-SYSTEMS><SW-SYSTEM><SHORT-NAME>s</SHORT-NAME>{inner}\
             </SW-SYSTEM></SW-SYSTEMS></MSRSW>"
        );
        let _ = parse_str(&text);
    }
}
