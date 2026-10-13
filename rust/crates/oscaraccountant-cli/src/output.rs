//! What the CLI prints: a table on a terminal, JSON when standard output is not a terminal or
//! with `--json`. Messages for a person go to standard error; data goes to standard output.

use std::io::{self, Write};
#[cfg(test)]
use std::sync::{Arc, Mutex, PoisonError};
use std::time::SystemTime;

use serde_json::Value;
use supports_hyperlinks::Stream;

use crate::env::Env;
use crate::error::CliError;
use crate::render::{self, theme::Role, theme::Theme};

/// The test switch that fixes the time of the run (RFC 3339), for relative times in a test.
pub const NOW: &str = "OSCAR_NOW";

/// The two output streams of a run, and how to write to them.
pub struct Output {
    out: Box<dyn Write + Send>,
    err: Box<dyn Write + Send>,
    /// Standard output is a terminal: tables, unless `--json`.
    pub stdout_tty: bool,
    /// Colours on standard error.
    pub color: bool,
    /// The theme of standard output: colours, symbols and links of the tables and cards.
    pub theme: Theme,
    /// The width of standard output: the terminal, or `COLUMNS`. `None`: no limit.
    pub width: Option<usize>,
    /// Now, for relative times and overdue dates.
    pub now: SystemTime,
    /// `OSCAR_DEBUG=1`: an error prints the whole body of the answer.
    pub debug: bool,
}

impl Output {
    /// The standard streams of the process. Sets the theme of standard error (`theme::init`).
    pub fn process(env: &Env) -> Self {
        let stderr_theme = Theme::for_stream(env, env.stderr_tty, Stream::Stderr);
        render::theme::init(stderr_theme);
        let theme = Theme::for_stream(env, env.stdout_tty, Stream::Stdout);
        let out: Box<dyn Write + Send> = if theme.color {
            // On an old Windows console, anstream turns the ANSI codes into console calls.
            Box::new(anstream::AutoStream::new(
                io::stdout(),
                anstream::ColorChoice::Always,
            ))
        } else {
            Box::new(io::stdout())
        };
        let err: Box<dyn Write + Send> = if stderr_theme.color {
            Box::new(anstream::AutoStream::new(
                io::stderr(),
                anstream::ColorChoice::Always,
            ))
        } else {
            Box::new(io::stderr())
        };
        Self {
            out,
            err,
            stdout_tty: env.stdout_tty,
            color: stderr_theme.color,
            theme,
            width: width(env),
            now: now(env),
            debug: env
                .var(crate::error::DEBUG_VARIABLE)
                .is_some_and(|value| value != "0"),
        }
    }

    /// Two buffers, for a test.
    #[cfg(test)]
    pub fn buffers() -> (Self, Buffer, Buffer) {
        let (out, err) = (Buffer::default(), Buffer::default());
        let output = Self {
            out: Box::new(out.clone()),
            err: Box::new(err.clone()),
            stdout_tty: false,
            color: false,
            theme: Theme::PLAIN,
            width: None,
            now: SystemTime::now(),
            debug: false,
        };
        (output, out, err)
    }

    /// Standard output.
    pub fn stdout(&mut self) -> &mut dyn Write {
        &mut self.out
    }

    /// One line of data on standard output.
    pub fn line(&mut self, text: &str) {
        // A closed pipe (`oscar ... | head`) is not an error of the command.
        let _ = writeln!(self.out, "{text}");
    }

    /// One line for the person on standard error.
    pub fn note(&mut self, text: &str) {
        let _ = writeln!(self.err, "{text}");
    }

    /// A warning on standard error.
    pub fn warn(&mut self, text: &str) {
        let label = self.paint("warning:", Role::Warn);
        let _ = writeln!(self.err, "{label} {text}");
    }

    /// An error on standard error, with its details.
    pub fn error(&mut self, error: &CliError) {
        let label = self.paint("error:", Role::Bad);
        let _ = writeln!(self.err, "{label} {}", error.message);
        for detail in &error.details {
            let _ = writeln!(self.err, "  {detail}");
        }
        if self.debug
            && let Some(body) = error.body.as_deref().filter(|body| !body.is_empty())
        {
            let _ = writeln!(self.err, "{}", body.trim_end());
        }
    }

    fn paint(&self, text: &str, role: Role) -> String {
        Theme {
            color: self.color,
            ..Theme::PLAIN
        }
        .paint_bold(role, text)
    }

    /// A JSON value on standard output: indented on a terminal, one line otherwise.
    pub fn json(&mut self, value: &Value) {
        let text = if self.stdout_tty {
            serde_json::to_string_pretty(value)
        } else {
            serde_json::to_string(value)
        };
        self.line(&text.unwrap_or_default());
    }

