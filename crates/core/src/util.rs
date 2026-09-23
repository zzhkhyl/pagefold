//! 纯格式化与时间换算工具。
//!
//! 放在 `fm-core` 而不是 UI 层，是因为状态栏与详情视图都要用同一套格式；
//! 这类函数没有依赖、易于测试，单独成模块比在 UI 里各写一份更稳妥。

use std::time::{SystemTime, UNIX_EPOCH};

/// 把字节数格式化为人类可读的字符串。
///
/// 约定（与资源管理器一致）：
/// - 小于 1 KB 时按「N 字节」原样显示，避免出现「0.0 KB」这种无意义读数；
/// - 其余按 1024 进制换算，保留一位小数。
pub fn format_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["KB", "MB", "GB", "TB"];

    if bytes < 1024 {
        return format!("{bytes} 字节");
    }

    let mut value = bytes as f64 / 1024.0;
    let mut unit = 0usize;
    // 逐级换算到最后一个单位或数值小于 1024 为止。
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }

    format!("{value:.1} {}", UNITS[unit])
}

/// 把 Unix 秒拆成民用日期时间字段：`(年, 月, 日, 时, 分, 秒)`。
///
/// 时间取 UTC。之所以自己算而不引入 `chrono`/`time`：
/// 只需要一个展示用的日期，而这套换算是纯整数运算，既无依赖也便于单测。
/// 本地时区换算由平台层在此基础上完成（见 `fm_platform::format_time_local`）。
///
/// 算法取自 Howard Hinnant 的 `civil_from_days`，对负数天数（1970 年之前）同样成立。
pub fn civil_from_unix_seconds(seconds: i64) -> (i64, u32, u32, u32, u32, u32) {
    let days = seconds.div_euclid(86_400);
    let second_of_day = seconds.rem_euclid(86_400);

    // 把 1970-01-01 起算的天数平移到 0000-03-01 起算（把闰年放在年末，便于取模）。
    let shifted = days + 719_468;
    let era = if shifted >= 0 { shifted } else { shifted - 146_096 } / 146_097;
    let day_of_era = (shifted - era * 146_097) as u64;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era as i64 + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * month_index + 2) / 5 + 1) as u32;
    // 上面的月份以 3 月为 0，这里折回 1..=12，并补齐跨年。
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    } as u32;
    let year = year + i64::from(month <= 2);

    (
        year,
        month,
        day,
        (second_of_day / 3_600) as u32,
        ((second_of_day % 3_600) / 60) as u32,
        (second_of_day % 60) as u32,
    )
}

/// 把日期时间字段格式化为 `YYYY-MM-DD HH:MM`。
pub fn format_date_time(year: i64, month: u32, day: u32, hour: u32, minute: u32) -> String {
    format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}")
}

/// 把 `SystemTime` 格式化为 **UTC** 的 `YYYY-MM-DD HH:MM`。
///
/// 仅作为平台层无法提供本地时区时的退化路径；`fm_platform::format_time_local`
/// 会给出正确的本地时间。
pub fn format_system_time_utc(time: SystemTime) -> String {
    let seconds = match time.duration_since(UNIX_EPOCH) {
        Ok(duration) => duration.as_secs() as i64,
        // 1970 年之前的文件（老软盘、归档解包出的时间戳）落在负半轴。
        Err(error) => -(error.duration().as_secs() as i64),
    };
    let (year, month, day, hour, minute, _second) = civil_from_unix_seconds(seconds);
    format_date_time(year, month, day, hour, minute)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_values_are_reported_in_bytes() {
        assert_eq!(format_size(0), "0 字节");
        assert_eq!(format_size(1), "1 字节");
        assert_eq!(format_size(1023), "1023 字节");
    }

    #[test]
    fn kibibyte_boundary_flips_to_kb() {
        assert_eq!(format_size(1024), "1.0 KB");
        assert_eq!(format_size(1536), "1.5 KB");
    }

    #[test]
    fn larger_units_are_selected_automatically() {
        assert_eq!(format_size(1024 * 1024), "1.0 MB");
        assert_eq!(format_size(1024 * 1024 * 1024), "1.0 GB");
        assert_eq!(format_size(1024_u64.pow(4)), "1.0 TB");
    }

    #[test]
    fn values_beyond_the_unit_table_stay_in_the_largest_unit() {
        // 1 PB 远超单位表上界，应停在 TB 而不是越界。
        assert_eq!(format_size(1024_u64.pow(5)), "1024.0 TB");
    }

    #[test]
    fn unix_epoch_is_1970_01_01() {
        assert_eq!(civil_from_unix_seconds(0), (1970, 1, 1, 0, 0, 0));
    }

    #[test]
    fn known_timestamps_convert_correctly() {
        // 1_000_000_000 = 2001-09-09T01:46:40Z
        assert_eq!(civil_from_unix_seconds(1_000_000_000), (2001, 9, 9, 1, 46, 40));
        // 闰日：2024-02-29T12:00:00Z
        assert_eq!(civil_from_unix_seconds(1_709_208_000), (2024, 2, 29, 12, 0, 0));
    }

    #[test]
    fn times_before_the_epoch_go_negative_without_panicking() {
        // 1969-12-31T23:59:59Z
        assert_eq!(civil_from_unix_seconds(-1), (1969, 12, 31, 23, 59, 59));
    }

    #[test]
    fn formatting_pads_single_digit_fields() {
        assert_eq!(format_date_time(2026, 9, 1, 8, 5), "2026-09-01 08:05");
    }

    #[test]
    fn utc_formatting_of_the_epoch_is_stable() {
        assert_eq!(format_system_time_utc(UNIX_EPOCH), "1970-01-01 00:00");
    }
}
