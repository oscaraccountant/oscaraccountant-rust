//! A small RFC 3339 parser for `expires_at` (the crate has no time dependency).

use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// `2026-09-27T10:00:00Z`, `2026-09-27T10:00:00.5+02:00`, ... as a [`SystemTime`].
pub(crate) fn parse(value: &str) -> Option<SystemTime> {
    let bytes = value.as_bytes();
    if bytes.len() < 20 || !matches!(bytes[10], b'T' | b't' | b' ') {
        return None;
    }
    let number = |range: std::ops::Range<usize>| -> Option<i64> {
        let text = value.get(range)?;
        text.bytes()
            .all(|byte| byte.is_ascii_digit())
            .then(|| text.parse().ok())?
    };
    let separators = [(4, b'-'), (7, b'-'), (13, b':'), (16, b':')];
    if separators
        .iter()
        .any(|(index, byte)| bytes[*index] != *byte)
    {
        return None;
    }
    let (year, month, day) = (number(0..4)?, number(5..7)?, number(8..10)?);
    let (hour, minute, second) = (number(11..13)?, number(14..16)?, number(17..19)?);
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 60
    {
        return None;
    }
    let mut rest = &value[19..];
    let mut nanos = 0u32;
    if let Some(fraction) = rest.strip_prefix('.') {
        let digits = fraction.bytes().take_while(u8::is_ascii_digit).count();
        if digits == 0 {
            return None;
        }
        let kept = &fraction[..digits.min(9)];
        nanos = kept.parse::<u32>().ok()? * 10u32.pow(9 - kept.len() as u32);
        rest = &fraction[digits..];
    }
    let offset = match rest {
        "Z" | "z" => 0,
        _ => {
            let sign = match rest.as_bytes().first()? {
                b'+' => 1,
                b'-' => -1,
                _ => return None,
            };
            if rest.len() != 6 || rest.as_bytes()[3] != b':' {
                return None;
            }
            let hours: i64 = rest[1..3].parse().ok()?;
            let minutes: i64 = rest[4..6].parse().ok()?;
            sign * (hours * 3600 + minutes * 60)
        }
    };
    let seconds =
        days_from_civil(year, month, day) * 86_400 + hour * 3600 + minute * 60 + second - offset;
    let seconds = u64::try_from(seconds).ok()?;
    Some(UNIX_EPOCH + Duration::new(seconds, nanos))
}

/// The days since 1970-01-01 of a date of the proleptic Gregorian calendar (Howard Hinnant).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let month_index = (month + 9) % 12;
    let day_of_year = (153 * month_index + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seconds(value: &str) -> Option<u64> {
        parse(value).map(|time| time.duration_since(UNIX_EPOCH).unwrap().as_secs())
    }

    #[test]
    fn parses_utc_and_offsets() {
        assert_eq!(seconds("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(seconds("2026-09-27T10:00:00Z"), Some(1_790_503_200));
        assert_eq!(seconds("2026-09-27T12:00:00+02:00"), Some(1_790_503_200));
        assert_eq!(seconds("2026-09-27T08:30:00-01:30"), Some(1_790_503_200));
        assert_eq!(seconds("2000-02-29t00:00:00z"), Some(951_782_400));
        let fraction = parse("2026-09-27T10:00:00.25Z").unwrap();
        assert_eq!(
            fraction.duration_since(UNIX_EPOCH).unwrap().subsec_millis(),
            250
        );
        // Windows keeps 100 ns ticks, so the nanoseconds are exact on Unix only.
        let long = parse("2026-09-27T10:00:00.1234567891Z").unwrap();
        let since = long.duration_since(UNIX_EPOCH).unwrap();
        assert_eq!(since.subsec_micros(), 123_456);
        #[cfg(unix)]
        assert_eq!(since.subsec_nanos(), 123_456_789);
    }

    #[test]
    fn refuses_other_text() {
        for value in [
            "",
            "2026-09-27",
            "2026-09-27X10:00:00Z",
            "2026/09/27T10:00:00Z",
            "2026-13-27T10:00:00Z",
            "2026-09-27T25:00:00Z",
            "2026-09-27T10:00:00",
            "2026-09-27T10:00:00.Z",
            "2026-09-27T10:00:00+0200",
            "2026-09-27T10:00:00+aa:00",
            "2026-09-27T10:00:00+02:bb",
            "2026-09-27T10:00:00*02:00",
            "20a6-09-27T10:00:00Z",
            "1969-12-31T23:59:59Z",
        ] {
            assert_eq!(parse(value), None, "{value}");
        }
    }
}
