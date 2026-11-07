//! 自造 ASC 样本（fixtures/replay/sample.asc）的解析钉住。
//!
//! 样本是自己写的（无许可问题），充当回归基线：字段顺序、相对时间
//! 归零、Tx/Rx 两种方向、通道列的容忍度。真实日志变体（错误帧、FD 行、
//! 统计行）在单元测试里覆盖，样本文件只锁"工具链读得出来的最短形状"。

use std::path::Path;

use openape_bus::{CanFrame, CanId};
use openape_bus_replay::{ReplayError, asc};

fn fixtures() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/replay")
}

#[test]
fn sample_file_parses_to_the_expected_frames() {
    let text = std::fs::read_to_string(fixtures().join("sample.asc")).expect("样本必须入库且可读");
    let frames = asc::parse(&text).expect("样本必须能解");
    assert_eq!(frames.len(), 3);
    let id = |raw: u16| CanId::standard(raw).expect("合法");
    assert_eq!(
        frames[0].frame,
        CanFrame::new(id(0x100), vec![0x11, 0x22]).expect("帧合法")
    );
    assert_eq!(frames[1].offset_us, 10_000);
    assert_eq!(frames[2].offset_us, 25_000);
    assert_eq!(
        frames[2].frame,
        CanFrame::new(id(0x7FF), vec![0x55]).expect("帧合法")
    );
}

#[test]
fn empty_file_is_zero_frames_not_an_error() {
    assert_eq!(asc::parse("").expect("空日志就是没有帧"), Vec::new());
}

#[test]
fn parse_error_names_the_line_number() {
    let error = asc::parse("0.000000 1  Tx   600   2   11\n").expect_err("数据不足必须报错");
    let ReplayError::Parse { line, .. } = error else {
        panic!("要的是 Parse");
    };
    assert_eq!(line, 1);
}
