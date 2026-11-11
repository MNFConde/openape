//! 离线重开的 daemon 面（M5-05）：文件 → 点流。
//!
//! 不碰硬件：`AppState::idle` 就够——`open_recording` 只读文件。三条
//! 分支：正常推流、未知通道点名可用、路径不存在报 not_found。

// 集成测试里的 expect 是断言手段：失败即测试失败，不走生产那道门。
#![allow(clippy::expect_used)]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use openape_daemon::service::{AppState, OpenApeService};
use openape_diag::Metrics;
use openape_format_mf4::{ChannelSpec, Writer};
use openape_proto as proto;
use openape_proto::open_ape_server::OpenApe as _;
use tonic::Request;

fn probe() -> PathBuf {
    // 名字带测试名：同一进程并行跑测试时不共用一个文件，别互相删对方的探针。
    let dir = std::env::temp_dir().join(format!("openape-daemon-rec-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("探针目录必须可建");
    dir.join(format!(
        "{}.mf4",
        std::thread::current()
            .name()
            .expect("测试线程有名")
            .replace("::", "-")
    ))
}

fn write_probe(path: &Path) {
    let mut writer =
        Writer::create(path, &[ChannelSpec::new("engine_speed")]).expect("探针必须可建");
    for (seconds, value) in [(0.0, 10.0), (0.1, 20.0), (0.2, 99.0)] {
        writer.write_row(seconds, &[value]).expect("样本行必须可写");
    }
    writer.finish().expect("收尾必须成功");
}

fn idle_service() -> OpenApeService {
    OpenApeService::new(Arc::new(AppState::idle(Arc::new(Metrics::new()))))
}

async fn collect(
    service: &OpenApeService,
    path: &str,
    channel: &str,
) -> Vec<proto::RecordingPoint> {
    let call = service
        .open_recording(Request::new(proto::RecordingRequest {
            path: path.to_owned(),
            channel: channel.to_owned(),
        }))
        .await
        .expect("流必须能建立");
    let mut stream = call.into_inner();
    let mut points = Vec::new();
    while let Some(item) = tokio_stream::StreamExt::next(&mut stream).await {
        points.push(item.expect("流不该中途报错"));
    }
    points
}

#[tokio::test]
async fn a_recording_file_streams_points_for_the_named_channel() {
    let path = probe();
    write_probe(&path);
    let service = idle_service();
    let points = collect(
        &service,
        path.to_str().expect("路径是 UTF-8"),
        "engine_speed",
    )
    .await;
    assert_eq!(points.len(), 3, "三行样本都到");
    assert_eq!(points[1].timestamp_us, 100_000, "时间列乘 1e6 还原微秒");
    assert_eq!(points[2].value, 99.0);
    let _ = std::fs::remove_file(&path);
}

#[tokio::test]
async fn unknown_channel_names_the_available_ones() {
    let path = probe();
    write_probe(&path);
    let service = idle_service();
    let err = service
        .open_recording(Request::new(proto::RecordingRequest {
            path: path.to_str().expect("路径是 UTF-8").to_owned(),
            channel: "not_a_channel".to_owned(),
        }))
        .await
        .expect_err("未知通道必须报错");
    assert!(err.message().contains("engine_speed"), "{err}");
    let _ = std::fs::remove_file(&path);
}

#[tokio::test]
async fn missing_file_is_not_found() {
    let service = idle_service();
    let err = service
        .open_recording(Request::new(proto::RecordingRequest {
            path: "/nonexistent/openape-rec.mf4".to_owned(),
            channel: "engine_speed".to_owned(),
        }))
        .await
        .expect_err("不存在的文件必须报错");
    assert!(err.message().contains("openape-rec.mf4"), "{err}");
}
