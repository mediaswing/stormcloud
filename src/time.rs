//! UTC dates and times as Graph, Exchange Online, the servers' reports and the log write them,
//! without a date library: seconds since 1970 to and from `YYYY-MM-DDTHH:MM:SSZ`.

use std::time::{SystemTime, UNIX_EPOCH};

/// The civil date of a day counted from 1970-01-01 (Howard Hinnant's algorithm).
pub fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

/// The day counted from 1970-01-01 of a civil date; the inverse of [`civil_from_days`].
pub fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * ((m + 9) % 12) + 2) / 5 + d - 1;
    era * 146_097 + yoe * 365 + yoe / 4 - yoe / 100 + doy - 719_468
}

/// `secs` since 1970 as `YYYY-MM-DDTHH:MM:SSZ`.
pub fn iso_utc(secs: i64) -> String {
    let (y, m, d) = civil_from_days(secs.div_euclid(86_400));
    let rem = secs.rem_euclid(86_400);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", rem / 3600, rem % 3600 / 60, rem % 60)
}

/// Seconds since 1970 of a time such as `2026-10-19T08:00:00Z`, read as UTC, ignoring any
/// fraction of a second.
pub fn parse_iso(t: &str) -> Option<i64> {
    let num = |r: std::ops::Range<usize>| t.get(r)?.parse::<i64>().ok();
    let (y, m, d) = (num(0..4)?, num(5..7)?, num(8..10)?);
    let (hh, mm, ss) = (num(11..13)?, num(14..16)?, num(17..19)?);
    Some(days_from_civil(y, m, d) * 86_400 + hh * 3600 + mm * 60 + ss)
}

/// The time now, as seconds and milliseconds since 1970.
pub fn now() -> (i64, u32) {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    (now.as_secs() as i64, now.subsec_millis())
}

pub fn now_iso() -> String {
    iso_utc(now().0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_dates() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(19_723), (2024, 1, 1));
        assert_eq!(civil_from_days(19_782), (2024, 2, 29));
        for day in [-1, 0, 59, 19_782, 30_000] {
            let (y, m, d) = civil_from_days(day);
            assert_eq!(days_from_civil(y, m.into(), d.into()), day);
        }
    }

    #[test]
    fn formats_and_reads_iso_times() {
        assert_eq!(iso_utc(1_760_000_000), "2025-10-09T08:53:20Z");
        assert_eq!(iso_utc(-1), "1969-12-31T23:59:59Z");
        assert_eq!(parse_iso("1970-01-02T00:00:01Z"), Some(86_401));
        assert_eq!(parse_iso("2000-03-01T00:00:00Z"), Some(951_868_800));
        assert_eq!(parse_iso("2025-10-09T08:53:20.1234567Z"), Some(1_760_000_000));
        assert_eq!(parse_iso("2025-10-09"), None);
    }
}
