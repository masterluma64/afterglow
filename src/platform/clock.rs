//! Today's date in local time, for the per-day watching stats.

#[repr(C)]
#[derive(Default)]
struct SystemTime {
    year: u16,
    month: u16,
    day_of_week: u16,
    day: u16,
    hour: u16,
    minute: u16,
    second: u16,
    milliseconds: u16,
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetLocalTime(time: *mut SystemTime);
}

/// Today, as days since 1970-01-01 (see [`crate::stats::day_from_date`]).
pub fn today() -> i64 {
    let mut now = SystemTime::default();
    unsafe { GetLocalTime(&mut now) };
    crate::stats::day_from_date(i64::from(now.year), u32::from(now.month), u32::from(now.day))
}
