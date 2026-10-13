//! When to retry, and how long to wait (`clients/spec/README.md`, section 3.4).

use std::time::{Duration, SystemTime};

/// The longest `Retry-After` the client waits for. A longer one ends the retries.
pub(crate) const MAX_RETRY_AFTER: Duration = Duration::from_secs(60);

/// A response status that may be retried. A 409 counts only with the code
/// `idempotency_in_progress`.
pub(crate) fn is_retriable_status(status: u16, code: &str) -> bool {
    matches!(status, 408 | 429 | 500 | 502 | 503 | 504)
        || (status == 409 && code == "idempotency_in_progress")
}

/// The delay before retry `retry` (1, 2, ...): `min(8, 0.5 * 2^(n-1)) * (1 - 0.25 * jitter)`
/// seconds.
pub(crate) fn backoff(retry: u32, jitter: f64) -> Duration {
    let exponent = retry.saturating_sub(1).min(16) as i32;
    let base = (0.5 * 2f64.powi(exponent)).min(8.0);
    Duration::from_secs_f64(base * (1.0 - 0.25 * jitter.clamp(0.0, 1.0)))
}

/// What a `Retry-After` header says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RetryAfter {
    /// Wait this long, then retry.
    Wait(Duration),
    /// The server asks for more than 60 seconds: do not retry.
    TooLong,
}

/// Parse a `Retry-After` value: whole seconds, or an HTTP date against `now`. `None` when the value
/// is neither.
pub(crate) fn parse_retry_after(value: &str, now: SystemTime) -> Option<RetryAfter> {
    let value = value.trim();
    let wait = if let Ok(seconds) = value.parse::<u64>() {
        Duration::from_secs(seconds)
    } else {
        let date = httpdate::parse_http_date(value).ok()?;
        date.duration_since(now).unwrap_or(Duration::ZERO)
    };
    Some(if wait > MAX_RETRY_AFTER {
        RetryAfter::TooLong
    } else {
        RetryAfter::Wait(wait)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retriable_statuses() {
        for status in [408, 429, 500, 502, 503, 504] {
            assert!(is_retriable_status(status, "unknown"), "{status}");
        }
        assert!(is_retriable_status(409, "idempotency_in_progress"));
        for status in [400, 401, 403, 404, 422, 501, 505] {
            assert!(!is_retriable_status(status, "unknown"), "{status}");
        }
        assert!(!is_retriable_status(409, "idempotency_conflict"));
    }

    #[test]
    fn backoff_grows_and_caps() {
        assert_eq!(backoff(1, 0.0), Duration::from_millis(500));
        assert_eq!(backoff(2, 0.0), Duration::from_secs(1));
        assert_eq!(backoff(3, 0.0), Duration::from_secs(2));
        assert_eq!(backoff(5, 0.0), Duration::from_secs(8));
        assert_eq!(backoff(40, 0.0), Duration::from_secs(8));
        assert_eq!(
            backoff(1, 0.5),
            Duration::from_millis(437) + Duration::from_micros(500)
        );
        assert_eq!(backoff(0, 0.0), Duration::from_millis(500));
        assert_eq!(backoff(1, 7.0), Duration::from_millis(375));
    }

    #[test]
    fn retry_after_in_seconds_and_as_a_date() {
        let now = httpdate::parse_http_date("Sun, 27 Sep 2026 10:00:00 GMT").unwrap();
        assert_eq!(
            parse_retry_after("3", now),
            Some(RetryAfter::Wait(Duration::from_secs(3)))
        );
        assert_eq!(
            parse_retry_after(" 60 ", now),
            Some(RetryAfter::Wait(Duration::from_secs(60)))
        );
        assert_eq!(parse_retry_after("61", now), Some(RetryAfter::TooLong));
        assert_eq!(
            parse_retry_after("Sun, 27 Sep 2026 10:00:05 GMT", now),
            Some(RetryAfter::Wait(Duration::from_secs(5)))
        );
        assert_eq!(
            parse_retry_after("Sun, 27 Sep 2026 09:00:00 GMT", now),
            Some(RetryAfter::Wait(Duration::ZERO))
        );
        assert_eq!(
            parse_retry_after("Sun, 27 Sep 2026 10:05:00 GMT", now),
            Some(RetryAfter::TooLong)
        );
        assert_eq!(parse_retry_after("soon", now), None);
        assert_eq!(parse_retry_after("-1", now), None);
    }
}
