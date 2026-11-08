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

#[tokio::test]
async fn fd_channel_carries_a_64_byte_payload() {
    let Some(vcan) = VcanGuard::create_fd("d") else {
        return;
    };

    let sender = SocketCanBus::open(vcan.name()).expect("发送端能打开");
    let receiver = SocketCanBus::open(vcan.name()).expect("接收端能打开");
    let caps = sender.capabilities();
    assert!(
        caps.classic_can() && caps.can_fd(),
        "MTU 72 的链路要报 FD：{caps:?}"
    );

    let payload: Vec<u8> = (0..64).collect();
    sender
        .send(CanFrame::new(CanId::Standard(0x123), payload.clone()).expect("64 字节是 FD 上限"))
        .await
        .expect("FD 帧发送成功");

    let received = receiver
        .recv(Duration::from_secs(2))
        .await
        .expect("2 秒内应收到");
    assert_eq!(received.id, CanId::Standard(0x123));
    assert_eq!(received.data, payload);
    assert!(received.timestamp.is_some(), "软时间戳必须填上");
}

#[tokio::test]
async fn eight_byte_frame_on_fd_channel_keeps_classic_form() {
    let Some(vcan) = VcanGuard::create_fd("g") else {
        return;
    };

    // 探针先开好再发，否则可能漏帧。
    let probe = socketcan::tokio::CanFdSocket::open(vcan.name()).expect("探针能打开");
    let sender = SocketCanBus::open(vcan.name()).expect("发送端能打开");

    // 8 字节是经典帧的上限：经典节点只认经典形态，这一帧不能变成 FD 帧。
    sender
        .send(CanFrame::new(CanId::Standard(0x321), vec![0x11; 8]).expect("8 字节合法"))
        .await
        .expect("发送成功");

    let frame = tokio::time::timeout(Duration::from_secs(2), probe.read_frame())
        .await
        .expect("2 秒内应收到")
        .expect("读帧成功");
    assert!(
        matches!(frame, socketcan::CanAnyFrame::Normal(_)),
        "8 字节帧应保持经典形态，实际 {frame:?}"
    );
}

#[tokio::test]
async fn fd_channel_still_carries_classic_frames() {
    let Some(vcan) = VcanGuard::create_fd("e") else {
        return;
    };

    let sender = SocketCanBus::open(vcan.name()).expect("发送端能打开");
    let receiver = SocketCanBus::open(vcan.name()).expect("接收端能打开");

    // 经典帧在 FD 链路上不回归：同一个 socket 收两种帧形态。
    let frame =
        CanFrame::new(CanId::Extended(0x18DA_F110), vec![0xff, 0x61, 0x02]).expect("3 字节");
    sender.send(frame).await.expect("经典帧发送成功");

    let received = receiver
        .recv(Duration::from_secs(2))
        .await
        .expect("2 秒内应收到");
    assert_eq!(received.id, CanId::Extended(0x18DA_F110));
    assert_eq!(received.data, vec![0xff, 0x61, 0x02]);
}

#[tokio::test]
async fn classic_only_channel_does_not_claim_fd() {
    let Some(vcan) = VcanGuard::create_classic_only("f") else {
        return;
    };
    let bus = SocketCanBus::open(vcan.name()).expect("能打开");
    let caps = bus.capabilities();
    assert!(caps.classic_can(), "经典帧始终可用：{caps:?}");
    assert!(!caps.can_fd(), "MTU 16 的链路不能报 FD：{caps:?}");
}
