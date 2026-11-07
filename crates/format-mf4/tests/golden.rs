//! 验收（M5-02）：写出结果字节级钉在 golden 上。
//!
//! MDF4 是录制产物，供离线重开与第三方工具（asammdf）读回。字节级比对
//! 的意义：任何写块、对齐、链接的漂移都在这里亮红灯，而不是等离线重开
//! 或第三方工具读出坏数据才发现。起始时间固定为 0，比对才稳定。
//!
//! 重新生成：`OPENAPE_WRITE_GOLDEN=1 cargo test -p openape-format-mf4 --test golden`
//! 是一个显式动作——顺手改期望会让比对失去意义。

use std::path::{Path, PathBuf};

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/mf4")
}

mod common;

use common::write_two_channels;

#[test]
fn golden_bytes_are_stable() {
    let golden = fixtures().join("golden.minimal.mf4");
    if std::env::var("OPENAPE_WRITE_GOLDEN").is_ok() {
        write_two_channels(&golden);
        return;
    }
    let expected = std::fs::read(&golden).expect("golden 必须入库，见模块文档的重生成命令");
    let probe = std::env::temp_dir().join("openape-mf4-golden-check.mf4");
    write_two_channels(&probe);
    let actual = std::fs::read(&probe).expect("探针必须可写");
    assert_eq!(
        actual, expected,
        "写出字节漂移了：先查改动是否有意，有意的按模块文档重生成 golden"
    );
    let _ = std::fs::remove_file(&probe);
}
