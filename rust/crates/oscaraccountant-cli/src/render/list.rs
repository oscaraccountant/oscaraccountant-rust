//! A list of objects as a table, from their view: the default columns, `-o wide`, `--columns`.

use serde_json::{Map, Value};

use super::format::{self, Context, Scope};
use super::table::{self, Column};
use crate::error::{CliError, Result};
use crate::generated::{FieldKind, View, ViewField};

/// Which columns a list shows.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Columns {
    /// The columns of the view.
    #[default]
    Default,
    /// The columns of the view, then its `wide` columns, ids in short form.
    Wide,
    /// Exactly these, in this order (`--columns`).
    Exact(Vec<String>),
}

/// The fields of the columns, checked: an unknown name of `--columns` fails with suggestions.
pub fn fields(view: &View, columns: &Columns) -> Result<Vec<&'static ViewField>> {
    let names: Vec<&str> = match columns {
        Columns::Default => view.columns.to_vec(),
        Columns::Wide => view.columns.iter().chain(view.wide).copied().collect(),
        Columns::Exact(names) => names.iter().map(String::as_str).collect(),
    };
    let mut found = Vec::new();
    for name in names {
        match view.field(name).or_else(|| by_header(view, name)) {
            // `account_name_en` shows in the place of `account_name` (`format::shown_value`).
            Some(field)
                if *columns != Columns::Default && *columns != Columns::Wide
                    || !format::is_english_of_another(field.name, view.fields) =>
            {
                if !found
                    .iter()
                    .any(|known: &&ViewField| known.name == field.name)
                {
                    found.push(field);
                }
            }
            Some(_) => {}
            None => {
                return Err(unknown_field(
                    name,
                    view.fields.iter().map(|field| field.name),
                ));
            }
        }
    }
    if *columns == Columns::Wide
        && let Some(id) = view
            .field("id")
            .filter(|id| !found.iter().any(|known| known.name == id.name))
    {
        found.push(id);
    }
    if found.is_empty() {
        found.extend(view.fields.iter().filter(|field| is_scalar(field)).take(6));
    }
    Ok(found)
}

/// The field a `--columns` name means when it is not a field name: the header of a column
/// (`gross` for `gross_minor`, `party-name`, `status` for the status of a view).
fn by_header(view: &View, name: &str) -> Option<&'static ViewField> {
    let wanted = name.to_lowercase().replace(['-', ' '], "_");
    view.fields
        .iter()
        .find(|field| {
            field.label.to_lowercase().replace(' ', "_") == wanted
                || field.name == format!("{wanted}_minor")
        })
        .or_else(|| {
            view.status
                .as_ref()
                .filter(|_| wanted == "status")
                .and_then(|status| view.field(status.field))
        })
}

/// A field that fits in a cell.
pub fn is_scalar(field: &ViewField) -> bool {
    !matches!(
        field.kind,
        FieldKind::Object(_) | FieldKind::Array(_) | FieldKind::Json
    )
}

/// The usage error of an unknown field name, with the closest names.
pub fn unknown_field<'a>(name: &str, known: impl Iterator<Item = &'a str>) -> CliError {
    let known: Vec<&str> = known.collect();
    let mut close: Vec<(usize, &str)> = known
        .iter()
        .map(|candidate| (distance(name, candidate), *candidate))
        .filter(|(score, candidate)| {
            *score <= 2.max(name.len() / 3) || candidate.contains(name) || name.contains(candidate)
        })
        .collect();
    close.sort();
    let mut error = CliError::usage(format!("Unknown field `{name}`."));
    if close.is_empty() {
        error.details.push(format!("Fields: {}", known.join(", ")));
    } else {
        let names: Vec<&str> = close
            .iter()
            .take(3)
            .map(|(_, candidate)| *candidate)
            .collect();
        error
            .details
            .push(format!("Did you mean: {}?", names.join(", ")));
    }
    error
}

/// The Levenshtein distance of two names.
fn distance(left: &str, right: &str) -> usize {
    let right: Vec<char> = right.chars().collect();
    let mut previous: Vec<usize> = (0..=right.len()).collect();
    for (row, left_char) in left.chars().enumerate() {
        let mut current = vec![row + 1];
        for (column, right_char) in right.iter().enumerate() {
            let cost = usize::from(left_char != *right_char);
            current.push(
                (previous[column] + cost)
                    .min(previous[column + 1] + 1)
                    .min(current[column] + 1),
            );
        }
        previous = current;
    }
    previous[right.len()]
}

