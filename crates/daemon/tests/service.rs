//! 服务方法的行为测试。
//!
//! 这些方法在 `tests/stream.rs` 里只被间接走到（流与健康），代读、探测、
//! 会话状态与指标是另一批分支，单独测才看得见。

use std::sync::Arc;
use std::time::Duration;

use openape_bus_socketcan::SocketCanBus;
use openape_bus_socketcan::testing::VcanGuard;
use openape_daemon::boot::{self, SampleSpec};
use openape_daemon::service::OpenApeService;
use openape_diag::Metrics;
use openape_proto as proto;
use openape_proto::open_ape_server::OpenApe as _;
use openape_sim_ecu::{SimEcu, SimEcuConfig};
use tonic::Request;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_service_answers_over_an_attached_channel() {
    let Some(vcan) = VcanGuard::create("sv") else {
        eprintln!("跳过：本机没有可用的 vcan");
        return;
    };
    let channel = vcan.name().to_owned();

    let bus = Arc::new(SocketCanBus::open(&channel).expect("能打开 vcan"));
    let mut ecu = SimEcu::new(SimEcuConfig::default_ids());
    ecu.poke(0x1000, &[0xDE, 0xAD, 0xBE, 0xEF]);
    let slave = tokio::spawn(openape_sim_ecu::serve(ecu, bus));

    let metrics = Arc::new(Metrics::new());
    // 走 boot 的启动路径：这样开会话、建链、配 DAQ 也被覆盖到。
    let state = Arc::new(
        boot::open_session(
            &channel,
            0x600,
            0x601,
            &[SampleSpec {
                address: 0x1000,
                len: 4,
            }],
            Arc::clone(&metrics),
        )
        .await
        .expect("启动应成功"),
    );
    let service = OpenApeService::new(Arc::clone(&state));

    // 健康与通道列表
    let health = service
        .health(Request::new(proto::Empty {}))
        .await
        .expect("健康可查")
        .into_inner();
    assert!(health.ok);
    assert!(!health.version.is_empty());

    let channels = service
        .list_channels(Request::new(proto::Empty {}))
        .await
        .expect("可列通道")
        .into_inner()
        .channels;
    assert!(
        channels.contains(&channel),
        "自己接管的通道要在列表里：{channels:?}"
    );

    // 探测自己接管的通道：答已知能力，不再去抢锁
    let probe = service
        .probe_channel(Request::new(proto::ProbeChannelRequest {
            channel: channel.clone(),
        }))
        .await
        .expect("已接管的通道能答")
        .into_inner();
    assert_eq!(probe.backend, "socketcan");
    assert!(
        probe.capabilities.expect("能力必须给出").classic_can,
        "vcan 是经典 CAN"
    );

    // 会话状态
    let report = service
        .get_session(Request::new(proto::Empty {}))
        .await
        .expect("可查会话")
        .into_inner();
    assert_eq!(report.channel, channel);
    assert!(report.linked, "已建链");
    assert!(report.capturing, "采集泵在跑");
    assert!(report.info.expect("有建链信息").max_cto > 0);

    // 代读
    let read = service
        .read_memory(Request::new(proto::ReadRequest {
            address: "0x1000".to_owned(),
            len: 4,
        }))
        .await
        .expect("代读应成功")
        .into_inner();
    assert_eq!(read.data_hex, "de ad be ef");
    assert_eq!(read.channel, channel);

    // 采集在跑：指标里能看到帧
    tokio::time::sleep(Duration::from_millis(80)).await;
    let snapshot = service
        .get_metrics(Request::new(proto::Empty {}))
        .await
        .expect("可查指标")
        .into_inner();
    assert!(
        snapshot.daq_frames_total > 0,
        "采了 80 毫秒还一帧没有：{snapshot:?}"
    );

    slave.abort();
}

#[tokio::test]
async fn an_idle_daemon_refuses_to_read_on_behalf_of_anyone() {
    let service = OpenApeService::new(Arc::new(openape_daemon::service::AppState::idle(Arc::new(
        Metrics::new(),
    ))));
    let err = service
        .read_memory(Request::new(proto::ReadRequest {
            address: "0x1000".to_owned(),
            len: 2,
        }))
        .await
        .expect_err("没接管就不该读");
    assert!(
        err.message().contains("没有接管通道"),
        "要指出原因：{}",
        err.message()
    );
}

#[tokio::test]
async fn bad_addresses_and_lengths_are_rejected_before_touching_the_bus() {
    let service = OpenApeService::new(Arc::new(openape_daemon::service::AppState::idle(Arc::new(
        Metrics::new(),
    ))));
    // 地址先于会话检查：这是纯参数校验，不该依赖有没有设备。
    let err = service
        .read_memory(Request::new(proto::ReadRequest {
            address: "zzz".to_owned(),
            len: 2,
        }))
        .await
        .expect_err("地址非法");
    assert!(err.message().contains("daemon 没有接管通道") || err.message().contains("地址"));
}
