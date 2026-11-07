//! 契约的形状约束。这些断言守的是"同一份类型既走 gRPC 也走 CLI `--json`"
//! 这件事——形状变了 CLI 的输出就变了，所以锁在测试里。

use openape_proto as proto;

#[test]
fn cli_json_keeps_snake_case_field_names() {
    let report = proto::BusReport {
        channels: vec!["vcan0".to_owned()],
        probed: None,
    };
    let json = serde_json::to_value(&report).expect("可序列化");
    assert_eq!(json["channels"][0], "vcan0");
    assert!(
        json.get("probed").is_some(),
        "缺省字段仍是字段，不因 None 消失"
    );

    let snapshot = proto::MetricsSnapshot {
        latency_p99_us: 1200,
        ..proto::MetricsSnapshot::default()
    };
    let json = serde_json::to_value(snapshot).expect("可序列化");
    assert_eq!(json["latency_p99_us"], 1200, "不转成 camelCase");
}

#[test]
fn addresses_stay_strings_so_wide_ones_survive_json() {
    // 超过 2^53 的地址在 JS 里会静默错位，所以协议层就不给它变成数字的机会
    let wide = "0x1ffffffffffff";
    let request = proto::ReadRequest {
        address: wide.to_owned(),
        len: 4,
    };
    let json = serde_json::to_value(&request).expect("可序列化");
    assert_eq!(json["address"], wide);
}

#[test]
fn reports_round_trip_through_json() {
    let report = proto::ConnectReport {
        channel: "vcan0".to_owned(),
        info: Some(proto::ConnectInfo {
            resource: 0xff,
            max_cto: 8,
            max_dto: 8,
            protocol_version: Some(proto::ProtocolVersion { major: 1, minor: 5 }),
            ..proto::ConnectInfo::default()
        }),
    };
    let json = serde_json::to_string(&report).expect("可序列化");
    let back: proto::ConnectReport = serde_json::from_str(&json).expect("可反序列化");
    assert_eq!(back, report);
}

#[test]
fn an_unreported_version_stays_absent_instead_of_becoming_zero() {
    let report = proto::ConnectReport {
        channel: "vcan0".to_owned(),
        info: Some(proto::ConnectInfo::default()),
    };
    let json = serde_json::to_value(&report).expect("可序列化");
    assert!(
        json["info"]["protocol_version"].is_null(),
        "从站没上报就是没有，不补 0.0"
    );
}

#[test]
fn the_generated_client_and_messages_are_named_as_expected() {
    // 服务端 trait 由 daemon 的实现证明可用，这里只锁住生成物可被命名
    fn named<T>() -> &'static str {
        std::any::type_name::<T>()
    }
    assert!(
        named::<proto::open_ape_client::OpenApeClient<tonic::transport::Channel>>()
            .contains("OpenApeClient")
    );

    let sample = proto::DaqSample {
        pid: 3,
        timestamp_us: 9,
        payload: vec![1, 2],
    };
    let json = serde_json::to_value(&sample).expect("可序列化");
    assert_eq!(json["payload"], serde_json::json!([1, 2]));
}

#[test]
fn write_shapes_keep_addresses_as_strings_and_audit_fields_queryable() {
    let request = proto::WriteRequest {
        address: "0x00001000".to_owned(),
        len: 2,
        data_hex: "aabb".to_owned(),
        symbol: "cal.kp".to_owned(),
        actor: "cli:alice".to_owned(),
    };
    let json = serde_json::to_value(&request).expect("可序列化");
    assert_eq!(json["address"], "0x00001000");
    assert_eq!(json["symbol"], "cal.kp");

    // 快照与审计字段直接可查：客户端不再自己拼字符串。
    let response = proto::WriteResponse {
        symbol: "cal.kp".to_owned(),
        snapshot_path: "/var/openape/snapshots/1-cal_kp.json".to_owned(),
        range_checked: true,
        range: Some(proto::RangeCheck {
            min: 0.0,
            max: 10.0,
            physical: 5.0,
        }),
        audited_at: "2026-11-07T00:00:00.000Z".to_owned(),
        ..proto::WriteResponse::default()
    };
    let json = serde_json::to_value(&response).expect("可序列化");
    assert_eq!(json["range"]["max"], 10.0);
    assert_eq!(json["range_checked"], true);
    assert!(
        json["audited_at"]
            .as_str()
            .is_some_and(|at| at.ends_with('Z'))
    );
}

#[test]
fn page_diff_says_which_side_is_missing() {
    let diff = proto::PageDiffResponse {
        reference_page: 0,
        working_page: 1,
        differences: vec![proto::Difference {
            symbol: "cal.kp".to_owned(),
            address: "0x00001000".to_owned(),
            reference_hex: "0001".to_owned(),
            working_hex: String::new(),
            in_reference: true,
            in_working: false,
        }],
    };
    let json = serde_json::to_value(&diff).expect("可序列化");
    assert_eq!(json["differences"][0]["in_working"], false);
    assert_eq!(
        json["differences"][0]["working_hex"], "",
        "空串与缺失的区分靠 in_* 布尔，不靠长度"
    );
}
