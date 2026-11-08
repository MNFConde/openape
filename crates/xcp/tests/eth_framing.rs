//! UDP 传输的拆包行为：第三方从站会把若干条 XCP 消息拼进一条包。

use std::time::Duration;

use openape_bus::{Bus, CanFrame, CanId};
use openape_xcp::eth::{HeaderMode, UdpXcpBus};

#[tokio::test]
async fn one_packet_yields_one_frame_per_message() {
    let server = tokio::net::UdpSocket::bind("127.0.0.1:0")
        .await
        .expect("假从站能绑");
    let peer = server.local_addr().expect("地址可读");
    let bus = UdpXcpBus::open(
        peer,
        CanId::standard(0x601).expect("合法"),
        HeaderMode::Headered,
    )
    .await
    .expect("能连上假从站");

    // 先发一帧，让假从站学到 master 的端口。
    bus.send(
        CanFrame::new(CanId::standard(0x600).expect("合法"), vec![0xFF, 0x00]).expect("2 字节"),
    )
    .await
    .expect("发得出");
    let mut buffer = [0u8; 128];
    let (_, master) = server.recv_from(&mut buffer).await.expect("收得到");

    // 一条包里装两条消息：各自带长度头与计数器。
    let mut packet = HeaderMode::Headered.wrap(&[0x00, 0xAA, 0x00, 0x00, 0x11], 1);
    packet.extend_from_slice(&HeaderMode::Headered.wrap(&[0x01, 0xAA, 0x00, 0x00, 0x22], 2));
    server.send_to(&packet, master).await.expect("发得出");

    let first = bus.recv(Duration::from_secs(1)).await.expect("第一帧");
    let second = bus.recv(Duration::from_secs(1)).await.expect("第二帧");
    assert_eq!(first.data, vec![0x00, 0xAA, 0x00, 0x00, 0x11]);
    assert_eq!(second.data, vec![0x01, 0xAA, 0x00, 0x00, 0x22]);
    assert_eq!(first.id, second.id, "两条消息都按响应标识符呈现");
}

#[tokio::test]
async fn headerless_packet_is_one_message() {
    let server = tokio::net::UdpSocket::bind("127.0.0.1:0")
        .await
        .expect("假从站能绑");
    let peer = server.local_addr().expect("地址可读");
    let bus = UdpXcpBus::open(
        peer,
        CanId::standard(0x601).expect("合法"),
        HeaderMode::Headerless,
    )
    .await
    .expect("能连上假从站");

    bus.send(
        CanFrame::new(CanId::standard(0x600).expect("合法"), vec![0xFF, 0x00]).expect("2 字节"),
    )
    .await
    .expect("发得出");
    let mut buffer = [0u8; 128];
    let (_, master) = server.recv_from(&mut buffer).await.expect("收得到");
    server
        .send_to(&[0xFF, 0x00, 0x01], master)
        .await
        .expect("发得出");

    let frame = bus.recv(Duration::from_secs(1)).await.expect("一帧");
    assert_eq!(frame.data, vec![0xFF, 0x00, 0x01]);
}
