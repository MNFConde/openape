//! XCP-on-Ethernet 回环（M5-12）：UDP master ↔ sim-ecu 的 UDP 从站。
//!
//! 放在本 crate 是依赖方向的约束：xcp 是 L2，不能反向依赖本 crate，
//! 而从站状态机在这里。
//!
//! 验收的最小闭环：CONNECT、内存短读、DAQ 配置与 DTO 收取，全程与
//! CAN 走同一套 master 会话逻辑与 DAQ 解码——传输层差异收在
//! `UdpXcpBus` 一个类型里。与 XCPlite（C 库）的互通攒真实对端后补。

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use openape_bus::{Bus, BusError, CanFrame, CanId};
use openape_cal_core::{Address, ByteLen};
use openape_diag::Metrics;
use openape_sim_ecu::{SimEcu, SimEcuConfig};
use openape_xcp::{CanXcpMaster, DaqLayout, DaqListSpec, DaqSignal, XcpConfig, XcpConnection};
use openape_xcp_proto::DaqFrame;

fn signal(address: u32, len: u16) -> DaqSignal {
    DaqSignal {
        address: Address::new(address),
        len: ByteLen::new(len).expect("测试长度非零"),
    }
}

/// 起一台 UDP 从站，返回它的地址。
async fn slave() -> (
    SocketAddr,
    tokio::task::JoinHandle<Result<(), openape_diag::Fault>>,
) {
    let socket = Arc::new(
        tokio::net::UdpSocket::bind("127.0.0.1:0")
            .await
            .expect("从站要能占端口"),
    );
    let addr = socket.local_addr().expect("地址可知");
    let mut ecu = SimEcu::new(SimEcuConfig::default_ids());
    ecu.poke(0x1000, &[0x11, 0x22]);
    let handle = tokio::spawn(openape_sim_ecu::serve_udp(
        ecu,
        socket,
        Arc::new(Metrics::new()),
    ));
    (addr, handle)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn udp_transport_carries_connect_read_and_daq() {
    let (addr, slave) = slave().await;
    let bus = Arc::new(
        openape_xcp::eth::UdpXcpBus::open(addr, CanId::standard(0x601).expect("合法"))
            .await
            .expect("master 要能连上从站地址"),
    );
    let mut master = CanXcpMaster::new(
        bus.clone(),
        XcpConfig::new(
            CanId::standard(0x600).expect("合法"),
            CanId::standard(0x601).expect("合法"),
        ),
    );

    // CONNECT：从站报资源位与包宽。
    let info = master.connect(0).await.expect("UDP 上要能建链");
    assert_eq!(
        info.resource & openape_xcp_proto::resource::DAQ,
        openape_xcp_proto::resource::DAQ,
        "从站声明 DAQ 能力"
    );

    // 内存短读：SHORT_UPLOAD 一条命令拿回 2 字节。
    let data = master
        .read(Address::new(0x1000), ByteLen::TWO)
        .await
        .expect("UDP 上要能读内存");
    assert_eq!(data, vec![0x11, 0x22], "读回要等于 poke 进去的值");

    // DAQ：配置走完拿到布局，随后 UDP 包流里有 DTO。
    let layout: DaqLayout = master
        .configure_daq(&[DaqListSpec::new(0, 1, vec![signal(0x1000, 2)]).expect("配置合法")])
        .await
        .expect("DAQ 配置要能走完");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    let dto = loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        let frame = bus.recv(remaining).await.expect("应收到 UDP 上的 DTO");
        if DaqFrame::parse(&frame.data).is_some() {
            break frame;
        }
    };
    let parsed = DaqFrame::parse(&dto.data).expect("刚验过是 DTO");
    let values: Vec<u8> = layout
        .decode(parsed.odt, parsed.payload)
        .expect("布局认识这个 ODT")
        .concat();
    assert_eq!(values, vec![0x11, 0x22], "DTO 的值要等于内存里的值");
    slave.abort();
}

#[tokio::test]
async fn udp_recv_times_out_instead_of_hanging() {
    let any_addr: SocketAddr = "127.0.0.1:1".parse().expect("合法地址");
    let bus = openape_xcp::eth::UdpXcpBus::open(any_addr, CanId::standard(0x601).expect("合法"))
        .await
        .expect("本端要能占端口");
    let error = bus
        .recv(Duration::from_millis(60))
        .await
        .expect_err("没有从站就该超时");
    assert!(matches!(error, BusError::Timeout { .. }), "{error}");
    // 没有从站时 send 是 UDP 上的静默失败（无连接协议的常态）：发得出去
    // 即可，收到回包才知道对端不在——这条只验证 send 本身不 panic。
    let cro = CanFrame::new(CanId::standard(0x600).expect("合法"), vec![0; 8]).expect("帧合法");
    bus.send(cro).await.expect("UDP send 允许对端缺席");
}
