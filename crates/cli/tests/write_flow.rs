//! 写入编排在真总线上的行为：写前读回、快照、写后校验、审计、回滚。
//!
//! 放在 L5 外壳里跑：只有这里能同时装起从站与编排层（同档 crate 不互相
//! 依赖）。没有 vcan 时跳过并返回——跳过必须留痕，否则"全绿"会掩盖没测。

use std::path::PathBuf;
use std::sync::Arc;

use openape_bus::Bus;
use openape_bus_socketcan::SocketCanBus;
use openape_bus_socketcan::testing::VcanGuard;
use openape_cal_core::{
    Address, ByteLen, ElemType, Range, Semantics, SourceMode, Symbol, Variable, VariableTree,
};
use openape_session::{RangeVerdict, Session, SessionConfig, WriteContext, read_snapshot};
use openape_sim_ecu::{SimEcu, SimEcuConfig};
use openape_xcp_proto::{Command, ExampleSeedKey};

fn variable(name: &str, address: u32) -> Variable {
    Variable::new(
        Symbol::new(name).expect("测试符号合法"),
        Address::new(address),
        ElemType::U16,
    )
}

/// 从站内部切页用的命令帧：poke 只写活动页，摆两页的值要先切页。
fn switch_page(page: u8) -> Vec<u8> {
    Command::SetCalPage {
        mode: 0,
        segment: 0,
        page,
    }
    .encode()
    .expect("一帧放得下")
}

