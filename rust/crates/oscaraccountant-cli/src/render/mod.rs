//! The output for a person: the theme, the formats of each kind of value, the borderless
//! tables and the detail cards.
//!
//! A command that prints an answer calls [`show`] with the answer, the [`Options`] of the run
//! (`-o`, `--columns`, `--json [fields]`, `--jq`, `--template`) and the view of its operation
//! (`Command::view`), or `None` for a view guessed from the JSON. `show` picks the form:
//!
//! | Form | When |
//! |---|---|
//! | `--template` | given |
//! | `--jq` | given (after `--json a,b` selected the fields) |
//! | NDJSON | `-o ndjson` |
//! | JSON | `--json`, `-o json`, or standard output is not a terminal (and no `-o table`) |
//! | a table | a page or a list, `-o table` (default) or `-o wide` |
//! | a card | one object |
//!
//! The JSON is exactly the answer of the API (`Output::json`), never with a colour code.
//! Notes for the person (`Showing 20 · more: ...`, the empty state) go to standard error.

pub mod card;
pub mod format;
pub mod infer;
pub mod list;
pub mod machine;
pub mod table;
pub mod theme;

use serde_json::{Map, Value};

use crate::error::{CliError, Result};
use crate::generated::View;
use crate::output::Output;
use format::Context;
use list::Columns;
use theme::{Role, Sym};

/// `-o/--output`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    /// A table or a card.
    Table,
    /// A table with the `wide` columns and short ids.
    Wide,
    /// JSON, as the API answered.
    Json,
    /// One JSON value per line: each item of a list.
    Ndjson,
}

impl Format {
    /// The format named on the command line.
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "table" => Some(Self::Table),
            "wide" => Some(Self::Wide),
            "json" => Some(Self::Json),
            "ndjson" => Some(Self::Ndjson),
            _ => None,
        }
    }
}

/// The output options of one run.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Options {
    /// `-o`, when given.
    pub format: Option<Format>,
    /// `--json` (with or without fields).
    pub json: bool,
    /// `--json=a,b`: the fields to keep.
    pub fields: Vec<String>,
    /// `--columns a,b`.
    pub columns: Option<Vec<String>>,
    /// `--jq <expr>`.
    pub jq: Option<String>,
    /// `--template <text>`.
    pub template: Option<String>,
}

impl Options {
    /// JSON (or NDJSON) for a program, not a table for a person.
    pub fn machine(&self, stdout_tty: bool) -> bool {
        match self.format {
            Some(Format::Json | Format::Ndjson) => true,
            Some(Format::Table | Format::Wide) => self.json,
            None => self.json || !stdout_tty,
        }
    }

    fn columns(&self) -> Columns {
        match (&self.columns, self.format) {
            (Some(names), _) => Columns::Exact(names.clone()),
            (None, Some(Format::Wide)) => Columns::Wide,
            _ => Columns::Default,
        }
    }
}

/// What `show` says about a list besides the rows.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ListInfo {
    /// What the items are, plural: `invoices`.
    pub noun: String,
    /// A filter was given: the empty state says so.
    pub filtered: bool,
    /// The command that makes one, for the empty state: `oscar invoices record --data ...`.
    pub create: Option<String>,
    /// The page of the web app: the title of a card links to it (OSC 8).
    pub link: Option<String>,
}

/// The success line of a command: `✓ <message>`, the check in green.
pub fn success_text(message: &str) -> String {
    format!(
        "{} {message}",
        theme::paint(Role::Ok, theme::symbol(Sym::Check))
    )
}

/// Print the success line of a command on standard error: `✓ <message>`.
pub fn success_line(out: &mut Output, message: &str) {
    out.note(&success_text(message));
}

/// The context of the cells on standard output.
pub fn context(out: &Output) -> Context {
    Context {
        theme: out.theme,
        now: out.now,
    }
}

