//! 本地时间。

use windows::Win32::System::SystemInformation::GetLocalTime;

/// 本地日期时间的各字段。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocalTime {
    /// 年。
    pub year: u16,
    /// 月（1-12）。
    pub month: u16,
    /// 日（1-31）。
    pub day: u16,
    /// 时（0-23）。
    pub hour: u16,
    /// 分（0-59）。
    pub minute: u16,
    /// 秒（0-59）。
    pub second: u16,
}

/// 当前本地时间。
pub fn local_now() -> LocalTime {
    // SAFETY: 无前置条件，返回按值的结构体。
    let st = unsafe { GetLocalTime() };
    LocalTime {
        year: st.wYear,
        month: st.wMonth,
        day: st.wDay,
        hour: st.wHour,
        minute: st.wMinute,
        second: st.wSecond,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn now_is_plausible() {
        let t = local_now();
        assert!(t.year >= 2024);
        assert!((1..=12).contains(&t.month));
        assert!((1..=31).contains(&t.day));
        assert!(t.hour < 24 && t.minute < 60 && t.second < 61);
    }
}
