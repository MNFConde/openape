//! 端到端：vcan 上跑真从站，CLI 作为独立进程建链与读数。
//!
//! 没有 vcan 时跳过并打印原因——跳过必须留痕，否则"全绿"会掩盖没测。

use std::process::Command;
use std::sync::Arc;

use openape_bus_socketcan::SocketCanBus;
use openape_sim_ecu::{SimEcu, SimEcuConfig};

/// 用完即删的 vcan 通道。
struct Vcan {
    name: String,
}

impl Vcan {
    fn create(tag: &str) -> Option<Self> {
        let name = format!("v{}{}", std::process::id() % 10_000, tag);
        let add = Command::new("ip")
            .args(["link", "add", &name, "type", "vcan"])
            .output()
            .ok()?;
        if !add.status.success() {
            eprintln!(
                "跳过：建 vcan 失败（{}）",
                String::from_utf8_lossy(&add.stderr).trim()
            );
            return None;
        }
        let up = Command::new("ip")
            .args(["link", "set", &name, "up"])
            .output()
            .ok()?;
        if !up.status.success() {
            let _ = Command::new("ip").args(["link", "del", &name]).output();
            eprintln!("跳过：vcan 起不来");
            return None;
        }
        Some(Self { name })
    }
}

impl Drop for Vcan {
    fn drop(&mut self) {
        let _ = Command::new("ip")
            .args(["link", "del", &self.name])
            .output();
    }
}

fn openape() -> &'static str {
    env!("CARGO_BIN_EXE_openape")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn read_returns_the_ecus_memory() {
    let Some(vcan) = Vcan::create("a") else {
        return;
    };

    let bus = Arc::new(SocketCanBus::open(&vcan.name).expect("能打开 vcan"));
    let mut ecu = SimEcu::new(SimEcuConfig::default_ids());
    ecu.poke(0x1000, &[0xDE, 0xAD, 0xBE, 0xEF]);
    let slave = tokio::spawn(openape_sim_ecu::serve(ecu, bus));

    let output = tokio::process::Command::new(openape())
        .args([
            "read",
            "--channel",
            &vcan.name,
            "--address",
            "0x1000",
            "--len",
            "4",
            "--json",
        ])
        .output()
        .await
        .expect("能起 CLI 子进程");

    slave.abort();

    assert!(
        output.status.success(),
        "CLI 失败，stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).expect("JSON 输出");
    assert_eq!(json["data_hex"], "de ad be ef");
    assert_eq!(json["address"], "0x00001000");
    assert_eq!(json["len"], 4);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn probe_xcp_reports_slave_capabilities() {
    let Some(vcan) = Vcan::create("b") else {
        return;
    };

    let bus = Arc::new(SocketCanBus::open(&vcan.name).expect("能打开 vcan"));
    let slave = tokio::spawn(openape_sim_ecu::serve(
        SimEcu::new(SimEcuConfig::default_ids()),
        bus,
    ));

    let output = tokio::process::Command::new(openape())
        .args(["probe", "xcp", "--channel", &vcan.name, "--json"])
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
    assert_eq!(json["max_cto"], 8);
    assert_eq!(json["max_dto"], 8);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn three_runs_of_the_same_read_agree() {
    let Some(vcan) = Vcan::create("c") else {
        return;
    };

    let bus = Arc::new(SocketCanBus::open(&vcan.name).expect("能打开 vcan"));
    let mut ecu = SimEcu::new(SimEcuConfig::default_ids());
    ecu.poke(0x2000, &[0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88]);
    let slave = tokio::spawn(openape_sim_ecu::serve(ecu, bus));

    let mut outputs = Vec::new();
    for _ in 0..3 {
        let output = tokio::process::Command::new(openape())
            .args([
                "read",
                "--channel",
                &vcan.name,
                "--address",
                "0x2000",
                "--len",
                "8",
                "--json",
            ])
            .output()
            .await
            .expect("能起子进程");
        assert!(
            output.status.success(),
            "第 {} 次失败：{}",
            outputs.len() + 1,
            String::from_utf8_lossy(&output.stderr)
        );
        outputs.push(output.stdout);
    }

    slave.abort();

    assert_eq!(outputs[0], outputs[1], "连跑结果必须一致");
    assert_eq!(outputs[1], outputs[2]);
    // 8 字节走 SET_MTA + UPLOAD 两帧，覆盖长读路径。
    let json: serde_json::Value = serde_json::from_slice(&outputs[0]).expect("JSON");
    assert_eq!(json["data_hex"], "11 22 33 44 55 66 77 88");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn probe_bus_lists_and_inspects_channels() {
    let Some(vcan) = Vcan::create("e") else {
        return;
    };

    let output = tokio::process::Command::new(openape())
        .args(["probe", "bus", "--json"])
        .output()
        .await
        .expect("能起子进程");
    assert!(output.status.success());
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).expect("JSON");
    let channels = json["channels"].as_array().expect("channels 是数组");
    assert!(
        channels.iter().any(|c| c.as_str() == Some(&vcan.name)),
        "刚建的 {} 应在列表里，实际 {channels:?}",
        vcan.name
    );

    let output = tokio::process::Command::new(openape())
        .args(["probe", "bus", "--channel", &vcan.name, "--json"])
        .output()
        .await
        .expect("能起子进程");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).expect("JSON");
    assert_eq!(json["probed"]["channel"], vcan.name);
    assert_eq!(json["probed"]["backend"], "socketcan");
    assert_eq!(json["probed"]["classic_can"], true);
    assert_eq!(json["probed"]["can_fd"], false);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn probe_bus_prints_human_readable_without_json() {
    let Some(vcan) = Vcan::create("f") else {
        return;
    };
    let output = tokio::process::Command::new(openape())
        .args(["probe", "bus", "--channel", &vcan.name])
        .output()
        .await
        .expect("能起子进程");
    assert!(output.status.success());
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.contains("通道"), "实际输出: {text}");
    assert!(text.contains(&vcan.name));
    assert!(text.contains("经典 CAN=true"), "能力位要出现在人读输出里");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn missing_channel_exits_with_the_bus_code() {
    let output = tokio::process::Command::new(openape())
        .args(["probe", "bus", "--channel", "nosuchif0"])
        .output()
        .await
        .expect("能起子进程");
    assert_eq!(output.status.code(), Some(10), "总线域退出码");
    assert!(!output.stderr.is_empty(), "失败要有可读说明");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn silent_channel_exits_with_the_xcp_code() {
    let Some(vcan) = Vcan::create("d") else {
        return;
    };

    let output = tokio::process::Command::new(openape())
        .args([
            "read",
            "--channel",
            &vcan.name,
            "--address",
            "0x1000",
            "--len",
            "1",
            "--timeout-ms",
            "50",
        ])
        .output()
        .await
        .expect("能起子进程");

    assert_eq!(output.status.code(), Some(20), "XCP 域退出码");
}