/// Print an answer of the API in the form the options ask for.
pub fn show(
    out: &mut Output,
    options: &Options,
    value: &Value,
    view: Option<&'static View>,
    info: &ListInfo,
) -> Result<()> {
    let selected;
    let value = if options.fields.is_empty() {
        value
    } else {
        selected = machine::select(value, &options.fields, view)?;
        &selected
    };
    if let Some(source) = &options.template {
        let text = machine::template(source, value)?;
        out.line(text.trim_end_matches('\n'));
        return Ok(());
    }
    if let Some(expression) = &options.jq {
        for line in machine::jq(expression, value)? {
            out.line(&line);
        }
        return Ok(());
    }
    if options.format == Some(Format::Ndjson) {
        for line in machine::ndjson(value) {
            out.line(&line);
        }
        return Ok(());
    }
    if options.machine(out.stdout_tty) {
        out.json(value);
        return Ok(());
    }
    human(out, options, value, view, info)
}

/// A table, a card, or a scalar.
fn human(
    out: &mut Output,
    options: &Options,
    value: &Value,
    view: Option<&'static View>,
    info: &ListInfo,
) -> Result<()> {
    let context = context(out);
    let (items, cursor): (Vec<&Map<String, Value>>, Option<&str>) = match value {
        Value::Object(object) if machine::is_page(object) => (
            object
                .get("items")
                .and_then(Value::as_array)
                .map(|items| items.iter().filter_map(Value::as_object).collect())
                .unwrap_or_default(),
            object.get("next_cursor").and_then(Value::as_str),
        ),
        Value::Array(items) if items.iter().all(Value::is_object) => {
            (items.iter().filter_map(Value::as_object).collect(), None)
        }
        Value::Object(object) => {
            let view = view.unwrap_or_else(|| infer::infer_object(&[object]));
            if options.columns.is_some() {
                return Err(CliError::usage("--columns is for a list."));
            }
            let text = card::render(view, object, out.width, &context, info.link.as_deref());
            out.line(&text);
            return Ok(());
        }
        Value::Array(items) => {
            for item in items {
                out.line(&format::scalar_text(item));
            }
            return Ok(());
        }
        other => {
            out.line(&format::scalar_text(other));
            return Ok(());
        }
    };
    let view = view.unwrap_or_else(|| infer::infer_object(&items));
    if items.is_empty() {
        // Check `--columns` even with nothing to show.
        list::fields(view, &options.columns())?;
        out.note(&empty_state(info));
        return Ok(());
    }
    let text = list::render(view, &items, &[], &options.columns(), out.width, &context)?;
    out.line(&text);
    if let Some(cursor) = cursor.filter(|cursor| !cursor.is_empty()) {
        out.note(&theme::paint(Role::Dim, &more_hint(items.len(), cursor)));
    }
    Ok(())
}

/// The longest cursor the footer prints: a longer one is not typed by hand.
const SHORT_CURSOR: usize = 24;

/// The footer of a page with more after it.
pub fn more_hint(shown: usize, cursor: &str) -> String {
    let dot = theme::symbol(Sym::Middot);
    if cursor.chars().count() <= SHORT_CURSOR {
        format!("Showing {shown} {dot} more: --cursor {cursor} (or --all)")
    } else {
        format!("Showing {shown} {dot} more: --all (or --cursor <next_cursor of --json>)")
    }
}

