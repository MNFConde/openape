//! vcan 上的真实收发。需要 root 与 vcan 模块，缺失即跳过而不失败——
//! 跳过要留下原因，否则 CI 里"全绿"会掩盖没测。

use std::time::Duration;

use openape_bus::{Bus, CanFrame, CanId};
use openape_bus_socketcan::SocketCanBus;
use openape_bus_socketcan::testing::VcanGuard;

#[tokio::test]
async fn vcan_carries_frames_back_to_a_second_socket() {
    let Some(vcan) = VcanGuard::create("a") else {
        return;
    };

    let sender = SocketCanBus::open(vcan.name()).expect("发送端能打开");
    let receiver = SocketCanBus::open(vcan.name()).expect("接收端能打开");

    assert_eq!(sender.channel(), vcan.name());
    assert_eq!(sender.backend(), "socketcan");
    assert!(sender.capabilities().classic_can());

    let frame =
        CanFrame::new(CanId::Standard(0x201), vec![0xff, 0x00, 0x01, 0x02]).expect("4 字节合法");
    sender.send(frame).await.expect("发送成功");

    let received = receiver
        .recv(Duration::from_secs(2))
        .await
        .expect("2 秒内应收到");
    assert_eq!(received.id, CanId::Standard(0x201));
    assert_eq!(received.data, vec![0xff, 0x00, 0x01, 0x02]);
    assert!(received.timestamp.is_some(), "软时间戳必须填上");
}

#[test]
fn vcan_shows_up_in_available_channels() {
    let Some(vcan) = VcanGuard::create("b") else {
        return;
    };
    let channels = openape_bus_socketcan::available_channels();
    assert!(
        channels.iter().any(|c| c == vcan.name()),
        "刚建的 {} 应出现在通道列表里，实际 {:?}",
        vcan.name(),
        channels
    );
}

#[tokio::test]
async fn recv_on_silent_channel_times_out() {
    let Some(vcan) = VcanGuard::create("c") else {
        return;
    };
    let bus = SocketCanBus::open(vcan.name()).expect("能打开");
    let err = bus
        .recv(Duration::from_millis(50))
        .await
        .expect_err("空通道应超时");
    assert!(
        matches!(err, openape_bus::BusError::Timeout { .. }),
        "实际得到: {err:?}"
    );
}
