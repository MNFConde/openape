//! 端到端：单端口同时服务 grpc-web、gRPC 与静态页。
//!
//! 没有 vcan 时样本流那条跳过并留痕——跳过不留痕的"全绿"等于没测。

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use http_body_util::BodyExt;
use openape_bus_socketcan::SocketCanBus;
use openape_bus_socketcan::testing::VcanGuard;
use openape_cal_core::{Address, ByteLen};
use openape_diag::Metrics;
use openape_proto as proto;
use openape_session::{Session, SessionConfig};
use openape_sim_ecu::{SimEcu, SimEcuConfig};
use openape_xcp::{DaqListSpec, DaqSignal};
use tower::ServiceExt;

use openape_daemon::server::build_router;
use openape_daemon::service::AppState;

fn empty_grpc_web_body() -> Vec<u8> {
    // grpc-web 帧：1 字节压缩标志 + 4 字节大端长度，Empty 消息长度为 0
    vec![0, 0, 0, 0, 0]
}

#[tokio::test]
async fn healthz_is_plain_text_and_does_not_touch_hardware() {
    let state = Arc::new(AppState::idle(Arc::new(Metrics::new())));
    let app = build_router(state, Path::new("ui/does-not-exist"));

    let response = app
        .oneshot(
            axum::http::Request::builder()
                .uri("/healthz")
                .body(Body::empty())
                .expect("请求可构造"),
        )
        .await
        .expect("服务可用");
    assert_eq!(response.status(), 200);
    let body = response
        .into_body()
        .collect()
        .await
        .expect("可读体")
        .to_bytes();
    assert_eq!(&body[..], b"ok\n");
}

#[tokio::test]
async fn grpc_web_unary_answers_over_http1() {
    let state = Arc::new(AppState::idle(Arc::new(Metrics::new())));
    let app = build_router(state, Path::new("ui/does-not-exist"));

    let response = app
        .oneshot(
            axum::http::Request::builder()
                .method("POST")
                .uri("/openape.v1.OpenApe/ListChannels")
                .header("content-type", "application/grpc-web+proto")
                .header("x-grpc-web", "1")
                .body(Body::from(empty_grpc_web_body()))
                .expect("请求可构造"),
        )
        .await
        .expect("服务可用");

    assert_eq!(response.status(), 200, "grpc-web 层没有拦下 HTTP/1.1");
    assert_eq!(
        response
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default(),
        "application/grpc-web+proto"
    );
    let body = response
        .into_body()
        .collect()
        .await
        .expect("可读体")
        .to_bytes();
    assert!(body.len() >= 5, "至少要有帧头");
    assert_eq!(body[0], 0, "首帧是未压缩的数据帧");
}

#[tokio::test]
async fn a_missing_static_dir_degrades_with_a_hint() {
    let state = Arc::new(AppState::idle(Arc::new(Metrics::new())));
    let app = build_router(state, Path::new("ui/does-not-exist"));

    let response = app
        .oneshot(
            axum::http::Request::builder()
                .uri("/")
                .body(Body::empty())
                .expect("请求可构造"),
        )
        .await
        .expect("服务可用");
    let body = response
        .into_body()
        .collect()
        .await
        .expect("可读体")
        .to_bytes();
    let text = String::from_utf8_lossy(&body);
    assert!(text.contains("前端未构建"), "要给人话提示，实际：{text}");
}

