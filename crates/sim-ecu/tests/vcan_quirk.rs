//! 怪癖注入的 vcan 复现（M5-07 验收）：注入的填充与回显差异在总线上
//! 看得见。配置走 M5-06 的 TOML——两份配置两种从站行为，全程无新代码。
//!
//! 三段验证串行走，不交错：DAQ 配置全程由 master 负责（它的读回确认
//! 对响应格式有假设，混入手写 CRO 会互相干扰）；随后在 DTO 上验填充，
//! 最后手写切页与 GET_CAL_PAGE 的 CRO 验回显——master 对短回显的读回
//! 会失败，这本身就是怪癖差异的一部分，切页因此绕开 master 的 API。

use std::sync::Arc;
use std::time::Duration;

use openape_bus::{Bus, CanFrame, CanId};
use openape_bus_socketcan::SocketCanBus;
use openape_bus_socketcan::testing::VcanGuard;
use openape_cal_core::quirk::QuirkConfig;
use openape_sim_ecu::{SimEcu, SimEcuConfig};
use openape_xcp::{CanXcpMaster, DaqListSpec, DaqSignal, XcpConfig, XcpConnection};

fn command_id() -> CanId {
    CanId::standard(0x600).expect("0x600 在 11 位内")
}

fn response_id() -> CanId {
    CanId::standard(0x601).expect("0x601 在 11 位内")
}

fn signal(address: u32, len: u16) -> DaqSignal {
    DaqSignal {
        address: openape_cal_core::Address::new(address),
        len: openape_cal_core::ByteLen::new(len).expect("测试长度非零"),
    }
}

fn cro(command_id: CanId, payload: Vec<u8>) -> CanFrame {
    CanFrame::new(command_id, payload).expect("CRO 帧合法")
}

/// 起一台按 `quirks` 配置的从站，返回 master 与观察总线。
async fn setup(
    name: &str,
    quirks: &str,
) -> Option<(
    openape_bus_socketcan::testing::VcanGuard,
    CanXcpMaster,
    Arc<dyn Bus>,
    tokio::task::JoinHandle<Result<(), openape_diag::Fault>>,
)> {
    let vcan = VcanGuard::create(name)?;
    let slave_bus: Arc<dyn Bus> = Arc::new(SocketCanBus::open(vcan.name()).expect("从站总线"));
    let mut ecu = SimEcu::new(SimEcuConfig::default_ids());
    ecu.poke(0x1000, &[0x11, 0x22]);
    let ecu = ecu.with_quirks(QuirkConfig::from_toml(quirks).expect("怪癖配置必须合法"));
    let slave = tokio::spawn(openape_sim_ecu::serve(ecu, slave_bus));

    let observer: Arc<dyn Bus> = Arc::new(SocketCanBus::open(vcan.name()).expect("观察总线"));
    let master_bus: Arc<dyn Bus> = Arc::new(SocketCanBus::open(vcan.name()).expect("master 总线"));
    let master = CanXcpMaster::new(master_bus, XcpConfig::new(command_id(), response_id()));
    Some((vcan, master, observer, slave))
}

async fn next_frame(bus: &Arc<dyn Bus>, take: impl Fn(&CanFrame) -> bool) -> CanFrame {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        let frame = bus.recv(remaining).await.expect("应等到目标帧");
        if take(&frame) {
            return frame;
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn injected_quirks_show_up_on_the_bus() {
    let Some((_vcan, mut master, observer, slave)) = setup(
        "q1",
        "pad_dtos = true\ndto_padding = 255\npage_echo = \"short\"\n",
    )
    .await
    else {
        return;
    };
    master.connect(0).await.expect("建链成功");

    // 填充差异：2 字节信号的 DTO 实长 3 字节，注入后补齐到 8、尾填 0xFF。
    master
        .configure_daq(&[DaqListSpec::new(0, 1, vec![signal(0x1000, 2)]).expect("配置合法")])
        .await
        .expect("配置应成功");
    let dto = next_frame(&observer, |frame| {
        frame.id == response_id() && frame.data.len() == 8
    })
    .await;
    assert_eq!(&dto.data[..3], &[0x00, 0x11, 0x22], "PID + 实数据在前");
    assert!(
        dto.data[3..].iter().all(|byte| *byte == 0xFF),
        "缺位补配置的 0xFF：{:?}",
        dto.data
    );

    // 回显差异：master.set_cal_page 会读回确认，短回显下读回必失败——
    // 这本身就是怪癖差异。切页与查询都走手写 CRO，不经 master 的解析。
    observer
        .send(cro(command_id(), vec![0xEB, 0x00, 0x00, 0x01]))
        .await
        .expect("切页 CRO 发得出去");
    let cro_id = command_id();
    next_frame(&observer, |frame| {
        frame.id == response_id() && frame.data.first() == Some(&0xFF)
    })
    .await;
    observer
        .send(cro(cro_id, vec![0xEA, 0x00, 0x00]))
        .await
        .expect("GET CRO 发得出去");
    // 从站对 CRO 的空响应（切页、启动等）也是 0xFF 开头，靠页号位置认回显帧。
    let echo = next_frame(&observer, |frame| {
        frame.data.first() == Some(&0xFF) && frame.data.get(1) == Some(&0x01)
    })
    .await;
    assert_eq!(
        &echo.data[..2],
        &[0xFF, 0x01],
        "短回显：命令码后紧跟页号 1，没有保留字节"
    );
    slave.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn without_quirks_the_frames_stay_as_before() {
    let Some((_vcan, mut master, observer, slave)) = setup("q2", "").await else {
        return;
    };
    master.connect(0).await.expect("建链成功");

    // 空配置 = 规范行为：DTO 按数据实长发（PID + 2 字节 = 3）。
    master
        .configure_daq(&[DaqListSpec::new(0, 1, vec![signal(0x1000, 2)]).expect("配置合法")])
        .await
        .expect("配置应成功");
    let dto = next_frame(&observer, |frame| {
        // 配置期间的响应帧还在缓冲里，用 DTO 的 PID 前缀认帧。
        frame.id == response_id() && frame.data.first() == Some(&0x00)
    })
    .await;
    assert_eq!(dto.data, vec![0x00, 0x11, 0x22], "不补齐：帧长就是数据实长");

    // 同样的手写链路，GET_CAL_PAGE 回标准三字节格式。
    observer
        .send(cro(command_id(), vec![0xEB, 0x00, 0x00, 0x01]))
        .await
        .expect("切页 CRO 发得出去");
    let cro_id = command_id();
    next_frame(&observer, |frame| {
        frame.id == response_id() && frame.data.first() == Some(&0xFF)
    })
    .await;
    observer
        .send(cro(cro_id, vec![0xEA, 0x00, 0x00]))
        .await
        .expect("GET CRO 发得出去");
    let echo = next_frame(&observer, |frame| {
        frame.data.first() == Some(&0xFF) && frame.data.get(3) == Some(&0x01)
    })
    .await;
    assert_eq!(
        &echo.data[..4],
        &[0xFF, 0x00, 0x00, 0x01],
        "标准回显：命令码 + 两个保留字节 + 页号 1"
    );
    slave.abort();
}