/// The header of a column: the label in capitals. The status of a view that also has a
/// lifecycle `state` is `STATUS`: it shows one or the other (`format::status_value`).
pub fn header(view: &View, field: &ViewField) -> String {
    match view.status.as_ref() {
        Some(status)
            if status.field == field.name && format::has_lifecycle(view.fields, status) =>
        {
            "STATUS".to_owned()
        }
        _ => field.label.to_uppercase(),
    }
}

/// Money a reader looks for first: a total, a closing balance, what is left to pay.
const KEY_MONEY: [&str; 6] = ["closing", "total", "gross", "residual", "balance", "amount"];

/// The priority of a column (0 is kept longest), from its kind: the title, then the status and
/// the short id, the key amount, the first date and the codes, the text, the rest, and last the
/// `wide` extras and the opening balances.
fn priority(view: &View, field: &ViewField, position: usize, first_date: Option<&str>) -> usize {
    if position == 0 || field.name == view.title {
        return 0;
    }
    if view
        .status
        .as_ref()
        .is_some_and(|status| status.field == field.name)
        || matches!(field.kind, FieldKind::Id | FieldKind::Ulid)
    {
        return 1;
    }
    if view.wide.contains(&field.name) && !view.columns.contains(&field.name) {
        return 6;
    }
    match field.kind {
        FieldKind::Money { .. } if field.name.starts_with("opening") => 6,
        FieldKind::Money { .. } if KEY_MONEY.iter().any(|word| field.name.contains(word)) => 2,
        FieldKind::Money { .. } => 3,
        FieldKind::Date if Some(field.name) == first_date => 3,
        FieldKind::Text if field.name == "code" || field.name.ends_with("_code") => 3,
        FieldKind::Text => 4,
        _ => 5,
    }
}

/// The table of `items` (objects) in the view, at `width` columns.
pub fn render(
    view: &View,
    items: &[&Map<String, Value>],
    scope: Scope<'_>,
    columns: &Columns,
    width: Option<usize>,
    context: &Context,
) -> Result<String> {
    let fields = fields(view, columns)?;
    let short_ids = matches!(columns, Columns::Wide);
    // The date a reader looks for first: the due date, else the first date.
    let first_date = fields
        .iter()
        .find(|field| field.name == "due_date")
        .or_else(|| fields.iter().find(|field| field.kind == FieldKind::Date))
        .map(|field| field.name);
    // A default column that is empty in every row says nothing (the account code of lines
    // that have none, a date no file has): it goes, unless it is the title.
    let fields: Vec<&ViewField> = if matches!(columns, Columns::Exact(_)) || items.is_empty() {
        fields
    } else {
        let kept: Vec<&ViewField> = fields
            .iter()
            .copied()
            .filter(|field| {
                field.name == view.title
                    || items.iter().any(|item| {
                        let value = format::shown_value(field.name, item);
                        !(value.is_null()
                            || value.as_str().is_some_and(|text| text.trim().is_empty()))
                    })
            })
            .collect();
        if kept.is_empty() { fields } else { kept }
    };
    let table: Vec<Column> = fields
        .iter()
        .enumerate()
        .map(|(position, field)| {
            let cells = items
                .iter()
                .map(|item| {
                    let cell = format::field_cell(
                        field,
                        item,
                        scope,
                        view.status.as_ref(),
                        context,
                        short_ids,
                    );
                    if field.name == view.title && cell.width() == 0 {
                        title_fallback(item).unwrap_or(cell)
                    } else {
                        cell
                    }
                })
                .collect();
            let column = Column::new(header(view, field), cells);
            match columns {
                Columns::Exact(_) => column,
                _ => column.with_priority(priority(view, field, position, first_date)),
            }
        })
        .collect();
    Ok(table::render(&table, width, &context.theme))
}

