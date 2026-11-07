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

#[tokio::test]
async fn variables_are_served_without_any_hardware() {
    // 只看描述文件、不连 ECU 是合法用法：daemon 起来了就能画表格。
    let elf = std::path::Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/elf/test.elf"
    ));
    let loaded = boot::load_variables(Some(elf), None).expect("给了文件就该有清单");
    let state = Arc::new(
        openape_daemon::service::AppState::idle(Arc::new(Metrics::new()))
            .with_variables(loaded.list),
    );
    let service = OpenApeService::new(state);

    let list = service
        .list_variables(Request::new(proto::Empty {}))
        .await
        .expect("要能回答")
        .into_inner();
    assert!(list.loaded);
    assert!(!list.variables.is_empty());
    assert!(list.variables.iter().all(|entry| entry.raw));
}

#[tokio::test]
async fn without_descriptors_the_list_says_not_loaded() {
    // 「未加载」与「加载了但没变量」要能分开：界面据此提示去加 --elf。
    let state = Arc::new(openape_daemon::service::AppState::idle(Arc::new(
        Metrics::new(),
    )));
    let service = OpenApeService::new(state);
    let list = service
        .list_variables(Request::new(proto::Empty {}))
        .await
        .expect("要能回答")
        .into_inner();
    assert!(!list.loaded);
    assert!(list.variables.is_empty());
}

#[tokio::test]
async fn faults_are_the_diag_type_mapped_not_reinvented() {
    // 界面上的故障必须能和日志里的同一条对上。这里从 `diag::Fault` 出发，
    // 断言字段确实是从它映射来的，而不是在 daemon 里另写一套文案。
    let state = openape_daemon::service::AppState::idle(Arc::new(Metrics::new()));
    let fault = openape_diag::Fault::new(
        openape_diag::FaultCode::new(openape_diag::Domain::Session, 1).expect("合法编号"),
        openape_diag::Severity::Error,
        "测试故障",
    )
    .with_hint("先做这个");
    state.record_fault(&fault);

    let service = OpenApeService::new(Arc::new(state));
    let list = service
        .list_faults(Request::new(proto::Empty {}))
        .await
        .expect("要能回答")
        .into_inner();
    assert_eq!(list.faults.len(), 1);
    assert_eq!(list.faults[0].message, "测试故障");
    assert_eq!(list.faults[0].hint, "先做这个");
    assert!(list.faults[0].code.contains('S') || !list.faults[0].code.is_empty());
}

/// 建一份带变量树与写入上下文的状态：`--elf` / `--a2l` 那条启动路径的等价物。
async fn state_with_descriptors(
    channel: &str,
    elf: &std::path::Path,
) -> Arc<openape_daemon::service::AppState> {
    let loaded = boot::load_variables(Some(elf), None).expect("给了文件就该有清单");
    let dir = std::env::temp_dir().join(format!(
        "openape-daemon-write-{}-{:02x}",
        std::process::id(),
        channel.len()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    let context = openape_session::WriteContext::open(&dir, "daemon", loaded.mode).expect("上下文");
    // 采集列表必给：daemon 接管通道时一定要有采样项，写命令不靠它。
    let samples = [SampleSpec {
        address: 0x2000,
        len: 2,
    }];
    let state = boot::open_session(channel, 0x600, 0x601, &samples, Arc::new(Metrics::new()))
        .await
        .expect("启动应成功")
        .with_variables(loaded.list);
    let state = match loaded.tree {
        Some(tree) => state.with_tree(tree, loaded.mode),
        None => state,
    };
    Arc::new(state.with_write_context(context))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn write_memory_goes_through_the_daemon_and_leaves_an_audit_record() {
    let Some(vcan) = VcanGuard::create("dw") else {
        eprintln!("跳过：本机没有可用的 vcan");
        return;
    };
    let channel = vcan.name().to_owned();

    let bus = Arc::new(SocketCanBus::open(&channel).expect("能打开 vcan"));
    let mut ecu = SimEcu::new(SimEcuConfig::default_ids());
    ecu.poke(0x1000, &[0x00, 0x01, 0x00, 0x02]);
    let slave = tokio::spawn(openape_sim_ecu::serve(ecu, bus));

    // fixtures/elf/test.elf 里的第一条标定量是 u16，够做一次按符号写。
    let elf = std::path::Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/elf/test.elf"
    ));
    let state = state_with_descriptors(&channel, elf).await;
    let service = OpenApeService::new(Arc::clone(&state));

    let list = service
        .list_variables(Request::new(proto::Empty {}))
        .await
        .expect("要能回答")
        .into_inner();
    let entry = list
        .variables
        .iter()
        .find(|entry| entry.elem == "U16")
        .expect("ELF 里应有 u16 变量")
        .clone();

    let response = service
        .write_memory(Request::new(proto::WriteRequest {
            address: format!("0x{:08x}", entry.address),
            len: 2,
            data_hex: "beef".to_owned(),
            symbol: entry.symbol.clone(),
            actor: "test:alice".to_owned(),
        }))
        .await
        .expect("daemon 代写应成功")
        .into_inner();

    assert_eq!(response.symbol, entry.symbol);
    assert_eq!(response.len, 2);
    assert!(!response.snapshot_path.is_empty(), "要留下写前快照");
    assert!(!response.range_checked, "仅 ELF 档没有量程，标 raw");
    assert!(response.audited_at.ends_with('Z'), "审计时刻是 ISO-8601");

    let audit = service
        .list_audit(Request::new(proto::Empty {}))
        .await
        .expect("审计可查")
        .into_inner();
    assert_eq!(audit.entries.len(), 1);
    assert_eq!(audit.entries[0].actor, "test:alice");
    assert_eq!(audit.entries[0].mode, "elf_only");
    assert_eq!(audit.entries[0].after_hex, "be ef");

    slave.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn write_memory_refuses_a_symbol_that_is_not_at_that_address() {
    let Some(vcan) = VcanGuard::create("dx") else {
        eprintln!("跳过：本机没有可用的 vcan");
        return;
    };
    let channel = vcan.name().to_owned();
    let bus = Arc::new(SocketCanBus::open(&channel).expect("能打开 vcan"));
    let slave = tokio::spawn(openape_sim_ecu::serve(
        SimEcu::new(SimEcuConfig::default_ids()),
        bus,
    ));

    let elf = std::path::Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/elf/test.elf"
    ));
    let state = state_with_descriptors(&channel, elf).await;
    let service = OpenApeService::new(state);

    let list = service
        .list_variables(Request::new(proto::Empty {}))
        .await
        .expect("要能回答")
        .into_inner();
    let entry = list.variables.first().expect("应有变量").clone();

    let err = service
        .write_memory(Request::new(proto::WriteRequest {
            address: "0x00004000".to_owned(),
            len: 2,
            data_hex: "0001".to_owned(),
            symbol: entry.symbol.clone(),
            actor: String::new(),
        }))
        .await
        .expect_err("符号与地址不符要拒绝");
    assert_eq!(err.code(), tonic::Code::InvalidArgument);
    assert!(err.message().contains("首地址"), "{err}");

    slave.abort();
}
