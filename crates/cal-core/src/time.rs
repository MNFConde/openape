//! 时间戳。没有引入日期库：只用到"Unix 毫秒 + 转 ISO-8601 文本"两件事，
//! 手写二十行比多一条依赖更划算。
//!
//! 时区一律 UTC 并带 `Z` 后缀——审计日志与快照的时间要能与其他工具对齐，
//! 本地时区会让两台机器上的记录无法比较。

use std::fmt;
use std::time::{SystemTime, UNIX_EPOCH};

/// 一个时刻，精度到毫秒。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Timestamp {
    unix_ms: u64,
}

impl Timestamp {
    /// 从 Unix 毫秒构造。
    pub const fn from_unix_ms(unix_ms: u64) -> Self {
        Self { unix_ms }
    }

    /// 当前时刻。系统时钟早于 1970 时退化为 0——这不该发生，但比 panic 好。
    pub fn now() -> Self {
        let unix_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|elapsed| u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX))
            .unwrap_or(0);
        Self { unix_ms }
    }

    /// Unix 毫秒。
    pub const fn unix_ms(self) -> u64 {
        self.unix_ms
    }

    /// `YYYY-MM-DDTHH:MM:SS.sssZ`。
    pub fn to_iso8601(self) -> String {
        let total_seconds = self.unix_ms / 1000;
        let millis = self.unix_ms % 1000;
        let days = total_seconds / 86_400;
        let seconds_of_day = total_seconds % 86_400;
        let (year, month, day) = civil_from_days(days);
        let hour = seconds_of_day / 3600;
        let minute = (seconds_of_day % 3600) / 60;
        let second = seconds_of_day % 60;
        format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{millis:03}Z")
    }
}

impl fmt::Display for Timestamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_iso8601())
    }
}

/// 天数（自 1970-01-01）转公历年月日。Howard Hinnant 的 `civil_from_days`。
fn civil_from_days(days: u64) -> (u64, u64, u64) {
    let days = days + 719_468;
    let era = days / 146_097;
    let day_of_era = days % 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let mp = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { year + 1 } else { year };
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn epoch_is_iso_1970() {
        assert_eq!(
            Timestamp::from_unix_ms(0).to_iso8601(),
            "1970-01-01T00:00:00.000Z"
        );
    }

    #[test]
    fn a_known_second_lands_on_the_documented_date() {
        // 1e9 秒是 2001-09-09T01:46:40Z，业界熟知的一个刻度。
        assert_eq!(
            Timestamp::from_unix_ms(1_000_000_000_000).to_iso8601(),
            "2001-09-09T01:46:40.000Z"
        );
    }

    #[test]
    fn sub_second_and_leap_year_are_kept() {
        // 2024-02-29T12:34:56.789Z
        let stamp = Timestamp::from_unix_ms(1_709_210_096_789);
        assert_eq!(stamp.to_iso8601(), "2024-02-29T12:34:56.789Z");
        assert_eq!(stamp.to_string(), stamp.to_iso8601());
    }

    #[test]
    fn now_is_after_the_epoch_and_round_trips_through_ms() {
        let now = Timestamp::now();
        assert!(now.unix_ms() > 1_600_000_000_000, "本机时钟应晚于 2020");
        assert_eq!(Timestamp::from_unix_ms(now.unix_ms()), now);
    }
}