/// The empty state of a list, and the next command.
pub fn empty_state(info: &ListInfo) -> String {
    let noun = if info.noun.is_empty() {
        "items"
    } else {
        &info.noun
    };
    let mut text = if info.filtered {
        format!("No {noun} match these filters.")
    } else {
        format!("No {noun} yet.")
    };
    if let Some(command) = info.create.as_deref().filter(|_| !info.filtered) {
        text.push_str(&format!(
            " {} {}",
            theme::symbol(Sym::Arrow),
            theme::paint(Role::Accent, command)
        ));
    }
    text
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::generated::VIEW_BOOKS_INVOICE;
    use crate::time;
    use theme::Theme;

    fn output(tty: bool) -> (Output, crate::output::Buffer, crate::output::Buffer) {
        let (mut out, stdout, stderr) = Output::buffers();
        out.stdout_tty = tty;
        out.width = Some(120);
        out.now = time::parse("2026-10-11T12:00:00Z").unwrap();
        (out, stdout, stderr)
    }

    fn page(cursor: Value) -> Value {
        json!({"items": [{"id": "01JX8Z3Q4R5S6T7V8W9XAYBZCD", "number": "2026-0042", "party_name": "Acme OÜ",
                          "currency": "EUR", "gross_minor": 122_000, "due_date": "2026-10-20",
                          "settlement_state": "unpaid"}],
               "next_cursor": cursor})
    }

    fn info() -> ListInfo {
        ListInfo {
            noun: "invoices".into(),
            filtered: false,
            create: Some("oscar invoices record --data @invoice.json".into()),
            link: None,
        }
    }

    #[test]
    fn json_on_a_pipe_is_the_answer() {
        let (mut out, stdout, stderr) = output(false);
        let value = page(json!("c2"));
        show(
            &mut out,
            &Options::default(),
            &value,
            Some(&VIEW_BOOKS_INVOICE),
            &info(),
        )
        .unwrap();
        assert_eq!(
            stdout.text(),
            format!("{}\n", serde_json::to_string(&value).unwrap())
        );
        assert_eq!(stderr.text(), "");
    }

    #[test]
    fn a_table_on_a_terminal_with_a_footer() {
        let (mut out, stdout, stderr) = output(true);
        show(
            &mut out,
            &Options::default(),
            &page(json!("c2")),
            Some(&VIEW_BOOKS_INVOICE),
            &info(),
        )
        .unwrap();
        assert!(stdout.text().starts_with("NUMBER"), "{}", stdout.text());
        assert_eq!(stderr.text(), "Showing 1 · more: --cursor c2 (or --all)\n");
        let options = Options {
            format: Some(Format::Table),
            json: true,
            ..Options::default()
        };
        let (mut out, stdout, _) = output(true);
        show(
            &mut out,
            &options,
            &page(json!(null)),
            Some(&VIEW_BOOKS_INVOICE),
            &info(),
        )
        .unwrap();
        assert!(stdout.text().starts_with("{\n"));
        let options = Options {
            format: Some(Format::Table),
            ..Options::default()
        };
        let (mut out, stdout, _) = output(false);
        show(
            &mut out,
            &options,
            &page(json!(null)),
            Some(&VIEW_BOOKS_INVOICE),
            &info(),
        )
        .unwrap();
        assert!(stdout.text().starts_with("NUMBER"));
    }

    #[test]
    fn a_long_cursor_is_not_printed() {
        // Self-test P2-11: a 165-character footer wrapped.
        let long = "7b22".repeat(30);
        let hint = more_hint(50, &long);
        assert_eq!(
            hint,
            "Showing 50 · more: --all (or --cursor <next_cursor of --json>)"
        );
        assert!(!hint.contains(&long));
        assert_eq!(
            more_hint(2, "c2"),
            "Showing 2 · more: --cursor c2 (or --all)"
        );
    }

    #[test]
    fn the_empty_state() {
        let (mut out, stdout, stderr) = output(true);
        let empty = json!({"items": [], "next_cursor": null});
        show(
            &mut out,
            &Options::default(),
            &empty,
            Some(&VIEW_BOOKS_INVOICE),
            &info(),
        )
        .unwrap();
        assert_eq!(stdout.text(), "");
        assert_eq!(
            stderr.text(),
            "No invoices yet. › oscar invoices record --data @invoice.json\n"
        );
        let filtered = ListInfo {
            filtered: true,
            ..info()
        };
        assert_eq!(empty_state(&filtered), "No invoices match these filters.");
        assert_eq!(empty_state(&ListInfo::default()), "No items yet.");
        let options = Options {
            columns: Some(vec!["nope".into()]),
            ..Options::default()
        };
        let (mut out, _, _) = output(true);
        assert!(
            show(
                &mut out,
                &options,
                &empty,
                Some(&VIEW_BOOKS_INVOICE),
                &info()
            )
            .is_err()
        );
    }

    #[test]
    fn machine_forms() {
        let value = page(json!(null));
        let run = |options: Options| {
            let (mut out, stdout, _) = output(true);
            out.width = None;
            show(
                &mut out,
                &options,
                &value,
                Some(&VIEW_BOOKS_INVOICE),
                &info(),
            )
            .unwrap();
            stdout.text()
        };
        assert_eq!(
            run(Options {
                json: true,
                fields: vec!["number".into()],
                ..Options::default()
            }),
            "{\n  \"items\": [\n    {\n      \"number\": \"2026-0042\"\n    }\n  ],\n  \"next_cursor\": null\n}\n"
        );
        assert_eq!(
            run(Options {
                jq: Some(".items[0].number".into()),
                ..Options::default()
            }),
            "2026-0042\n"
        );
        assert_eq!(
            run(Options {
                template: Some("{% for i in items %}{{ i.number }}\n{% endfor %}".into()),
                ..Options::default()
            }),
            "2026-0042\n"
        );
        assert!(
            run(Options {
                format: Some(Format::Ndjson),
                ..Options::default()
            })
            .starts_with("{\"id\":")
        );
        assert!(
            run(Options {
                format: Some(Format::Wide),
                ..Options::default()
            })
            .contains("9XAYBZCD")
        );
        assert!(
            run(Options {
                columns: Some(vec!["number".into()]),
                ..Options::default()
            })
            .starts_with("NUMBER\n2026-0042")
        );
    }

    #[test]
    fn cards_scalars_and_guessed_views() {
        let (mut out, stdout, _) = output(true);
        show(
            &mut out,
            &Options::default(),
            &json!({"name": "Acme", "id": "c1"}),
            None,
            &info(),
        )
        .unwrap();
        assert_eq!(stdout.text(), "Acme\n\nMeta\n  ID  c1\n");
        let (mut out, stdout, _) = output(true);
        show(
            &mut out,
            &Options::default(),
            &json!(["a", 1]),
            None,
            &info(),
        )
        .unwrap();
        assert_eq!(stdout.text(), "a\n1\n");
        let (mut out, stdout, _) = output(true);
        show(&mut out, &Options::default(), &json!("x"), None, &info()).unwrap();
        assert_eq!(stdout.text(), "x\n");
        let (mut out, stdout, _) = output(true);
        show(
            &mut out,
            &Options::default(),
            &json!([{"name": "a"}]),
            None,
            &info(),
        )
        .unwrap();
        assert_eq!(stdout.text(), "NAME\na\n");
        let options = Options {
            columns: Some(vec!["a".into()]),
            ..Options::default()
        };
        let (mut out, _, _) = output(true);
        assert!(show(&mut out, &options, &json!({"a": 1}), None, &info()).is_err());
    }

    #[test]
    fn success_lines_and_formats() {
        let _theme = theme::scoped(Theme::PLAIN);
        let (mut out, _, stderr) = output(true);
        success_line(&mut out, "Issued invoice 2026-0042");
        assert_eq!(stderr.text(), "✓ Issued invoice 2026-0042\n");
        for name in ["table", "wide", "json", "ndjson"] {
            assert!(Format::parse(name).is_some());
        }
        assert_eq!(Format::parse("yaml"), None);
        let options = Options {
            format: Some(Format::Json),
            ..Options::default()
        };
        assert!(options.machine(true));
        assert!(!Options::default().machine(true));
        assert!(Options::default().machine(false));
    }
}
