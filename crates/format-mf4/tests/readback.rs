//! 验收（M5-02）：写出的文件能被 mf4-rs 读回，通道与时间戳与样本一致。
//!
//! 写出端只写不读，读回交给同一个库的另一条路径（api 层走的是解析器，
//! 与写出器共享的只有格式定义）。"自己写的自己读得回"不是充分证明，所以
//! 另有第三方互测：script/verify-mf4.py 用 asammdf 读同一份 golden。

use std::path::PathBuf;

mod common;

use common::{SPEED, THROTTLE, TIMES, write_two_channels};

fn tmp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("openape-mf4-test-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("临时目录必须可建");
    dir
}

#[test]
fn two_channels_read_back_in_order() {
    let dir = tmp_dir("readback");
    let path = dir.join("two-channels.mf4");
    write_two_channels(&path);

    let mdf = mf4_rs::api::mdf::MDF::from_file(path.to_str().expect("路径是 UTF-8"))
        .expect("写出的文件必须能被 mf4-rs 打开");

    let time = mdf
        .channel("time")
        .expect("时间主通道必须存在")
        .values_as_f64()
        .expect("时间列必须可读");
    assert_eq!(time, TIMES, "时间戳必须逐条一致");

    let speed = mdf
        .channel("engine_speed")
        .expect("通道按名可查")
        .values_as_f64()
        .expect("取值列必须可读");
    assert_eq!(speed, SPEED, "样本值必须逐条一致");

    let throttle = mdf.channel("throttle").expect("第二通道必须存在");
    assert_eq!(
        throttle.values_as_f64().expect("取值列必须可读"),
        THROTTLE,
        "第二个通道不能串到第一个通道的数据上"
    );
}

#[test]
fn row_length_is_checked_at_the_boundary() {
    let dir = tmp_dir("mismatch");
    let path = dir.join("mismatch.mf4");
    let mut writer =
        openape_format_mf4::Writer::create(&path, &[openape_format_mf4::ChannelSpec::new("only")])
            .expect("文件必须可建");
    let err = writer
        .write_row(0.0, &[1.0, 2.0])
        .expect_err("值数与通道数不符必须拒绝");
    assert!(
        matches!(
            err,
            openape_format_mf4::Error::ColumnMismatch {
                expected: 1,
                actual: 2
            }
        ),
        "报错要点名两个数：{err}"
    );
}

#[test]
fn writing_after_finish_is_impossible_by_type() {
    let dir = tmp_dir("finish");
    let path = dir.join("finish.mf4");
    let writer =
        openape_format_mf4::Writer::create(&path, &[openape_format_mf4::ChannelSpec::new("x")])
            .expect("文件必须可建");
    writer.finish().expect("收尾必须成功");
    // finish 消费 Writer：再写一行这行代码就编译不过。这行是给读代码的人看的。
    let _ = &path;
}
