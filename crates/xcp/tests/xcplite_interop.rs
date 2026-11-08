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
//! 实现逐条接受，**DTO 数据流已通到总线层**：配置后每 1ms 一拍，载荷是
//! `global_counter` 的 u32 且逐帧递增。
//!
//! 头按生产档案剥（D-27）：参考实现的 DTO 头是私有布局
//! `[相对 ODT 号][0xAA 对齐][DAQ list u16 LE][32 位时间戳][数据]`（源码
//! `xcplite.c` 的 ODT_HEADER_SIZE==4 分支），比标准的 `[PID][数据]` 多
//! 7 字节且 PID 是 list 内相对号。本测试直接用
//! [`openape_xcp_proto::dto_profile::XCPLITE`] 剥头，跟生产解码同一份
//! 字段表——以前这里手写过一个私有剥头函数，那等于同一条布局有两份实现。
//!
//! 仍未接：采集泵的档案接线（`capture::CaptureConfig::with_dto`）已到位，
//! 但 daemon 还没有选档案的入口，所以本测试走的是档案 + 总线，不是整条
//! 会话链。

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use openape_bus::{Bus, CanId};
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
    // 已知差异（2026-11-08 实测）：参考实现 GET_DAQ_EVENT_INFO 返回的
    // 名字不可信——UPLOAD 读回的是 "V2.3"（EPK 版本串），不是事件名；
    // 而 A2L（`tmp/XCPlite/hello_xcp_V2.3.a2l`）给出的权威事件是
    // EVENT 0 = calc_power、EVENT 1 = mainloop。**事件号按 A2L 取，不按
    // 这里返回的名字配对**；名称解析本身留待验证（不影响编号）。
    let events = master.daq_events().await.expect("XCPlite 支持事件查询");
    assert!(events.len() >= 2, "事件清单非空，实际 {}", events.len());

    // 动态 DAQ 配置全序列：每条命令都要被接受。timestamp 位是参考
    // 实现的硬要求；地址扩展 0x01 是它默认配置的绝对寻址（0 是标定
    // 段相对，global_counter 不在标定段）。
    //
    // 事件号 1 = mainloop（A2L 权威）：global_counter 挂在它上面、每
    // 1ms 触发一次。**配成 EVENT 0 会静默成功但永不出帧**——calc_power
    // 只在函数被调用时触发，这就是第一轮「命令全过却没有 DTO」的根因。
    let list = DaqListSpec::new(1, 1, vec![signal(counter_addr, 4)])
        .expect("配置合法")
        .with_timestamp()
        .with_address_extension(0x01);
    master
        .configure_daq(&[list])
        .await
        .expect("XCPlite 要接受动态 DAQ 配置全序列");

    // DTO 到达：取两帧，按生产档案剥头后载荷是递增的计数器。
    let first = bus.recv(Duration::from_secs(2)).await.expect("要收到 DTO");
    let second = bus
        .recv(Duration::from_secs(2))
        .await
        .expect("要收到第二帧");
    let profile = &openape_xcp_proto::dto_profile::XCPLITE;
    let first = openape_xcp_proto::dto_profile::split(profile, &first.data)
        .expect("第一帧是 XCPlite 的 DTO");
    let second = openape_xcp_proto::dto_profile::split(profile, &second.data)
        .expect("第二帧是 XCPlite 的 DTO");
    assert_eq!(first.pid, 0, "单 list 单 ODT，相对编号应为 0");
    assert_eq!(first.daq_list, Some(0), "DAQ list 号应为 0");
    assert!(first.timestamp.is_some(), "档案声明了时间戳，就该有值");
    assert_eq!(first.payload.len(), 4, "global_counter 是 u32");
    let first_value = u32::from_le_bytes(first.payload.try_into().expect("4 字节"));
    let second_value = u32::from_le_bytes(second.payload.try_into().expect("4 字节"));
    assert!(
        second_value > first_value,
        "计数器要递增：{first_value} → {second_value}"
    );
}
