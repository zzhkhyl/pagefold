//! 本地时区的时间格式化。
//!
//! 换算分两步：先把 Unix 秒拆成 UTC 的民用日期字段（纯整数运算，见 [`fm_core::util`]），
//! 再把 UTC 字段交给 `SystemTimeToTzSpecificLocalTime` 做时区与夏令时换算。
//!
//! 这样既不需要 `chrono` / `time` 之类的依赖，也不会出现「把 UTC 直接当本地时间显示」
//! 这种在详情视图里一眼就能看出来的偏差。

use std::time::{SystemTime, UNIX_EPOCH};

use fm_core::util::{civil_from_unix_seconds, format_date_time};
use windows::Win32::Foundation::SYSTEMTIME;
use windows::Win32::System::Time::SystemTimeToTzSpecificLocalTime;

/// 把 `SystemTime` 转成本地时区的 `YYYY-MM-DD HH:MM`。
///
/// 转换失败（时间超出 `SYSTEMTIME` 的可表示范围，例如极端损坏的文件时间戳）
/// 返回 `None`，由调用方决定退化显示。
pub fn format_local(time: SystemTime) -> Option<String> {
    let seconds = match time.duration_since(UNIX_EPOCH) {
        Ok(duration) => duration.as_secs() as i64,
        Err(error) => -(error.duration().as_secs() as i64),
    };
    let (year, month, day, hour, minute, second) = civil_from_unix_seconds(seconds);

    let utc = SYSTEMTIME {
        wYear: u16::try_from(year).ok()?,
        wMonth: month as u16,
        // 星期几由系统换算后自行填充，这里无需（也不应）预先计算。
        wDayOfWeek: 0,
        wDay: day as u16,
        wHour: hour as u16,
        wMinute: minute as u16,
        wSecond: second as u16,
        wMilliseconds: 0,
    };

    let mut local = SYSTEMTIME::default();
    // 第一个参数传 None 表示使用系统当前时区，并自动应用该时刻的夏令时规则。
    unsafe { SystemTimeToTzSpecificLocalTime(None, &utc, &mut local).ok()? };

    Some(format_date_time(
        i64::from(local.wYear),
        u32::from(local.wMonth),
        u32::from(local.wDay),
        u32::from(local.wHour),
        u32::from(local.wMinute),
    ))
}
