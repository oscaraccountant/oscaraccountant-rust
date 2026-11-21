//! The environment of one run: the variables, and whether each standard stream is a terminal.
//! The commands read it from here, never from the process, so a test gives its own.

use std::collections::HashMap;
use std::io::IsTerminal;

/// The test switch that makes standard output count as a terminal.
pub const FORCE_TTY: &str = "OSCAR_FORCE_TTY";

/// The key of a variable name in the map. Windows compares variable names without case (`Path`
/// answers `PATH`), so there the key is the name in upper case; elsewhere it is the name as it
/// is. Only ASCII letters are folded: every name the CLI reads is ASCII, and Windows never folds
/// a non-ASCII letter onto an ASCII one, so this agrees with Windows for every lookup. A Unicode
/// fold would merge names that Windows keeps apart (`ſ` and `s`, `ı` and `i`).
fn key(name: &str, windows: bool) -> String {
    if windows {
        name.to_ascii_uppercase()
    } else {
        name.to_owned()
    }
}

/// The variables, by key. When two names share a key (they differ only by case, on Windows),
/// the first one wins, so the result does not depend on the order of a hash map.
fn collect<I: IntoIterator<Item = (String, String)>>(vars: I) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for (name, value) in vars {
        map.entry(key(&name, cfg!(windows))).or_insert(value);
    }
    map
}

/// The environment of one run.
#[derive(Debug, Clone, Default)]
pub struct Env {
    vars: HashMap<String, String>,
    /// Standard input is a terminal.
    pub stdin_tty: bool,
    /// Standard output is a terminal.
    pub stdout_tty: bool,
    /// Standard error is a terminal.
    pub stderr_tty: bool,
}

impl Env {
    /// The environment of this process. `OSCAR_FORCE_TTY=1` makes standard output count as a
    /// terminal, so a test can check the tables.
    pub fn process() -> Self {
        let vars = collect(std::env::vars_os().filter_map(|(name, value)| {
            Some((name.into_string().ok()?, value.into_string().ok()?))
        }));
        let forced = vars
            .get(&key(FORCE_TTY, cfg!(windows)))
            .is_some_and(|value| value == "1");
        Self {
            stdin_tty: std::io::stdin().is_terminal(),
            stdout_tty: forced || std::io::stdout().is_terminal(),
            stderr_tty: std::io::stderr().is_terminal(),
            vars,
        }
    }

    /// An environment with these variables and no terminal.
    #[cfg(test)]
    pub fn with_vars(vars: &[(&str, &str)]) -> Self {
        Self {
            vars: collect(
                vars.iter()
                    .map(|(name, value)| ((*name).to_owned(), (*value).to_owned())),
            ),
            ..Self::default()
        }
    }

    /// The value of a variable. An empty value counts as not set. On Windows the case of the name
    /// does not count.
    pub fn var(&self, name: &str) -> Option<&str> {
        self.vars
            .get(&key(name, cfg!(windows)))
            .map(String::as_str)
            .filter(|value| !value.is_empty())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_value_is_not_set() {
        let env = Env::with_vars(&[("A", "1"), ("B", "")]);
        assert_eq!(env.var("A"), Some("1"));
        assert_eq!(env.var("B"), None);
        assert_eq!(env.var("C"), None);
    }

    #[test]
    fn a_key_folds_case_on_windows_only() {
        assert_eq!(key("Path", true), "PATH");
        assert_eq!(key("oscar_api_key", true), "OSCAR_API_KEY");
        // A non-ASCII letter stays as it is, so it never lands on an ASCII name.
        assert_eq!(key("Öö", true), "Öö");
        assert_ne!(key("OſCAR_API_KEY", true), "OSCAR_API_KEY");
        assert_ne!(key("oscar_apı_key", true), "OSCAR_API_KEY");
        assert_eq!(key("Path", false), "Path");
    }

    #[test]
    #[cfg(windows)]
    fn a_name_ignores_case_on_windows() {
        let env = Env::with_vars(&[("Path", "C:\\bin")]);
        assert_eq!(env.var("PATH"), Some("C:\\bin"));
        assert_eq!(env.var("path"), Some("C:\\bin"));
        assert_eq!(env.var("Path"), Some("C:\\bin"));
        // Two names that differ only by case: the first one wins.
        let env = Env::with_vars(&[("Path", "first"), ("PATH", "second")]);
        assert_eq!(env.var("PATH"), Some("first"));
    }

    #[test]
    #[cfg(not(windows))]
    fn a_name_keeps_its_case_elsewhere() {
        let env = Env::with_vars(&[("Path", "/bin")]);
        assert_eq!(env.var("Path"), Some("/bin"));
        assert_eq!(env.var("PATH"), None);
        let env = Env::with_vars(&[("Path", "first"), ("PATH", "second")]);
        assert_eq!(env.var("Path"), Some("first"));
        assert_eq!(env.var("PATH"), Some("second"));
    }

    #[test]
    fn reads_the_process() {
        let env = Env::process();
        assert_eq!(
            env.var("PATH").is_some(),
            std::env::var("PATH").is_ok_and(|p| !p.is_empty())
        );
        // The process and the standard library agree on another case too: found on Windows,
        // a different variable elsewhere.
        assert_eq!(
            env.var("path").is_some(),
            std::env::var("path").is_ok_and(|p| !p.is_empty())
        );
    }
}
