//! The environment of one run: the variables, and whether each standard stream is a terminal.
//! The commands read it from here, never from the process, so a test gives its own.

use std::collections::HashMap;
use std::io::IsTerminal;

/// The test switch that makes standard output count as a terminal.
pub const FORCE_TTY: &str = "OSCAR_FORCE_TTY";

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
        let vars: HashMap<String, String> = std::env::vars_os()
            .filter_map(|(name, value)| Some((name.into_string().ok()?, value.into_string().ok()?)))
            .collect();
        let forced = vars.get(FORCE_TTY).is_some_and(|value| value == "1");
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
            vars: vars
                .iter()
                .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
                .collect(),
            ..Self::default()
        }
    }

    /// The value of a variable. An empty value counts as not set.
    pub fn var(&self, name: &str) -> Option<&str> {
        self.vars
            .get(name)
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
    fn reads_the_process() {
        let env = Env::process();
        assert_eq!(
            env.var("PATH").is_some(),
            std::env::var("PATH").is_ok_and(|p| !p.is_empty())
        );
    }
}
