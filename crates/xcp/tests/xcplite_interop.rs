//! 与 XCPlite（vectorgrp 实现，MIT）的互通验收（M5-12）。
//!
//! 这条测试依赖外部构建链与参考从站：`tmp/XCPlite` 的 `hello_xcp` 跑在
//! UDP 5555（CTR 头模式）。平时跳过，由 `script/xcplite-interop.sh` 起
//! 从站、算好地址后以 `--ignored` 调起。
//!
//! 已验收（2026-11-08 实测）：CONNECT 拿到能力位，GET_DAQ_PROCESSOR_INFO
//! / GET_DAQ_EVENT_INFO 的两种响应布局都能解，动态 DAQ 分配全序列
//! （FREE_DAQ → ALLOC_DAQ → ALLOC_ODT → ALLOC_ODT_ENTRY → SET_DAQ_PTR →
//! WRITE_DAQ → SET_DAQ_LIST_MODE(timestamp) → START_STOP_SYNCH）被参考
//! 实现逐条接受。
//!
//! 未达成（如实记录，不谎报）：DTO 数据流。所有命令都通过、从站侧
//! 「DAQ processing start」也有日志，但参考实现不发出 DTO；其 DTO 头
//! 还是私有格式（pid+保留+daq 词+32 位时间戳），两层适配攒下一轮。

use std::net::SocketAddr;
use std::sync::Arc;

use openape_bus::CanId;
use openape_cal_core::{Address, ByteLen};
use openape_xcp::CanXcpMaster;
use openape_xcp::XcpConfig;
use openape_xcp::connection::XcpConnection;
use openape_xcp::daq::DaqListSpec;
use openape_xcp::eth::{HeaderMode, UdpXcpBus};

fn signal(address: u32, len: u16) -> openape_xcp::daq::DaqSignal {
    openape_xcp::daq::DaqSignal {
        address: Address::new(address),
        len: ByteLen::new(len).expect("长度非零"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "依赖外部 XCPlite 从站：先跑 script/xcplite-interop.sh"]
async fn xcplite_server_accepts_connect_events_and_daq_config() {
    let addr: SocketAddr = std::env::var("XCPLITE_ADDR")
        .unwrap_or_else(|_| "127.0.0.1:5555".to_owned())
        .parse()
        .expect("XCPLITE_ADDR 形如 127.0.0.1:5555");
    let counter_addr = u32::from_str_radix(
        std::env::var("XCPLITE_COUNTER_ADDR")
            .expect("要给出 global_counter 的符号偏移（脚本算）")
            .trim_start_matches("0x"),
        16,
    )
    .expect("地址是十六进制数");

    let bus = Arc::new(
        UdpXcpBus::open(
            addr,
            CanId::standard(0x601).expect("合法"),
            HeaderMode::Headered,
        )
        .await
        .expect("master 要能连上 XCPlite"),
    );
    let mut master = CanXcpMaster::new(
        bus.clone(),
        XcpConfig::new(
            CanId::standard(0x600).expect("合法"),
            CanId::standard(0x601).expect("合法"),
        ),
    );

    // CONNECT：能力位带 DAQ。
    let info = master.connect(0).await.expect("XCPlite 上要能建链");
    assert!(
        info.resource & openape_xcp_proto::resource::DAQ != 0,
        "XCPlite 声明 DAQ 能力"
    );

    // 事件发现：参考实现的精简响应布局（无 name_length 域，名字走
    // UPLOAD 按 0 结尾截）也要能解。
    //
    // 已知错位（2026-11-08 实测）：参考实现的 GET_DAQ_EVENT_INFO 编号
    // 空间与 SET_DAQ_LIST_MODE 的不一致——E5 空间 [0]=V2.3、[1]=空名，
    // 而它自己的日志打印 1=mainloop。事件名在这里不可作配对依据，
    // 验收点收在「机制工作、清单非空」；真实 ECU 的对接以 ECU 手册
    // 的事件号为准。
    let events = master.daq_events().await.expect("XCPlite 支持事件查询");
    assert!(!events.is_empty(), "事件清单不能为空");

    // 动态 DAQ 配置全序列：每条命令都要被接受。timestamp 位是参考
    // 实现的硬要求；地址扩展 0x01 是它默认配置的绝对寻址（0 是标定
    // 段相对，global_counter 不在标定段）。
    let list = DaqListSpec::new(events[0].index, 1, vec![signal(counter_addr, 4)])
        .expect("配置合法")
        .with_timestamp()
        .with_address_extension(0x01);
    master
        .configure_daq(&[list])
        .await
        .expect("XCPlite 要接受动态 DAQ 配置全序列");
}
