//! 回放总线的行为：按日志时间出帧、帧序不乱、放完报关闭。
//!
//! 采集通路同一性的验收点：`ReplayBus` 实现 `Bus`，上层拿它与拿
//! SocketCan 是同一个接口——这里直接按总线的用法消费它，不加任何
//! 回放专用分支。

use std::time::Duration;

use openape_bus::{Bus, BusError, CanFrame, CanId};
use openape_bus_replay::{ReplayBus, ReplayFrame};

fn frame(id: u16, data: u8) -> CanFrame {
    CanFrame::new(CanId::standard(id).expect("合法"), vec![data]).expect("帧合法")
}

/// 反复 recv 直到拿到帧（上层采集泵就是这样顶着超时轮询的），
/// 返回帧与「从建总线起的流逝毫秒」。
async fn pump_until(bus: &ReplayBus, started: std::time::Instant) -> (CanFrame, u128) {
    loop {
        match bus.recv(Duration::from_millis(20)).await {
            Ok(frame) => return (frame, started.elapsed().as_millis()),
            Err(BusError::Timeout { .. }) => continue,
            Err(err) => panic!("回放中途不应有别的错误：{err}"),
        }
    }
}

#[tokio::test]
async fn frames_arrive_in_log_order_keeping_the_log_pace() {
    let bus = ReplayBus::new(vec![
        ReplayFrame {
            offset_us: 0,
            frame: frame(0x100, 1),
        },
        ReplayFrame {
            offset_us: 50_000,
            frame: frame(0x101, 2),
        },
        ReplayFrame {
            offset_us: 120_000,
            frame: frame(0x102, 3),
        },
    ]);
    let started = std::time::Instant::now();

    let (first, at) = pump_until(&bus, started).await;
    assert_eq!(first.data[0], 1, "首帧立即可收");
    assert!(at < 20, "首帧不等日志时间：{at} ms");

    let (second, at) = pump_until(&bus, started).await;
    assert_eq!(second.data[0], 2);
    assert!(
        (50..=90).contains(&at),
        "第二帧要等 50 ms 的日志间隔（留调度余量）：{at} ms"
    );

    let (third, at) = pump_until(&bus, started).await;
    assert_eq!(third.data[0], 3);
    assert!(
        (120..=170).contains(&at),
        "第三帧按日志间隔 120 ms 到：{at} ms"
    );

    let err = bus
        .recv(Duration::from_millis(50))
        .await
        .expect_err("放完要报关闭");
    assert!(err.to_string().contains("关闭"), "{err}");
}

/// D-25：回放喂入保留日志时间——帧的时间戳是「起表时刻 + 日志偏移」，
/// 不是接收时刻；混源（一半墙钟一半日志时间）从这里挡住。
#[tokio::test]
async fn frames_carry_the_log_timestamp_not_the_receive_moment() {
    let bus = ReplayBus::new(vec![
        ReplayFrame {
            offset_us: 100_000,
            frame: frame(0x100, 1),
        },
        ReplayFrame {
            offset_us: 300_000,
            frame: frame(0x101, 2),
        },
    ]);
    let started = std::time::Instant::now();
    let (first, _) = pump_until(&bus, started).await;
    let (second, _) = pump_until(&bus, started).await;

    let at = |f: &CanFrame| {
        f.timestamp
            .map(|t| t.duration_since(started).as_micros())
            .unwrap_or_else(|| panic!("回放帧必须带日志时间戳"))
    };
    let first_at = at(&first);
    assert!(
        (90_000..=140_000).contains(&first_at),
        "首帧时间戳要对齐日志 100 ms 偏移：{first_at} µs"
    );
    let second_at = at(&second);
    assert!(
        (290_000..=350_000).contains(&second_at),
        "次帧时间戳要对齐日志 300 ms 偏移：{second_at} µs"
    );
}
