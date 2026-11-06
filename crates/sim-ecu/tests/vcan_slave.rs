//! 从站在真总线上的行为。命令语义由 crate 内单测覆盖，这里只证明
//! `serve` 的收发循环、帧过滤与响应编码在 CAN 上成立。

use std::sync::Arc;
use std::time::Duration;

use openape_bus::{Bus, CanFrame, CanId};
use openape_bus_socketcan::SocketCanBus;
use openape_bus_socketcan::testing::VcanGuard;
use openape_sim_ecu::{SimEcu, SimEcuConfig};

fn command_id() -> CanId {
    CanId::standard(0x600).expect("0x600 在 11 位内")
}

fn response_id() -> CanId {
    CanId::standard(0x601).expect("0x601 在 11 位内")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn slave_answers_connect_and_short_upload_on_the_bus() {
    let Some(vcan) = VcanGuard::create("a") else {
        return;
    };

    let bus: Arc<dyn Bus> = Arc::new(SocketCanBus::open(vcan.name()).expect("从站总线"));
    let mut ecu = SimEcu::new(SimEcuConfig::default_ids());
    ecu.poke(0x1000, &[0xDE, 0xAD, 0xBE, 0xEF]);
    let slave = tokio::spawn(openape_sim_ecu::serve(ecu, bus));

    let client = SocketCanBus::open(vcan.name()).expect("客户端总线");

    // CONNECT：整帧 PID + 命令码 + 6 字节参数。
    let connect =
        CanFrame::new(command_id(), vec![0xFF, 0xFF, 0x00, 0, 0, 0, 0, 0]).expect("8 字节");
    client.send(connect).await.expect("发 CONNECT");
    let reply = client
        .recv(Duration::from_secs(2))
        .await
        .expect("应收到响应");
    assert_eq!(reply.id, response_id());
    assert_eq!(
        reply.data,
        vec![0xFF, 0xFF, 0x05, 0x00, 0x08, 0x00, 0x08, 0x00],
        "响应回显命令码，参数为 resource/maxCto/maxDto"
    );

    // SHORT_UPLOAD：地址落在整帧 offset 4。
    let upload = CanFrame::new(
        command_id(),
        vec![0xFF, 0xF4, 0x04, 0x00, 0x00, 0x10, 0x00, 0x00],
    )
    .expect("8 字节");
    client.send(upload).await.expect("发 SHORT_UPLOAD");
    let reply = client
        .recv(Duration::from_secs(2))
        .await
        .expect("应收到数据");
    assert_eq!(reply.data, vec![0xFF, 0xF4, 0xDE, 0xAD, 0xBE, 0xEF]);

    slave.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn slave_ignores_frames_that_are_not_commands() {
    let Some(vcan) = VcanGuard::create("b") else {
        return;
    };

    let bus: Arc<dyn Bus> = Arc::new(SocketCanBus::open(vcan.name()).expect("从站总线"));
    let slave = tokio::spawn(openape_sim_ecu::serve(
        SimEcu::new(SimEcuConfig::default_ids()),
        bus,
    ));
    let client = SocketCanBus::open(vcan.name()).expect("客户端总线");

    // 别人的标识符 + 命令标识符上的 DAQ DTO（PID 0x00），两种都不该应答。
    let foreign =
        CanFrame::new(CanId::standard(0x123).expect("合法"), vec![0xFF, 0xFF]).expect("2 字节");
    client.send(foreign).await.expect("发外来帧");
    let dto = CanFrame::new(command_id(), vec![0x00, 0x11, 0x22, 0, 0, 0, 0, 0]).expect("8 字节");
    client.send(dto).await.expect("发 DTO");

    let silence = client.recv(Duration::from_millis(300)).await;
    assert!(
        matches!(silence, Err(openape_bus::BusError::Timeout { .. })),
        "非命令帧不该得到响应，实际: {silence:?}"
    );

    slave.abort();
}
