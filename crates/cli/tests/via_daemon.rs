//! `--via-daemon`：设备锁在 daemon 手里时，CLI 仍然能读，而且不去抢锁。
//!
//! 这一条同时守住两件事：daemon 独占时直连必须撞锁（真独占），经 daemon
//! 的代读必须成功（复用的是同一条会话）。

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use openape_bus_socketcan::SocketCanBus;
use openape_bus_socketcan::testing::VcanGuard;
use openape_cal_core::{Address, ByteLen};
use openape_cli::commands;
use openape_daemon::server::build_router;
use openape_daemon::service::AppState;
use openape_diag::Metrics;
use openape_session::{Session, SessionConfig};
use openape_sim_ecu::{SimEcu, SimEcuConfig};
use openape_xcp::{DaqListSpec, DaqSignal};

fn openape() -> &'static str {
    env!("CARGO_BIN_EXE_openape")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cli_reads_through_the_daemon_without_stealing_the_lock() {
    let Some(vcan) = VcanGuard::create("dv") else {
        eprintln!("跳过：本机没有可用的 vcan");
        return;
    };
    let channel = vcan.name().to_owned();

    let bus = Arc::new(SocketCanBus::open(&channel).expect("能打开 vcan"));
    let mut ecu = SimEcu::new(SimEcuConfig::default_ids());
    ecu.poke(0x1000, &[0xDE, 0xAD, 0xBE, 0xEF]);
    let slave = tokio::spawn(openape_sim_ecu::serve(ecu, bus));

    // daemon 侧：建链、配一路采集、拿住设备锁。
    let mut session = Session::open(SessionConfig::new(&channel)).expect("会话可打开");
    let info = session.connect(0).await.expect("建链成功");
    let list = DaqListSpec::new(
        0,
        1,
        vec![DaqSignal {
            address: Address::new(0x1000),
            len: ByteLen::new(2).expect("长度非零"),
        }],
    )
    .expect("配置合法");
    let layout = session.configure_daq(&[list]).await.expect("配置应成功");
    let capture = session.start_capture(layout);

    let state = Arc::new(AppState::with_capture(
        Arc::new(Metrics::new()),
        channel.clone(),
        session,
        capture,
        Some(info),
    ));
    let app = build_router(state, Path::new("ui/does-not-exist"));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("端口可绑");
    let address = listener.local_addr().expect("地址可读");
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.expect("服务能跑");
    });

    // 直连必须撞锁：daemon 是真的独占，而不是"看起来独占"。
    let stolen = Session::open(SessionConfig::new(&channel));
    assert!(
        stolen.is_err(),
        "daemon 持锁期间直连应当失败，实际却拿到了会话"
    );

    let output = tokio::process::Command::new(openape())
        .args([
            "read",
            &format!("--via-daemon=http://{address}"),
            "--channel",
            &channel,
            "--address",
            "0x1000",
            "--len",
            "4",
            "--json",
        ])
        .output()
        .await
        .expect("能起 CLI 子进程");

    server.abort();
    slave.abort();

    assert!(
        output.status.success(),
        "CLI 失败，stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).expect("JSON 输出");
    assert_eq!(json["data_hex"], "de ad be ef");
    assert_eq!(json["address"], "0x00001000");
    assert_eq!(json["channel"], channel);
}

/// 同一批 gRPC 入口也被库内测试直接调用：子进程跑一遍看不到覆盖率。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_via_helpers_work_against_a_live_daemon() {
    let Some(vcan) = VcanGuard::create("dw") else {
        eprintln!("跳过：本机没有可用的 vcan");
        return;
    };
    let channel = vcan.name().to_owned();

    let bus = Arc::new(SocketCanBus::open(&channel).expect("能打开 vcan"));
    let mut ecu = SimEcu::new(SimEcuConfig::default_ids());
    ecu.poke(0x1000, &[0xDE, 0xAD, 0xBE, 0xEF]);
    let slave = tokio::spawn(openape_sim_ecu::serve(ecu, bus));

    let mut session = Session::open(SessionConfig::new(&channel)).expect("会话可打开");
    let info = session.connect(0).await.expect("建链成功");
    let list = DaqListSpec::new(
        0,
        1,
        vec![DaqSignal {
            address: Address::new(0x1000),
            len: ByteLen::new(2).expect("长度非零"),
        }],
    )
    .expect("配置合法");
    let layout = session.configure_daq(&[list]).await.expect("配置应成功");
    let capture = session.start_capture(layout);

    let state = Arc::new(AppState::with_capture(
        Arc::new(Metrics::new()),
        channel.clone(),
        session,
        capture,
        Some(info),
    ));
    let app = build_router(state, Path::new("ui/does-not-exist"));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("端口可绑");
    let address = listener.local_addr().expect("地址可读");
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.expect("服务能跑");
    });
    let url = format!("http://{address}");

    let channels_json = commands::probe_bus_via(&url, Some(&channel), true)
        .await
        .expect("经 daemon 探测通道");
    let report: serde_json::Value = serde_json::from_str(&channels_json).expect("JSON");
    assert_eq!(report["probed"]["channel"], channel);
    assert_eq!(report["probed"]["capabilities"]["classic_can"], true);

    let connect_json = commands::probe_xcp_via(&url, &channel, true)
        .await
        .expect("经 daemon 看会话");
    let report: serde_json::Value = serde_json::from_str(&connect_json).expect("JSON");
    assert_eq!(report["channel"], channel);
    assert!(report["info"]["max_cto"].as_u64().unwrap_or(0) > 0);

    let mismatch = commands::probe_xcp_via(&url, "vcan_other", false).await;
    assert!(mismatch.is_err(), "通道对不上要报错");

    let read_json = commands::read_via(&url, 0x1000, 4, true)
        .await
        .expect("经 daemon 读内存");
    let report: serde_json::Value = serde_json::from_str(&read_json).expect("JSON");
    assert_eq!(report["data_hex"], "de ad be ef");

    assert!(
        commands::read_via(&url, 0x1000, 0, false).await.is_err(),
        "长度 0 要在本地就被挡下"
    );

    server.abort();
    slave.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_daemon_that_owns_nothing_says_so() {
    // daemon 起得来但没接管通道：CLI 应得到可操作的提示，而不是空结果。
    let state = Arc::new(AppState::idle(Arc::new(Metrics::new())));
    let app = build_router(state, Path::new("ui/does-not-exist"));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("端口可绑");
    let address = listener.local_addr().expect("地址可读");
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.expect("服务能跑");
    });
    // 给它一点启动时间，避免连接竞态。
    tokio::time::sleep(Duration::from_millis(50)).await;

    let output = tokio::process::Command::new(openape())
        .args([
            "read",
            &format!("--via-daemon=http://{address}"),
            "--channel",
            "vcan999",
            "--address",
            "0x1000",
            "--len",
            "2",
        ])
        .output()
        .await
        .expect("能起 CLI 子进程");

    server.abort();

    assert!(!output.status.success(), "没有会话就不该成功");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("daemon 没有接管通道"),
        "要指出原因，实际：{stderr}"
    );
}