#[tokio::test]
async fn the_built_ui_is_served_from_the_same_port() {
    let dist = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../ui/dist");
    if !dist.is_dir() {
        eprintln!("跳过：ui/dist 不存在，先跑 script/frontend-check.sh");
        return;
    }

    let state = Arc::new(AppState::idle(Arc::new(Metrics::new())));
    let app = build_router(state, &dist);
    let response = app
        .oneshot(
            axum::http::Request::builder()
                .uri("/")
                .body(Body::empty())
                .expect("请求可构造"),
        )
        .await
        .expect("服务可用");
    assert_eq!(response.status(), 200);
    let body = response
        .into_body()
        .collect()
        .await
        .expect("可读体")
        .to_bytes();
    let html = String::from_utf8_lossy(&body);
    assert!(html.contains("OpenAPE"), "应是 index.html：{html}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn twenty_samples_reach_a_grpc_client() {
    let Some(vcan) = VcanGuard::create("s1") else {
        eprintln!("跳过：本机没有可用的 vcan");
        return;
    };

    let bus = Arc::new(SocketCanBus::open(vcan.name()).expect("能打开 vcan"));
    let mut ecu = SimEcu::new(SimEcuConfig::default_ids());
    ecu.poke(0x1000, &[0x11, 0x22]);
    let slave = tokio::spawn(openape_sim_ecu::serve(ecu, bus));

    let mut session = Session::open(SessionConfig::new(vcan.name())).expect("会话可打开");
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

    let metrics = Arc::new(Metrics::new());
    let state = Arc::new(AppState::with_capture(
        Arc::clone(&metrics),
        vcan.name().to_owned(),
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

    let mut client = proto::open_ape_client::OpenApeClient::connect(format!("http://{address}"))
        .await
        .expect("客户端能连");
    let mut stream = client
        .stream_samples(proto::StreamRequest {})
        .await
        .expect("流能建立")
        .into_inner();

    for index in 0..20 {
        let message = tokio::time::timeout(Duration::from_secs(5), stream.message())
            .await
            .unwrap_or_else(|_| panic!("第 {} 帧没到", index + 1))
            .expect("流不该报错");
        let sample = message.expect("流不该提前结束");
        assert_eq!(sample.pid, 0);
        assert_eq!(sample.payload, vec![0x11, 0x22]);
    }

    server.abort();
    slave.abort();
}

/// 真浏览器回归：headless chrome 加载页面，确认样本流把曲线页喂起来了。
///
/// 标了 `#[ignore]`：门禁不该依赖本机装没装 chrome。手工跑：
/// `cargo test -p openape-daemon -- --ignored headless`
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "需要 google-chrome，手工跑"]
async fn headless_chrome_renders_the_curve() {
    let dist = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../ui/dist");
    assert!(dist.is_dir(), "先跑 script/frontend-check.sh 生成 ui/dist");

    let Some(vcan) = VcanGuard::create("s9") else {
        eprintln!("跳过：本机没有可用的 vcan");
        return;
    };

    let bus = Arc::new(SocketCanBus::open(vcan.name()).expect("能打开 vcan"));
    let mut ecu = SimEcu::new(SimEcuConfig::default_ids());
    ecu.poke(0x1000, &[0x34, 0x12]);
    let slave = tokio::spawn(openape_sim_ecu::serve(ecu, bus));

    let mut session = Session::open(SessionConfig::new(vcan.name())).expect("会话可打开");
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
        vcan.name().to_owned(),
        session,
        capture,
        Some(info),
    ));
    let app = build_router(state, &dist);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("端口可绑");
    let address = listener.local_addr().expect("地址可读");
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.expect("服务能跑");
    });

    let output = std::process::Command::new("google-chrome")
        .args([
            "--headless=new",
            "--no-sandbox",
            "--disable-gpu",
            "--virtual-time-budget=5000",
            "--dump-dom",
            &format!("http://{address}/"),
        ])
        .output()
        .expect("能起 chrome");
    let dom = String::from_utf8_lossy(&output.stdout);
    server.abort();
    slave.abort();

    // headless 的 --dump-dom 在 load 后立刻输出，来不及等流；这里只验页面
    // 起得来、uPlot 已挂上、脚本没抛错。样本帧数由
    // `grpc_web_streaming_over_http1_carries_twenty_frames` 覆盖。
    assert!(dom.contains("u-wrap"), "uPlot 没初始化：{dom}");
    assert!(!dom.contains("出错了"), "页面脚本报错：{dom}");
}