/// The title of an object that has none yet (a draft invoice has no number): the short form of
/// its id, dim.
pub fn title_fallback(item: &Map<String, Value>) -> Option<format::Cell> {
    let id = item.get("id").and_then(Value::as_str)?;
    Some(format::Cell::fixed(
        format::short_id(id),
        Some(super::theme::Role::Dim),
        format::Align::Left,
    ))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::generated::{VIEW_BOOKS_INVOICE, VIEW_BOOKS_TRIAL_BALANCE_ROW, VIEW_BOOKS_VAT_CODE};
    use crate::render::theme::Theme;
    use crate::time;

    fn context() -> Context {
        Context {
            theme: Theme::PLAIN,
            now: time::parse("2026-10-11T12:00:00Z").unwrap(),
        }
    }

    #[test]
    fn suggests_a_close_name() {
        let error = fields(&VIEW_BOOKS_INVOICE, &Columns::Exact(vec!["nmber".into()])).unwrap_err();
        assert_eq!(error.message, "Unknown field `nmber`.");
        assert!(error.details[0].contains("number"), "{:?}", error.details);
        let error = fields(
            &VIEW_BOOKS_INVOICE,
            &Columns::Exact(vec!["zzzzzzzzzz".into()]),
        )
        .unwrap_err();
        assert!(error.details[0].starts_with("Fields: id, "));
        assert_eq!(distance("kitten", "sitting"), 3);
        assert_eq!(distance("", "ab"), 2);
    }

    #[test]
    fn exact_wide_and_default_columns() {
        let exact = fields(
            &VIEW_BOOKS_INVOICE,
            &Columns::Exact(vec!["id".into(), "number".into(), "id".into()]),
        )
        .unwrap();
        assert_eq!(
            exact.iter().map(|f| f.name).collect::<Vec<_>>(),
            ["id", "number"]
        );
        let default = fields(&VIEW_BOOKS_INVOICE, &Columns::Default).unwrap();
        let wide = fields(&VIEW_BOOKS_INVOICE, &Columns::Wide).unwrap();
        assert!(wide.len() > default.len());
        assert!(wide.iter().any(|f| f.name == "id"));
        static EMPTY: View = View {
            name: "Empty",
            title: "",
            fields: &[ViewField {
                name: "a",
                label: "A",
                kind: FieldKind::Text,
                nullable: false,
            }],
            columns: &[],
            wide: &[],
            status: None,
            hidden: &[],
        };
        assert_eq!(fields(&EMPTY, &Columns::Default).unwrap().len(), 1);
    }

    fn invoice_row(number: Option<&str>, party: &str) -> Value {
        json!({
            "id": "01JX8Z3Q4R5S6T7V8W9XAYBZCD", "number": number, "party_name": party,
            "currency": "EUR", "gross_minor": 372_000, "due_date": "2026-10-20",
            "settlement_state": "part_paid", "state": "posted", "invoice_date": "2026-09-20",
            "direction": "sales", "kind": "invoice", "open_minor": 100,
        })
    }

    fn widest(text: &str) -> usize {
        text.lines()
            .map(unicode_width::UnicodeWidthStr::width)
            .max()
            .unwrap_or(0)
    }

    #[test]
    fn the_status_and_the_amount_stay_at_80_columns() {
        // Self-test P2-1: at 80 columns the status went first and the party stayed whole.
        let rows = [
            invoice_row(
                Some("EE-2026-006"),
                "Aurora Analytics Holdings International OÜ",
            ),
            invoice_row(Some("EE-2026-007"), "Echo Robotics OÜ"),
        ];
        let items: Vec<&Map<String, Value>> = rows.iter().filter_map(Value::as_object).collect();
        let text = render(
            &VIEW_BOOKS_INVOICE,
            &items,
            &[],
            &Columns::Default,
            Some(80),
            &context(),
        )
        .unwrap();
        assert!(widest(&text) <= 80, "{text}");
        let header = text.lines().next().unwrap();
        assert!(header.starts_with("NUMBER"), "{text}");
        assert!(
            header.contains("STATUS") && header.contains("GROSS"),
            "{text}"
        );
        assert!(
            header.contains("DUE DATE") && !header.contains("INVOICE DATE"),
            "{text}"
        );
        assert!(
            text.contains("● part paid") && text.contains("3 720.00 EUR"),
            "{text}"
        );
        // The order of the columns stays: the status is still the last one.
        assert!(header.trim_end().ends_with("STATUS"), "{text}");
    }

    #[test]
    fn wide_keeps_the_short_id_at_120_and_80_columns() {
        // Self-test P2-2: the short id only appeared at 160 columns.
        let rows = [invoice_row(Some("EE-2026-006"), "Aurora Analytics OÜ")];
        let items: Vec<&Map<String, Value>> = rows.iter().filter_map(Value::as_object).collect();
        for width in [80, 120] {
            let text = render(
                &VIEW_BOOKS_INVOICE,
                &items,
                &[],
                &Columns::Wide,
                Some(width),
                &context(),
            )
            .unwrap();
            assert!(widest(&text) <= width, "{text}");
            assert!(text.contains("XAYBZCD"), "{width}: {text}");
            assert!(text.contains("● part paid"), "{width}: {text}");
        }
    }

    #[test]
    fn a_draft_row_shows_its_short_id() {
        // Self-test P2-4: a draft has no number; its row started with blanks.
        let rows = [invoice_row(None, "Echo Robotics OÜ")];
        let items: Vec<&Map<String, Value>> = rows.iter().filter_map(Value::as_object).collect();
        let text = render(
            &VIEW_BOOKS_INVOICE,
            &items,
            &[],
            &Columns::Default,
            Some(120),
            &context(),
        )
        .unwrap();
        assert!(
            text.lines().nth(1).unwrap().starts_with("9XAYBZCD  "),
            "{text}"
        );
    }

    #[test]
    fn codes_are_never_cut_and_closing_balances_stay() {
        // Self-test P2-3 and P2-1: `EE-PUR-FOREIGN-V…`, and no CLOSING BASE at 80 columns.
        let codes = [
            json!({"id": "v1", "code": "EE-PUR-RC-EU-GOODS-REVERSE", "name": "Reverse charge on goods bought in the EU",
                            "direction": "purchase", "treatment": "rc_eu_goods", "deductible_bps": 10_000, "active": true}),
        ];
        let items: Vec<&Map<String, Value>> = codes.iter().filter_map(Value::as_object).collect();
        let text = render(
            &VIEW_BOOKS_VAT_CODE,
            &items,
            &[],
            &Columns::Default,
            Some(80),
            &context(),
        )
        .unwrap();
        assert!(widest(&text) <= 80, "{text}");
        assert!(text.contains("EE-PUR-RC-EU-GOODS-REVERSE"), "{text}");
        // Self-test P2-10: the treatment in words, not `rc eu goods`.
        assert!(text.contains("reverse charge EU"), "{text}");
        let text = render(
            &VIEW_BOOKS_VAT_CODE,
            &items,
            &[],
            &Columns::Default,
            Some(120),
            &context(),
        )
        .unwrap();
        assert!(text.contains("reverse charge EU goods"), "{text}");
        assert!(text.contains("100%"), "{text}");
        let rows = [
            json!({"account_code": "1000", "account_name": "Bank accounts in euro",
                           "opening_base_minor": 1_234_567, "debit_base_minor": 2_345_678,
                           "credit_base_minor": 3_456_789, "closing_base_minor": 4_567_890,
                           "base_currency": "EUR"}),
        ];
        let items: Vec<&Map<String, Value>> = rows.iter().filter_map(Value::as_object).collect();
        let text = render(
            &VIEW_BOOKS_TRIAL_BALANCE_ROW,
            &items,
            &[],
            &Columns::Default,
            Some(80),
            &context(),
        )
        .unwrap();
        assert!(widest(&text) <= 80, "{text}");
        assert!(
            text.contains("CLOSING BASE") && text.contains("45 678.90 EUR"),
            "{text}"
        );
        assert!(!text.contains("OPENING BASE"), "{text}");
    }

    #[test]
    fn a_column_by_its_header() {
        // Self-test P2-13: the header says GROSS, `--columns gross` failed.
        let names = |columns: &[&str]| {
            fields(
                &VIEW_BOOKS_INVOICE,
                &Columns::Exact(columns.iter().map(|name| (*name).to_owned()).collect()),
            )
            .unwrap()
            .iter()
            .map(|field| field.name)
            .collect::<Vec<_>>()
        };
        assert_eq!(names(&["gross"]), ["gross_minor"]);
        assert_eq!(
            names(&["party-name", "status"]),
            ["party_name", "settlement_state"]
        );
        assert_eq!(names(&["Due date"]), ["due_date"]);
    }

    #[test]
    fn renders_items_in_their_view() {
        let items = [json!({
            "id": "01JX8Z3Q4R5S6T7V8W9XAYBZCD", "number": "2026-0042", "party_name": "Acme OÜ",
            "currency": "EUR", "gross_minor": 122_000, "due_date": "2026-10-06",
            "settlement_state": "unpaid", "invoice_date": "2026-09-06",
        })];
        let items: Vec<&Map<String, Value>> = items.iter().filter_map(Value::as_object).collect();
        let text = render(
            &VIEW_BOOKS_INVOICE,
            &items,
            &[],
            &Columns::Default,
            Some(120),
            &context(),
        )
        .unwrap();
        assert!(text.contains("2026-0042"), "{text}");
        assert!(text.contains("1 220.00 EUR"));
        assert!(text.contains("overdue 5d"));
        let wide = render(
            &VIEW_BOOKS_INVOICE,
            &items,
            &[],
            &Columns::Wide,
            Some(200),
            &context(),
        )
        .unwrap();
        assert!(
            wide.contains("9XAYBZCD") && !wide.contains("01JX8Z3Q"),
            "{wide}"
        );
    }
}
