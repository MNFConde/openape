//! 读回与 golden 共用的样本：两通道五条，负值、零、非整点都在。
//!
//! 数字刻意不平滑：`-3.25` 防止符号位写坏也看不出来，`0.0` 防止全零
//! 记录被默认值掩盖，`99.0` 防止只验证了低字节。

#![allow(dead_code)]
// 集成测试里的 expect 是断言手段：失败即测试失败，不走生产那道门。
#![allow(clippy::expect_used)]

use std::path::Path;

use openape_format_mf4::{ChannelSpec, Writer};

pub const TIMES: [f64; 5] = [0.0, 0.1, 0.2, 0.3, 0.4];
pub const SPEED: [f64; 5] = [10.0, 20.5, -3.25, 0.0, 99.0];
pub const THROTTLE: [f64; 5] = [0.0, 12.5, 45.0, 8.0, 100.0];

/// 写一份两通道样本文件：通道 `engine_speed`、`throttle`，行序同常量。
pub fn write_two_channels(path: &Path) {
    let mut writer = Writer::create(
        path,
        &[
            ChannelSpec::new("engine_speed"),
            ChannelSpec::new("throttle"),
        ],
    )
    .expect("文件必须可建");
    for i in 0..TIMES.len() {
        writer
            .write_row(TIMES[i], &[SPEED[i], THROTTLE[i]])
            .expect("样本行必须可写");
    }
    writer.finish().expect("收尾必须成功");
}