/// 每个用例一个目录：并行跑时共用目录会让审计条数互相污染。
fn tempdir(case: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("openape-writeflow-{}-{case}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("建临时目录");
    dir
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn write_records_snapshot_and_audit_and_can_be_rolled_back() {
    let Some(vcan) = VcanGuard::create("w") else {
        return;
    };

    let bus: Arc<dyn Bus> = Arc::new(SocketCanBus::open(vcan.name()).expect("从站总线"));
    let mut ecu =
        SimEcu::new(SimEcuConfig::default_ids()).with_protection(Arc::new(ExampleSeedKey));
    ecu.poke(0x1000, &[0x01, 0x02]);
    let slave = tokio::spawn(openape_sim_ecu::serve(ecu, bus));

    let dir = tempdir("rollback");
    let context = WriteContext::open(&dir, "test:actor", SourceMode::WithA2l).expect("上下文");
    let target = variable("cal.kp", 0x1000);
    let mut session = Session::open(SessionConfig::new(vcan.name())).expect("打开会话");
    session.connect(0).await.expect("建链");

    // 未解锁：写入被拒，ECU 里的值不变。快照在写之前就落盘了——写到一半
    // 掉电时它是唯一的依据，宁可多留一份也不留隐患。
    let err = session
        .write_one(&target, &[0xAA, 0xBB], &context)
        .await
        .expect_err("被保护的资源写不进去");
    assert!(err.hint().is_some(), "未解锁要给可操作建议：{err}");
    assert_eq!(
        session
            .read(target.address(), ByteLen::TWO)
            .await
            .expect("读回"),
        vec![0x01, 0x02],
        "被拒的写入不能改动 ECU"
    );

    session.unlock().await.expect("解锁");

    let written = session
        .write_one(&target, &[0xAA, 0xBB], &context)
        .await
        .expect("写入成功");
    assert_eq!(written.before, vec![0x01, 0x02], "写前读回");
    assert_eq!(written.after, vec![0xAA, 0xBB], "写后读回");

    let snapshot = read_snapshot(&written.snapshot).expect("快照可读回");
    assert_eq!(
        snapshot.entries()[0].raw(),
        &[0x01, 0x02],
        "快照存的是写前值，回滚才有依据"
    );

    let records = context.audit().records().expect("审计可读");
    assert_eq!(records.len(), 1, "一次写入一条记录");
    assert_eq!(records[0].actor, "test:actor", "谁写的");
    assert_eq!(records[0].before, vec![0x01, 0x02]);
    assert_eq!(records[0].after, vec![0xAA, 0xBB]);

    // 回滚：快照里的值写回去，同样留痕。
    session
        .restore_snapshot(&snapshot, &context)
        .await
        .expect("回滚成功");
    let current = session
        .read(target.address(), ByteLen::TWO)
        .await
        .expect("读回");
    assert_eq!(current, vec![0x01, 0x02], "回滚后 ECU 里是写前值");
    assert_eq!(context.audit().records().expect("审计可读").len(), 2);

    let _ = session.disconnect().await;
    slave.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn out_of_range_write_is_refused_before_it_reaches_the_ecu() {
    let Some(vcan) = VcanGuard::create("y") else {
        return;
    };

    let bus: Arc<dyn Bus> = Arc::new(SocketCanBus::open(vcan.name()).expect("从站总线"));
    let mut ecu = SimEcu::new(SimEcuConfig::default_ids());
    ecu.poke(0x1000, &[0x00, 0x05]);
    let slave = tokio::spawn(openape_sim_ecu::serve(ecu, bus));

    let dir = tempdir("range");
    let context = WriteContext::open(&dir, "test:actor", SourceMode::WithA2l).expect("上下文");
    let mut session = Session::open(SessionConfig::new(vcan.name())).expect("打开会话");
    session.connect(0).await.expect("建链");

    // 带 A2L 量程：0..=10，写 100 必须被拦。
    let ranged = variable("cal.kp", 0x1000)
        .with_semantics(Semantics::none().with_range(Range::new(0.0, 10.0).expect("合法量程")));
    let err = session
        .write_one(&ranged, &[0x00, 0x64], &context)
        .await
        .expect_err("100 越界");
    assert!(err.message().contains("0..=10"), "{err}");
    assert_eq!(
        session
            .read(ranged.address(), ByteLen::TWO)
            .await
            .expect("读回"),
        vec![0x00, 0x05],
        "被拦下的写入不能落到 ECU"
    );

    // 无量程（仅 ELF 档）：不校验，但要如实报告"未校验"。
    let raw = variable("cal.kp", 0x1000);
    let written = session
        .write_one(&raw, &[0x00, 0x64], &context)
        .await
        .expect("没量程就不拦");
    assert_eq!(written.range, RangeVerdict::Unchecked);

    let _ = session.disconnect().await;
    slave.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn page_diff_lists_only_differences_and_copy_needs_an_explicit_list() {
    let Some(vcan) = VcanGuard::create("z") else {
        return;
    };

    let bus: Arc<dyn Bus> = Arc::new(SocketCanBus::open(vcan.name()).expect("从站总线"));
    let mut ecu = SimEcu::new(SimEcuConfig::default_ids());
    // 页 0（参考页）：kp=1, ki=2；页 1（工作页）：kp=1, ki=9。
    // poke 只写活动页，所以靠从站的 SET_CAL_PAGE 在两页之间摆值。
    ecu.poke(0x1000, &[0x00, 0x01]);
    ecu.poke(0x1002, &[0x00, 0x02]);
    ecu.handle(&switch_page(1));
    ecu.poke(0x1000, &[0x00, 0x01]);
    ecu.poke(0x1002, &[0x00, 0x09]);
    ecu.handle(&switch_page(0));
    let slave = tokio::spawn(openape_sim_ecu::serve(ecu, bus));

    let dir = tempdir("pages");
    let context = WriteContext::open(&dir, "test:actor", SourceMode::WithA2l).expect("上下文");
    let mut session = Session::open(SessionConfig::new(vcan.name())).expect("打开会话");
    session.connect(0).await.expect("建链");

    let tree = VariableTree::new(vec![variable("cal.kp", 0x1000), variable("cal.ki", 0x1002)])
        .expect("无重复");
    let differences = session
        .diff_pages(&tree, 0, 1, SourceMode::WithA2l)
        .await
        .expect("能对比");
    let names: Vec<&str> = differences
        .iter()
        .map(|difference| difference.symbol().as_str())
        .collect();
    assert_eq!(names, vec!["cal.ki"], "只列差异地址");

    // 空列表复制被拒：没有整页覆盖入口。
    let reference = session
        .page_dataset(&tree, 0, SourceMode::WithA2l)
        .await
        .expect("参考页取样");
    let err = session
        .copy_to_working_page(&tree, &reference, 1, &[], &context)
        .await
        .expect_err("空列表不许复制");
    assert!(err.hint().is_some(), "{err}");

    let approved = [Symbol::new("cal.ki").expect("合法符号")];
    session
        .copy_to_working_page(&tree, &reference, 1, &approved, &context)
        .await
        .expect("逐项复制成功");

    session.set_cal_page(1).await.expect("切到工作页核对");
    let copied = session
        .read(Address::new(0x1002), ByteLen::TWO)
        .await
        .expect("读回");
    assert_eq!(copied, vec![0x00, 0x02], "工作页的 ki 已与参考页一致");
    session.set_cal_page(0).await.expect("切回参考页");
    let kept = session
        .read(Address::new(0x1002), ByteLen::TWO)
        .await
        .expect("读回");
    assert_eq!(kept, vec![0x00, 0x02], "参考页没被改动");

    let _ = session.disconnect().await;
    slave.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wrong_length_is_refused_before_reading_the_ecu() {
    let Some(vcan) = VcanGuard::create("x") else {
        return;
    };

    let bus: Arc<dyn Bus> = Arc::new(SocketCanBus::open(vcan.name()).expect("从站总线"));
    let slave = tokio::spawn(openape_sim_ecu::serve(
        SimEcu::new(SimEcuConfig::default_ids()),
        bus,
    ));

    let dir = tempdir("length");
    let context = WriteContext::open(&dir, "test:actor", SourceMode::ElfOnly).expect("上下文");
    let mut session = Session::open(SessionConfig::new(vcan.name())).expect("打开会话");
    session.connect(0).await.expect("建链");

    let target = variable("cal.kp", 0x1000);
    let err = session
        .write_one(&target, &[0x01], &context)
        .await
        .expect_err("u16 只给了 1 字节");
    assert!(err.message().contains("2 字节"), "要点明长度：{err}");

    let _ = session.disconnect().await;
    slave.abort();
}
