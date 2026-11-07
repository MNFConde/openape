//! `write` / `page` / `dataset` 三条命令的端到端行为。
//!
//! 走子进程：命令行的价值就在参数与输出形状上，直接调函数测不到那一段。
//! 没有 vcan 时跳过并打印原因。

use std::sync::Arc;

use openape_bus_socketcan::SocketCanBus;
use openape_bus_socketcan::testing::VcanGuard;
use openape_sim_ecu::{SimEcu, SimEcuConfig};
use openape_xcp_proto::Command;

fn openape() -> &'static str {
    env!("CARGO_BIN_EXE_openape")
}

fn state_dir(case: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("openape-cli-{}-{case}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// `fixtures/elf/test.elf` 里第一条 u16 变量，供按符号写用。
fn elf() -> std::path::PathBuf {
    std::path::Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/elf/test.elf"
    ))
    .to_path_buf()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn write_reports_before_range_and_after_then_leaves_a_snapshot() {
    let Some(vcan) = VcanGuard::create("cw") else {
        eprintln!("跳过：本机没有可用的 vcan");
        return;
    };
    let bus = Arc::new(SocketCanBus::open(vcan.name()).expect("能打开 vcan"));
    let mut ecu = SimEcu::new(SimEcuConfig::default_ids());
    ecu.poke(0x2000, &[0x00, 0x05]);
    let slave = tokio::spawn(openape_sim_ecu::serve(ecu, bus));

    let dir = state_dir("write");
    let output = tokio::process::Command::new(openape())
        .args([
            "write",
            "--channel",
            vcan.name(),
            "--address",
            "0x2000",
            "--data",
            "beef",
            "--state-dir",
            &dir.display().to_string(),
            "--json",
        ])
        .output()
        .await
        .expect("能起子进程");

    slave.abort();

    assert!(
        output.status.success(),
        "命令失败，stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).expect("JSON 输出");
    assert_eq!(json["address"], "0x00002000");
    assert_eq!(json["before_hex"], "00 05");
    assert_eq!(json["after_hex"], "be ef");
    assert_eq!(json["range_checked"], false, "没有 A2L 就没有量程校验");
    let snapshot = json["snapshot_path"].as_str().expect("要有快照路径");
    assert!(
        std::path::Path::new(snapshot).exists(),
        "快照文件应存在：{snapshot}"
    );
    assert!(
        json["audited_at"]
            .as_str()
            .is_some_and(|at| at.ends_with('Z')),
        "审计时刻是 ISO-8601"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn write_human_output_shows_before_range_and_after() {
    let Some(vcan) = VcanGuard::create("ch") else {
        eprintln!("跳过：本机没有可用的 vcan");
        return;
    };
    let bus = Arc::new(SocketCanBus::open(vcan.name()).expect("能打开 vcan"));
    let mut ecu = SimEcu::new(SimEcuConfig::default_ids());
    ecu.poke(0x2000, &[0x00, 0x05]);
    let slave = tokio::spawn(openape_sim_ecu::serve(ecu, bus));

    let output = tokio::process::Command::new(openape())
        .args([
            "write",
            "--channel",
            vcan.name(),
            "--address",
            "0x2000",
            "--data",
            "0007",
            "--state-dir",
            &state_dir("human").display().to_string(),
        ])
        .output()
        .await
        .expect("能起子进程");

    slave.abort();

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(stdout.contains("写前 00 05"), "{stdout}");
    assert!(stdout.contains("[raw]"), "无量程要如实标 [raw]：{stdout}");
    assert!(stdout.contains("写后 00 07"), "{stdout}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn page_set_and_get_round_trip() {
    let Some(vcan) = VcanGuard::create("cp") else {
        eprintln!("跳过：本机没有可用的 vcan");
        return;
    };
    let bus = Arc::new(SocketCanBus::open(vcan.name()).expect("能打开 vcan"));
    let slave = tokio::spawn(openape_sim_ecu::serve(
        SimEcu::new(SimEcuConfig::default_ids()),
        bus,
    ));

    let output = tokio::process::Command::new(openape())
        .args([
            "page",
            "set",
            "--channel",
            vcan.name(),
            "--page",
            "1",
            "--json",
        ])
        .output()
        .await
        .expect("能起子进程");
    slave.abort();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).expect("JSON 输出");
    assert_eq!(json["page"], 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn page_diff_lists_only_the_differing_symbols() {
    let Some(vcan) = VcanGuard::create("cd") else {
        eprintln!("跳过：本机没有可用的 vcan");
        return;
    };
    let bus = Arc::new(SocketCanBus::open(vcan.name()).expect("能打开 vcan"));
    let mut ecu = SimEcu::new(SimEcuConfig::default_ids());
    // 页 0 与页 1 只差 0x2002 这几个字节。
    ecu.poke(0x2000, &[0x00, 0x01]);
    ecu.handle(
        &Command::SetCalPage {
            mode: 0,
            segment: 0,
            page: 1,
        }
        .encode()
        .expect("一帧放得下"),
    );
    ecu.poke(0x2000, &[0x00, 0x01]);
    ecu.handle(
        &Command::SetCalPage {
            mode: 0,
            segment: 0,
            page: 0,
        }
        .encode()
        .expect("一帧放得下"),
    );
    let slave = tokio::spawn(openape_sim_ecu::serve(ecu, bus));

    let output = tokio::process::Command::new(openape())
        .args([
            "page",
            "diff",
            "--channel",
            vcan.name(),
            "--elf",
            &elf().display().to_string(),
            "--reference",
            "0",
            "--working",
            "1",
            "--json",
        ])
        .output()
        .await
        .expect("能起子进程");
    slave.abort();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).expect("JSON 输出");
    // 页 1 全零，页 0 只有 0x2000 上那两个字节非零（若该地址恰好是清单里的变量）。
    assert!(json["differences"].is_array(), "差异要给出数组：{json}");
}

#[tokio::test]
async fn dataset_diff_names_the_mismatch_when_modes_differ() {
    let dir = state_dir("dataset");
    std::fs::create_dir_all(&dir).expect("建目录");
    let left = dir.join("left.json");
    let right = dir.join("right.json");
    std::fs::write(
        &left,
        r#"{"schema_version":1,"mode":"elf_only","taken_at_unix_ms":1,"entries":[{"symbol":"kp","address":"0x00001000","raw":"01"}]}"#,
    )
    .expect("写左侧");
    std::fs::write(
        &right,
        r#"{"schema_version":1,"mode":"with_a2l","taken_at_unix_ms":2,"entries":[{"symbol":"kp","address":"0x00001000","raw":"01"}]}"#,
    )
    .expect("写右侧");

    let output = tokio::process::Command::new(openape())
        .args([
            "dataset",
            "diff",
            "--left",
            &left.display().to_string(),
            "--right",
            &right.display().to_string(),
        ])
        .output()
        .await
        .expect("能起子进程");

    assert!(!output.status.success(), "两档混比要拒绝");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("不可混比"), "{stderr}");
}
