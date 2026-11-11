//! 性质测试（RH-01 第 6 条）：解码器吃任意字节不 panic，切分守恒。
//!
//! 只写两条不追语义完备：解析失败与错位是最难靠人眼发现的一类错，
//! 用随机字节把它们顶出来。

// 集成测试里的 expect 是断言手段：失败即测试失败，不走生产那道门。
#![allow(clippy::expect_used)]

use openape_xcp_proto::dto_profile::{ProfileError, STANDARD, XCPLITE, split};
use openape_xcp_proto::{DaqFrame, PID_DAQ_MAX, Response};
use proptest::prelude::*;

fn arb_frame() -> impl Strategy<Value = Vec<u8>> {
    prop::collection::vec(any::<u8>(), 0..=65)
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 128, ..ProptestConfig::default() })]

    #[test]
    fn daq_frame_parse_never_panics_and_strips_exactly_one_byte(frame in arb_frame()) {
        match DaqFrame::parse(&frame) {
            None => prop_assert!(frame.is_empty() || frame[0] > PID_DAQ_MAX),
            Some(parsed) => {
                prop_assert_eq!(parsed.odt, frame[0]);
                prop_assert_eq!(parsed.payload, &frame[1..]);
            }
        }
    }

    #[test]
    fn response_decode_never_panics_and_round_trips(frame in arb_frame()) {
        if let Ok(response) = Response::decode(&frame) {
            prop_assert_eq!(Response::decode(&response.encode()), Ok(response));
        }
    }

    #[test]
    fn split_never_panics_and_conserves_length(
        frame in arb_frame(),
        xcplite in any::<bool>(),
    ) {
        let profile = if xcplite { &XCPLITE } else { &STANDARD };
        match split(profile, &frame) {
            Err(err) => {
                // 现有档案都带齐 Pid/Payload、时间戳宽度合法，唯一的失败面是报文太短。
                let too_short = matches!(err, ProfileError::TooShort { .. });
                prop_assert!(too_short, "档案齐了，失败只能是报文太短：{err}");
            }
            Ok(decoded) => {
                prop_assert_eq!(profile.header_len() + decoded.payload.len(), frame.len());
                if !xcplite {
                    // 标准档案的载荷恰好是 data[1..]。
                    prop_assert_eq!(decoded.payload, &frame[1..]);
                }
            }
        }
    }
}
