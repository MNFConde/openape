//! 浏览器的路径：HTTP/1.1 + grpc-web 帧，服务端流式。
//!
//! 这里不用 gRPC 客户端、也不用浏览器自动化，而是手写 HTTP/1.1 请求并解析
//! chunked 响应——浏览器发出去的就是这个东西。有了它，"浏览器能不能收到流"
//! 就不必靠人眼看。

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use openape_bus_socketcan::SocketCanBus;
use openape_bus_socketcan::testing::VcanGuard;
use openape_cal_core::{Address, ByteLen};
use openape_diag::Metrics;
use openape_session::{Session, SessionConfig};
use openape_sim_ecu::{SimEcu, SimEcuConfig};
use openape_xcp::{DaqListSpec, DaqSignal};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use openape_daemon::server::build_router;
use openape_daemon::service::AppState;

/// 从 chunked 编码里解出原始字节。返回已收到的字节流。
async fn read_chunked(stream: &mut tokio::net::TcpStream, budget: Duration) -> Vec<u8> {
    let mut raw = Vec::new();
    let mut buffer = [0u8; 4096];
    let deadline = tokio::time::Instant::now() + budget;

    loop {
        let read = tokio::time::timeout_at(deadline, stream.read(&mut buffer)).await;
        match read {
            Ok(Ok(0)) => break,
            Ok(Ok(count)) => raw.extend_from_slice(&buffer[..count]),
            _ => break,
        }
        // 帧数够了就不必等连接收尾
        if count_grpc_web_frames(&decode_chunked(&raw)) >= 20 {
            break;
        }
    }
    decode_chunked(&raw)
}

/// 解 chunked。抬头没结束或块不完整时，返回已解的完整块。
fn decode_chunked(raw: &[u8]) -> Vec<u8> {
    let Some(headers_end) = raw.windows(4).position(|window| window == b"\r\n\r\n") else {
        return Vec::new();
    };
    let mut body = &raw[headers_end + 4..];
    let mut out = Vec::new();
    while let Some(line_end) = body.windows(2).position(|window| window == b"\r\n") {
        let Ok(size_text) = std::str::from_utf8(&body[..line_end]) else {
            break;
        };
        let size_text = size_text.split(';').next().unwrap_or_default().trim();
        let Ok(size) = usize::from_str_radix(size_text, 16) else {
            break;
        };
        if size == 0 {
            break;
        }
        let start = line_end + 2;
        let Some(chunk) = body.get(start..start + size) else {
            break;
        };
        out.extend_from_slice(chunk);
        body = &body[(start + size + 2).min(body.len())..];
    }
    out
}

/// 数 grpc-web 帧：跳过 1 字节标志与 4 字节长度。
fn count_grpc_web_frames(body: &[u8]) -> usize {
    let mut offset = 0;
    let mut frames = 0;
    while offset + 5 <= body.len() {
        let len = u32::from_be_bytes([
            body[offset + 1],
            body[offset + 2],
            body[offset + 3],
            body[offset + 4],
        ]) as usize;
        if offset + 5 + len > body.len() {
            break;
        }
        frames += 1;
        offset += 5 + len;
    }
    frames
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn grpc_web_streaming_over_http1_carries_twenty_frames() {
    let Some(vcan) = VcanGuard::create("w1") else {
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

    let state = Arc::new(AppState::with_capture(
        Arc::new(Metrics::new()),
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

    let mut client = tokio::net::TcpStream::connect(address)
        .await
        .expect("能连上");
    let request = format!(
        "POST /openape.v1.OpenApe/StreamSamples HTTP/1.1\r\n\
         Host: {address}\r\n\
         content-type: application/grpc-web+proto\r\n\
         x-grpc-web: 1\r\n\
         content-length: 5\r\n\
         connection: close\r\n\r\n"
    );
    client
        .write_all(request.as_bytes())
        .await
        .expect("能写请求头");
    client.write_all(&[0, 0, 0, 0, 0]).await.expect("能写帧");

    let body = read_chunked(&mut client, Duration::from_secs(5)).await;
    server.abort();
    slave.abort();

    let frames = count_grpc_web_frames(&body);
    assert!(
        frames >= 20,
        "浏览器路径应收到至少 20 帧，实际 {frames} 帧，字节数 {}",
        body.len()
    );
}
