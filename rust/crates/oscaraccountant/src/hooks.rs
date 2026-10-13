//! What the tests inject: the environment, the clock, the sleep, the jitter and the UUID source
//! (`clients/spec/README.md`, sections 3.4 and 3.5).

use std::fmt;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use futures::future::BoxFuture;

/// Reads an environment variable. `None` when it is not set.
pub type EnvFn = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;
/// The current time.
pub type ClockFn = Arc<dyn Fn() -> SystemTime + Send + Sync>;
/// Waits for a duration.
pub type SleepFn = Arc<dyn Fn(Duration) -> BoxFuture<'static, ()> + Send + Sync>;
/// A number in `[0, 1)`.
pub type JitterFn = Arc<dyn Fn() -> f64 + Send + Sync>;
/// A new UUID version 4, in lower case with hyphens.
pub type UuidFn = Arc<dyn Fn() -> String + Send + Sync>;

/// The injectable parts of a client. Each one defaults to the real thing.
#[derive(Clone)]
pub(crate) struct Hooks {
    pub(crate) env: EnvFn,
    pub(crate) clock: ClockFn,
    pub(crate) sleep: SleepFn,
    pub(crate) jitter: JitterFn,
    pub(crate) uuid: UuidFn,
}

impl Default for Hooks {
    fn default() -> Self {
        Self {
            env: Arc::new(|name| std::env::var(name).ok()),
            clock: Arc::new(SystemTime::now),
            sleep: Arc::new(|duration| Box::pin(tokio::time::sleep(duration))),
            jitter: Arc::new(random_unit),
            uuid: Arc::new(|| uuid::Uuid::new_v4().to_string()),
        }
    }
}

impl Hooks {
    /// The value of an environment variable, `None` when it is not set or empty.
    pub(crate) fn var(&self, name: &str) -> Option<String> {
        (self.env)(name).filter(|value| !value.is_empty())
    }
}

impl fmt::Debug for Hooks {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Hooks").finish_non_exhaustive()
    }
}

/// A random number in `[0, 1)`, from the random bits of a UUID version 4.
fn random_unit() -> f64 {
    let bits = uuid::Uuid::new_v4().as_u64_pair().0 >> 11;
    bits as f64 / (1u64 << 53) as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn the_defaults_work() {
        let hooks = Hooks::default();
        assert!((hooks.env)("PATH").is_some());
        assert_eq!(hooks.var("OSCAR_TEST_A_VARIABLE_THAT_IS_NOT_SET"), None);
        assert!((hooks.clock)() > SystemTime::UNIX_EPOCH);
        (hooks.sleep)(Duration::from_secs(5)).await;
        for _ in 0..100 {
            let value = (hooks.jitter)();
            assert!((0.0..1.0).contains(&value));
        }
        let id = (hooks.uuid)();
        assert_eq!(id.len(), 36);
        assert_eq!(id, id.to_lowercase());
        assert_eq!(&id[14..15], "4");
        assert_eq!(format!("{hooks:?}"), "Hooks { .. }");
    }

    #[test]
    fn an_empty_variable_is_not_set() {
        let hooks = Hooks {
            env: Arc::new(|_| Some(String::new())),
            ..Hooks::default()
        };
        assert_eq!(hooks.var("X"), None);
    }
}
