//! RFC 3339 dates in UTC, for `expires_at` in the store and in `oscar auth token --json`. The CLI
//! needs no time crate for this.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

const DAY: i64 = 86_400;

/// `2026-09-27T10:00:00Z`: whole seconds, UTC.
pub fn format(time: SystemTime) -> String {
    let seconds = match time.duration_since(UNIX_EPOCH) {
        Ok(since) => i64::try_from(since.as_secs()).unwrap_or(i64::MAX),
        Err(before) => -i64::try_from(before.duration().as_secs()).unwrap_or(i64::MAX),
    };
    let (days, rest) = (seconds.div_euclid(DAY), seconds.rem_euclid(DAY));
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rest / 3600,
        rest % 3600 / 60,
        rest % 60
    )
}

/// Parse an RFC 3339 date and time: `2026-09-27T10:00:00Z`, with optional fractions of a second
/// and an offset such as `+03:00`. `None` when it is not one.
pub fn parse(text: &str) -> Option<SystemTime> {
    let bytes = text.as_bytes();
    if bytes.len() < 20 || !matches!(bytes[10], b'T' | b't' | b' ') {
        return None;
    }
    if bytes[4] != b'-' || bytes[7] != b'-' || bytes[13] != b':' || bytes[16] != b':' {
        return None;
    }
    let year = number(&text[0..4])?;
    let month = number(&text[5..7])?;
    let day = number(&text[8..10])?;
    let hour = number(&text[11..13])?;
    let minute = number(&text[14..16])?;
    let second = number(&text[17..19])?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) || hour > 23 || minute > 59 {
        return None;
    }
    if second > 60 {
        return None;
    }
    let mut rest = &text[19..];
    if let Some(fraction) = rest.strip_prefix('.') {
        let digits = fraction.bytes().take_while(u8::is_ascii_digit).count();
        if digits == 0 {
            return None;
        }
        rest = &fraction[digits..];
    }
    let offset = match rest {
        "Z" | "z" => 0,
        _ => {
            let (sign, value) = match rest.as_bytes().first()? {
                b'+' => (1, &rest[1..]),
                b'-' => (-1, &rest[1..]),
                _ => return None,
            };
            if value.len() != 5 || value.as_bytes()[2] != b':' {
                return None;
            }
            sign * (number(&value[0..2])? * 3600 + number(&value[3..5])? * 60)
        }
    };
    let seconds =
        days_from_civil(year, month, day) * DAY + hour * 3600 + minute * 60 + second - offset;
    Some(match u64::try_from(seconds) {
        Ok(after) => UNIX_EPOCH + Duration::from_secs(after),
        Err(_) => UNIX_EPOCH - Duration::from_secs(seconds.unsigned_abs()),
    })
}

fn number(text: &str) -> Option<i64> {
    if text.bytes().all(|byte| byte.is_ascii_digit()) {
        text.parse().ok()
    } else {
        None
    }
}

/// Days since 1970-01-01 of a date of the proleptic Gregorian calendar (H. Hinnant's algorithm).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let day_of_year = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// The date of a number of days since 1970-01-01.
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let days = days + 719_468;
    let era = days.div_euclid(146_097);
    let day_of_era = days - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(seconds: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(seconds)
    }

    #[test]
    fn formats_utc() {
        assert_eq!(format(UNIX_EPOCH), "1970-01-01T00:00:00Z");
        assert_eq!(format(at(1_790_503_200)), "2026-09-27T10:00:00Z");
        assert_eq!(format(at(951_782_400)), "2000-02-29T00:00:00Z");
        assert_eq!(
            format(UNIX_EPOCH - Duration::from_secs(1)),
            "1969-12-31T23:59:59Z"
        );
    }

    #[test]
    fn parses_what_it_formats() {
        for seconds in [0, 1, 951_782_400, 1_790_503_200, 4_102_444_800] {
            assert_eq!(parse(&format(at(seconds))), Some(at(seconds)));
        }
    }

    #[test]
    fn parses_fractions_and_offsets() {
        let expected = Some(at(1_790_503_200));
        assert_eq!(parse("2026-09-27T10:00:00.123456Z"), expected);
        assert_eq!(parse("2026-09-27T13:00:00+03:00"), expected);
        assert_eq!(parse("2026-09-27T08:30:00-01:30"), expected);
        assert_eq!(parse("2026-09-27 10:00:00z"), expected);
        assert_eq!(
            parse("1969-12-31T23:59:59Z"),
            Some(UNIX_EPOCH - Duration::from_secs(1))
        );
    }

    #[test]
    fn refuses_what_is_not_a_date() {
        for text in [
            "",
            "2026-09-27",
            "2026-09-27X10:00:00Z",
            "2026/09/27T10:00:00Z",
            "2026-13-27T10:00:00Z",
            "2026-09-32T10:00:00Z",
            "2026-09-27T24:00:00Z",
            "2026-09-27T10:60:00Z",
            "2026-09-27T10:00:61Z",
            "2026-09-27T10:00:00",
            "2026-09-27T10:00:00.Z",
            "2026-09-27T10:00:00+0300",
            "2026-09-27T10:00:00*03:00",
            "2026-09-27T1a:00:00Z",
            "abcd-09-27T10:00:00Z",
        ] {
            assert_eq!(parse(text), None, "{text}");
        }
    }
}