    /// A value as JSON (`json`), or for a person: a table or a card from a view guessed from
    /// the value. The commands of the API use `render::show` with their generated view.
    pub fn value(&mut self, value: &Value, json: bool) {
        if json {
            self.json(value);
        } else {
            let options = render::Options {
                format: Some(render::Format::Table),
                ..render::Options::default()
            };
            if let Err(error) =
                render::show(self, &options, value, None, &render::ListInfo::default())
            {
                self.error(&error);
            }
        }
    }

    /// Flush both streams.
    pub fn flush(&mut self) {
        let _ = self.out.flush();
        let _ = self.err.flush();
    }
}

/// The width of standard output: `COLUMNS` when it is a number, else the terminal's when standard
/// output is one.
fn width(env: &Env) -> Option<usize> {
    if let Some(columns) = env
        .var("COLUMNS")
        .and_then(|value| value.parse::<usize>().ok())
    {
        return Some(columns.max(1));
    }
    if !env.stdout_tty {
        return None;
    }
    terminal_size::terminal_size_of(io::stdout())
        .map(|(terminal_size::Width(width), _)| usize::from(width))
}

/// Now, or the time of `OSCAR_NOW`.
fn now(env: &Env) -> SystemTime {
    env.var(NOW)
        .and_then(crate::time::parse)
        .unwrap_or_else(SystemTime::now)
}

/// A shared buffer that a test reads after the run.
#[cfg(test)]
#[derive(Debug, Clone, Default)]
pub struct Buffer(Arc<Mutex<Vec<u8>>>);

#[cfg(test)]
impl Buffer {
    /// Everything written, as text.
    pub fn text(&self) -> String {
        let bytes = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        String::from_utf8_lossy(&bytes).into_owned()
    }
}

#[cfg(test)]
impl Write for Buffer {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn a_page_is_a_table_of_items() {
        let (mut output, out, err) = Output::buffers();
        output.value(
            &json!({
                "items": [
                    {"id": "a", "state": "issued", "lines": [1], "total": 10, "paid": true, "note": null},
                    {"id": "b", "state": "draft", "extra": "x"},
                ],
                "next_cursor": null,
            }),
            false,
        );
        let text = out.text();
        let header = text.lines().next().unwrap();
        for column in ["ID", "STATE", "TOTAL", "PAID", "EXTRA"] {
            assert!(header.contains(column), "{column} in {header}");
        }
        assert!(!header.contains("LINES"));
        assert!(text.contains("issued") && text.contains("draft") && text.contains("yes"));
        output.value(&json!({"items": [], "next_cursor": null}), false);
        assert_eq!(err.text(), "No items yet.\n");
    }

    #[test]
    fn writes_json_tables_and_messages() {
        let (mut output, out, err) = Output::buffers();
        output.value(&json!({"items": [{"id": "a"}], "next_cursor": "c1"}), true);
        assert_eq!(
            out.text(),
            "{\"items\":[{\"id\":\"a\"}],\"next_cursor\":\"c1\"}\n"
        );
        output.value(&json!({"items": [{"id": "a"}], "next_cursor": "c1"}), false);
        assert!(err.text().contains("--cursor c1"));
        output.stdout_tty = true;
        output.json(&json!({"a": 1}));
        assert!(out.text().ends_with("{\n  \"a\": 1\n}\n"));
        output.warn("careful");
        output.error(&CliError {
            details: vec!["field: bad".into()],
            ..CliError::usage("wrong")
        });
        output.flush();
        assert!(err.text().contains("warning: careful\n"));
        assert!(err.text().contains("error: wrong\n  field: bad\n"));
        writeln!(output.stdout(), "raw").unwrap();
        assert!(out.text().ends_with("raw\n"));
        output.value(&json!({"a": 1}), false);
        assert!(out.text().ends_with("A  1\n"), "{}", out.text());
    }

    #[test]
    fn colours_only_when_asked() {
        let (mut output, _, err) = Output::buffers();
        output.color = true;
        output.warn("w");
        assert!(
            err.text()
                .starts_with("\u{1b}[1m\u{1b}[33mwarning:\u{1b}[0m w"),
            "{:?}",
            err.text()
        );
        let output = Output::process(&Env::default());
        assert!(!output.color && !output.stdout_tty);
        assert_eq!(output.width, None);
    }

    #[test]
    fn width_and_now_from_the_environment() {
        assert_eq!(width(&Env::with_vars(&[("COLUMNS", "80")])), Some(80));
        assert_eq!(width(&Env::with_vars(&[("COLUMNS", "0")])), Some(1));
        assert_eq!(width(&Env::with_vars(&[("COLUMNS", "wide")])), None);
        let fixed = now(&Env::with_vars(&[(NOW, "2026-10-11T12:00:00Z")]));
        assert_eq!(crate::time::format(fixed), "2026-10-11T12:00:00Z");
        assert!(
            now(&Env::with_vars(&[])) > fixed - std::time::Duration::from_secs(86_400 * 365 * 10)
        );
    }
}
