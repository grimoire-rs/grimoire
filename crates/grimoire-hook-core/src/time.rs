// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! UTC timestamps without a date-time dependency.

use std::time::{SystemTime, UNIX_EPOCH};

/// `%Y-%m-%dT%H:%M:%SZ` in UTC, second precision.
///
/// A pre-epoch instant floors to the earlier second, as chrono does. Valid for
/// years 0000-9999 (the audit clock is "now"; the range is far wider than any
/// real timestamp); outside it the year field is not RFC 3339.
pub fn rfc3339_utc(t: SystemTime) -> String {
    let secs: i64 = match t.duration_since(UNIX_EPOCH) {
        Ok(d) => d.as_secs() as i64,
        Err(e) => {
            let d = e.duration();
            -(d.as_secs() as i64) - i64::from(d.subsec_nanos() > 0)
        }
    };
    let (y, m, d) = civil_from_days(secs.div_euclid(86_400));
    let rem = secs.rem_euclid(86_400);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

/// Days since 1970-01-01 to a proleptic Gregorian (year, month, day)
/// (Howard Hinnant's `civil_from_days`).
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

#[cfg(test)]
mod tests {
    use super::rfc3339_utc;
    use std::time::{Duration, UNIX_EPOCH};

    fn at(secs: u64) -> String {
        rfc3339_utc(UNIX_EPOCH + Duration::from_secs(secs))
    }

    #[test]
    fn the_epoch_and_known_instants() {
        assert_eq!(at(0), "1970-01-01T00:00:00Z");
        assert_eq!(at(951_782_400), "2000-02-29T00:00:00Z", "2000 is a leap year");
        assert_eq!(at(1_709_208_000), "2024-02-29T12:00:00Z");
        assert_eq!(at(4_107_542_399), "2100-02-28T23:59:59Z", "2100 is not a leap year");
        assert_eq!(at(4_107_542_400), "2100-03-01T00:00:00Z");
        assert_eq!(at(1_999_999_999), "2033-05-18T03:33:19Z");
    }

    #[test]
    fn sub_second_precision_truncates() {
        assert_eq!(
            rfc3339_utc(UNIX_EPOCH + Duration::new(1, 999_999_999)),
            "1970-01-01T00:00:01Z"
        );
    }

    #[test]
    fn before_the_epoch_floors_to_the_earlier_second() {
        assert_eq!(rfc3339_utc(UNIX_EPOCH - Duration::from_secs(1)), "1969-12-31T23:59:59Z");
        assert_eq!(
            rfc3339_utc(UNIX_EPOCH - Duration::from_millis(1)),
            "1969-12-31T23:59:59Z"
        );
    }
}
