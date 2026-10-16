//! stagit's three time formats (SPEC §4.2), C locale, no external crate.

/// Civil date from days since the Unix epoch (Howard Hinnant's algorithm).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32; // [1, 12]
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// Broken-down UTC time, like `gmtime`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Tm {
    pub year: i64,
    pub month: u32, // 1..=12
    pub day: u32,   // 1..=31
    pub hour: u32,
    pub min: u32,
    pub sec: u32,
    pub wday: u32, // 0 = Sunday
}

/// `gmtime(t)`; `None` when glibc/musl `gmtime` would fail (year out of `int` range).
pub fn gmtime(t: i64) -> Option<Tm> {
    let days = t.div_euclid(86_400);
    let secs = t.rem_euclid(86_400) as u32;
    let (year, month, day) = civil_from_days(days);
    if !(-2_147_481_748..=2_147_485_547).contains(&year) {
        return None;
    }
    // 1970-01-01 was a Thursday (4)
    let wday = (days + 4).rem_euclid(7) as u32;
    Some(Tm {
        year,
        month,
        day,
        hour: secs / 3600,
        min: (secs / 60) % 60,
        sec: secs % 60,
        wday,
    })
}

const WDAY: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
const MON: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// `printtimeshort`: `%Y-%m-%d %H:%M` in UTC (offset ignored).
pub fn time_short(t: i64) -> String {
    match gmtime(t) {
        Some(tm) => format!(
            "{:04}-{:02}-{:02} {:02}:{:02}",
            tm.year, tm.month, tm.day, tm.hour, tm.min
        ),
        None => String::new(),
    }
}

/// `printtime`: `%a, %e %b %Y %H:%M:%S` of the local time plus ` +HHMM`.
pub fn time_long(t: i64, offset_min: i32) -> String {
    match gmtime(t.saturating_add(offset_min as i64 * 60)) {
        Some(tm) => {
            let sign = if offset_min < 0 { '-' } else { '+' };
            let a = offset_min.unsigned_abs();
            format!(
                "{}, {:>2} {} {:04} {:02}:{:02}:{:02} {}{:02}{:02}",
                WDAY[tm.wday as usize],
                tm.day,
                MON[(tm.month - 1) as usize],
                tm.year,
                tm.hour,
                tm.min,
                tm.sec,
                sign,
                a / 60,
                a % 60
            )
        }
        None => String::new(),
    }
}

/// `printtimez`: `%Y-%m-%dT%H:%M:%SZ` in UTC.
pub fn time_z(t: i64) -> String {
    match gmtime(t) {
        Some(tm) => format!(
            "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
            tm.year, tm.month, tm.day, tm.hour, tm.min, tm.sec
        ),
        None => String::new(),
    }
}

/// RFC 7231 HTTP date (`Last-Modified`).
pub fn http_date(t: i64) -> String {
    match gmtime(t) {
        Some(tm) => format!(
            "{}, {:02} {} {:04} {:02}:{:02}:{:02} GMT",
            WDAY[tm.wday as usize],
            tm.day,
            MON[(tm.month - 1) as usize],
            tm.year,
            tm.hour,
            tm.min,
            tm.sec
        ),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_dates() {
        assert_eq!(time_short(0), "1970-01-01 00:00");
        assert_eq!(time_z(951_782_400), "2000-02-29T00:00:00Z");
        // stagit sample: Thu,  9 Nov 2023 17:57:51 -0500
        assert_eq!(
            time_long(1_699_570_671, -300),
            "Thu,  9 Nov 2023 17:57:51 -0500"
        );
        assert_eq!(
            time_long(1_699_570_671, 0),
            "Thu,  9 Nov 2023 22:57:51 +0000"
        );
        assert_eq!(
            time_long(1_699_570_671, 330),
            "Fri, 10 Nov 2023 04:27:51 +0530"
        );
        assert_eq!(time_short(-86_400), "1969-12-31 00:00");
        assert_eq!(http_date(784_111_777), "Sun, 06 Nov 1994 08:49:37 GMT");
    }

    #[test]
    fn weekday_and_leap_years() {
        let tm = gmtime(1_709_164_800).unwrap(); // 2024-02-29
        assert_eq!((tm.year, tm.month, tm.day, tm.wday), (2024, 2, 29, 4));
        assert!(gmtime(i64::MAX).is_none());
    }
}
