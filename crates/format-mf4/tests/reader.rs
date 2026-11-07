//! 读端（M5-05）：离线重开的语义层——时间主通道 + f64 列按行还原。
//!
//! 读端走 `mf4-rs` 的解析器，本 crate 只立约定：时间列乘 1e6 还原成微秒
//! 时间戳（round 半分远离零，录制端除以 1e6 的无损逆）、通道按写序列举、
//! 按名取列时点名可用通道。写出→读回逐值无损是本文件的验收线。

mod common;

use std::path::Path;

use common::{SPEED, THROTTLE, TIMES, write_two_channels};
use openape_format_mf4::Reader;

fn fixtures() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/mf4")
}

fn probe() -> std::path::PathBuf {
    // 名字带测试名：同一进程并行跑测试时不共用一个文件，别互相删对方的探针。
    let dir = std::env::temp_dir().join(format!("openape-mf4-reader-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("探针目录必须可建");
    dir.join(format!(
        "{}.mf4",
        std::thread::current()
            .name()
            .expect("测试线程有名")
            .replace("::", "-")
    ))
}

#[test]
fn rows_round_trip_without_loss() {
    let path = probe();
    write_two_channels(&path);
    let reader = Reader::open(&path).expect("自己写的文件必须能读");
    assert_eq!(
        reader.channel_names(),
        ["engine_speed", "throttle"],
        "通道按写序列举，时间主通道不算在内"
    );
    let rows = reader.rows().expect("行必须可读");
    assert_eq!(rows.len(), TIMES.len());
    for (index, row) in rows.iter().enumerate() {
        assert_eq!(
            row.timestamp_us,
            (TIMES[index] * 1e6).round() as u64,
            "时间列乘 1e6 还原微秒，第 {index} 行"
        );
        assert_eq!(row.values, vec![SPEED[index], THROTTLE[index]]);
    }
    let _ = std::fs::remove_file(&path);
}

#[test]
fn golden_file_reads_back_through_the_reader() {
    let reader = Reader::open(&fixtures().join("golden.minimal.mf4")).expect("golden 必须可读");
    let rows = reader.rows().expect("行必须可读");
    assert_eq!(rows.len(), TIMES.len());
    assert_eq!(rows[3].values, vec![SPEED[3], THROTTLE[3]]);
}

#[test]
fn unknown_channel_is_named_with_the_available_ones() {
    let path = probe();
    write_two_channels(&path);
    let reader = Reader::open(&path).expect("文件必须能读");
    let err = reader
        .column("not_a_channel")
        .expect_err("未知通道必须报错而不是给空列");
    assert!(err.to_string().contains("not_a_channel"), "{err}");
    assert!(
        err.to_string().contains("engine_speed"),
        "错误要点名可用通道：{err}"
    );
    let column = reader.column("throttle").expect("已知通道必须可取");
    assert_eq!(column, THROTTLE.to_vec());
    let _ = std::fs::remove_file(&path);
}

#[test]
fn missing_file_is_reported_not_panic() {
    let path = probe();
    let err = Reader::open(&path).expect_err("不存在的文件必须报错");
    assert!(
        err.to_string().contains(
            path.file_name()
                .and_then(|name| name.to_str())
                .expect("文件名是 UTF-8"),
        ),
        "{err}"
    );
}
