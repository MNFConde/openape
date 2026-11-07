//! 验收（M3-09）：同一帧既能出原始位也能出物理值。
//!
//! 两者并排是刻意的：物理值经过因子与偏移，DBC 的单位写错时只有原始位
//! 还说得清收到了什么。

use std::path::Path;

use openape_format_dbc::load_str;
use openape_session::CanObserver;

fn observer() -> CanObserver {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/dbc/typical.dbc");
    let text = std::fs::read_to_string(path).expect("DBC 夹具必须入库");
    CanObserver::new(load_str(&text).expect("夹具必须能解析"))
}

#[test]
fn one_frame_yields_both_raw_bits_and_physical_values() {
    let observer = observer();
    // EngineSpeed 是 0|16@1+ 因子 0.25，原始值 0x2710 = 10000 → 2500 rpm
    let frame = [0x10, 0x27, 0x50, 0x64, 0x00, 0x00, 0x00, 0x00];
    let decoded = observer.decode(256, &frame).expect("这条报文在 DBC 里");
    assert_eq!(decoded.message_name(), "EngineData");

    let speed = decoded
        .signals()
        .iter()
        .find(|signal| signal.name() == "EngineSpeed")
        .expect("信号在");
    assert_eq!(speed.raw(), 0x2710, "原始位必须原样保留");
    assert!(
        (speed.physical() - 2500.0).abs() < 1e-9,
        "实为 {}",
        speed.physical()
    );
    assert_eq!(speed.unit(), "rpm");
}

#[test]
fn signed_signals_keep_their_sign_in_both_forms() {
    let observer = observer();
    // CoolantTemp 是 8 位有符号，偏移 -40：原始 0x50=80 → 40 degC
    let frame = [0x00, 0x00, 0x50, 0x00, 0x00, 0x00, 0x00, 0x00];
    let decoded = observer.decode(256, &frame).expect("报文在");
    let temp = decoded
        .signals()
        .iter()
        .find(|signal| signal.name() == "CoolantTemp")
        .expect("信号在");
    assert_eq!(temp.raw(), 80);
    assert!((temp.physical() - 40.0).abs() < 1e-9);
}

#[test]
fn negative_physical_values_survive_the_round_trip() {
    let observer = observer();
    // 原始 0xF0 = -16（有符号 8 位），加偏移 -40 → -56 degC
    let frame = [0x00, 0x00, 0xF0, 0x00, 0x00, 0x00, 0x00, 0x00];
    let decoded = observer.decode(256, &frame).expect("报文在");
    let temp = decoded
        .signals()
        .iter()
        .find(|signal| signal.name() == "CoolantTemp")
        .expect("信号在");
    assert_eq!(temp.raw(), -16, "原始值要带符号");
    assert!((temp.physical() - (-56.0)).abs() < 1e-9);
}

#[test]
fn unknown_ids_are_ignored_not_treated_as_errors() {
    // 总线上的报文远多于我们关心的那些，未知 id 是常态。
    let observer = observer();
    assert!(observer.decode(0x1234, &[0u8; 8]).is_none());
}

#[test]
fn every_signal_of_the_message_is_reported() {
    let observer = observer();
    let decoded = observer.decode(256, &[0u8; 8]).expect("报文在");
    let names: Vec<&str> = decoded.signals().iter().map(|s| s.name()).collect();
    assert_eq!(
        names,
        vec!["EngineSpeed", "CoolantTemp", "ThrottlePos", "BigEndianMix"],
        "定义顺序要保留，否则界面上信号会跳来跳去"
    );
}
