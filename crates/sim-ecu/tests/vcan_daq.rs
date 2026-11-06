//! DAQ 端到端：master 配置两个 ODT，从站按事件周期出帧，master 把帧切回
//! 各内存块的值。这是 M2-08 与 M2-09 的合并验收——两侧任何一边只做一半都
//! 跑不通，所以只能端到端验。

use std::sync::Arc;
use std::time::Duration;

use openape_bus::{Bus, CanId};
use openape_bus_socketcan::SocketCanBus;
use openape_bus_socketcan::testing::VcanGuard;
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

fn config() -> SimEcuConfig {
    SimEcuConfig {
        event_period: Duration::from_millis(10),
        ..SimEcuConfig::default_ids()
    }
}

/// 起从站并返回 master。测试与 master 各用一条 socket，vcan 会把帧分发给两个。
async fn setup(
    name: &str,
    config: SimEcuConfig,
) -> Option<(
    openape_bus_socketcan::testing::VcanGuard,
    CanXcpMaster,
    Arc<dyn Bus>,
    tokio::task::JoinHandle<Result<(), openape_diag::Fault>>,
    Arc<Metrics>,
)> {
    let vcan = VcanGuard::create(name)?;
    let metrics = Arc::new(Metrics::new());

    let slave_bus: Arc<dyn Bus> = Arc::new(SocketCanBus::open(vcan.name()).expect("从站总线"));
    let mut ecu = SimEcu::new(config);
    ecu.poke(0x1000, &[0x11, 0x22]);
    ecu.poke(0x1002, &[0x33, 0x44, 0x55, 0x66]);
    ecu.poke(0x1006, &[0x77, 0x88]);
    let slave = tokio::spawn(openape_sim_ecu::serve_with_metrics(
        ecu,
        slave_bus,
        metrics.clone(),
    ));

    let observer: Arc<dyn Bus> = Arc::new(SocketCanBus::open(vcan.name()).expect("观察总线"));
    let master_bus: Arc<dyn Bus> = Arc::new(SocketCanBus::open(vcan.name()).expect("master 总线"));
    let master = CanXcpMaster::new(
        master_bus,
        XcpConfig::new(
            CanId::standard(0x600).expect("合法"),
            CanId::standard(0x601).expect("合法"),
        ),
    );
    Some((vcan, master, observer, slave, metrics))
}

/// 收一帧 DAQ DTO。
///
/// 两条件缺一不可：标识符必须是响应标识符，byte0 必须落在 DTO 编号区间。
/// 只看 byte0 会把 CRO 上的命令帧（`D5 00 01 00` 的 byte0 是 0xD5）当成 DTO——
/// 命令与数据走不同标识符，这一层过滤不能只靠 PID。
async fn next_dto(bus: &Arc<dyn Bus>, timeout: Duration) -> openape_bus::CanFrame {
    let response_id = CanId::standard(0x601).expect("合法");
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        let frame = bus.recv(remaining).await.expect("应收到 DAQ 帧");
        if frame.id == response_id && DaqFrame::parse(&frame.data).is_some() {
            return frame;
        }
    }
}

fn payload_of(layout: &DaqLayout, frame: &openape_bus::CanFrame) -> Vec<u8> {
    let dto = DaqFrame::parse(&frame.data).expect("是 DTO");
    layout
        .decode(dto.odt, dto.payload)
        .expect("布局认识这个 ODT")
        .concat()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn daq_frames_carry_the_signal_values() {
    let Some((_vcan, mut master, observer, slave, _metrics)) = setup("d1", config()).await else {
        return;
    };
    master.connect(0).await.expect("建链成功");
    // 2 + 4 + 2 = 8 字节，单个 ODT 只装 7 字节 → 必须切成两个
    let layout = master
        .configure_daq(&[DaqListSpec::new(
            0,
            1,
            vec![signal(0x1000, 2), signal(0x1002, 4), signal(0x1006, 2)],
        )
        .expect("配置合法")])
        .await
        .expect("配置应成功");

    assert_eq!(layout.len(), 2, "8 字节要两个 ODT");

    let first = next_dto(&observer, Duration::from_secs(2)).await;
    let second = next_dto(&observer, Duration::from_secs(2)).await;
    let (a, b) = if first.data[0] == 0 {
        (first, second)
    } else {
        (second, first)
    };

    let first_payload = payload_of(&layout, &a);
    let second_payload = payload_of(&layout, &b);
    assert_eq!(
        first_payload,
        vec![0x11, 0x22, 0x33, 0x44, 0x55, 0x66],
        "第一个 ODT 装前两个信号"
    );
    assert_eq!(second_payload, vec![0x77, 0x88], "第二个 ODT 装第三个信号");

    let stamps = [a.timestamp, b.timestamp];
    assert!(
        stamps[0].zip(stamps[1]).is_none_or(|(x, y)| x <= y),
        "时间戳要单调：{stamps:?}"
    );
    slave.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn out_of_range_signal_is_counted_as_dropped() {
    let Some((_vcan, mut master, _observer, slave, metrics)) = setup("d2", config()).await else {
        return;
    };
    master.connect(0).await.expect("建链成功");
    // MEM_SIZE 是 64 KiB：0xFFFE + 4 越界。四字节装得进一个 ODT，所以配置
    // 阶段不会被拒；出帧时才发现取不到数据——这正是要点：越界不是配置错误。
    master
        .configure_daq(&[DaqListSpec::new(0, 1, vec![signal(0xFFFE, 4)]).expect("配置合法")])
        .await
        .expect("配置阶段看不出越界");

    tokio::time::sleep(Duration::from_millis(120)).await;
    let snapshot = metrics.snapshot();
    assert!(
        snapshot.dropped_frames_total >= 3,
        "10 ms 周期跑 120 ms，越界的帧应逐帧计入丢帧：{snapshot:?}"
    );
    assert_eq!(snapshot.daq_frames_total, 0, "越界帧一个都不该发出去");
    slave.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn jitter_keeps_the_rate_inside_the_band() {
    let jittery = SimEcuConfig {
        event_period: Duration::from_millis(20),
        jitter: Duration::from_millis(5),
        ..SimEcuConfig::default_ids()
    };
    let Some((_vcan, mut master, observer, slave, _metrics)) = setup("d3", jittery).await else {
        return;
    };
    master.connect(0).await.expect("建链成功");
    master
        .configure_daq(&[DaqListSpec::new(0, 1, vec![signal(0x1000, 2)]).expect("配置合法")])
        .await
        .expect("配置应成功");

    let mut stamps = Vec::new();
    for _ in 0..4 {
        stamps.push(next_dto(&observer, Duration::from_secs(2)).await.timestamp);
    }
    let deltas: Vec<Duration> = stamps
        .windows(2)
        .filter_map(|pair| match (pair[0], pair[1]) {
            (Some(a), Some(b)) => Some(b.saturating_duration_since(a)),
            _ => None,
        })
        .collect();
    assert!(!deltas.is_empty(), "要能拿到时间戳");
    for delta in deltas {
        assert!(
            delta >= Duration::from_millis(12) && delta <= Duration::from_millis(60),
            "间隔应落在 20±5 ms 附近（留调度余量），实际 {delta:?}"
        );
    }
    slave.abort();
}
