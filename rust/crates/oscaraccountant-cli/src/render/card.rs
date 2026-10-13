//! One object as a detail card: a header (title, status, key amount), aligned `Label  value`
//! pairs, a section per nested object, a sub-table per list of objects, and the ids and stamps
//! in a dim `Meta` section at the end.

use serde_json::{Map, Value};
use unicode_width::UnicodeWidthStr;

use super::format::{self, Cell, Context};
use super::list::{self, Columns};
use super::theme::{Role, Sym};
use crate::generated::{FieldKind, View, ViewField};

/// The rows of a sub-table before `… N more`.
pub const SUB_ROWS: usize = 10;

/// The indent of a section.
const INDENT: usize = 2;

/// The stamps that go to `Meta`.
const STAMPS: [&str; 2] = ["created_at", "updated_at"];

/// One line of a card, before it is indented.
type Lines = Vec<String>;

/// The card of `object` in its view.
pub fn render(
    view: &View,
    object: &Map<String, Value>,
    width: Option<usize>,
    context: &Context,
    link: Option<&str>,
) -> String {
    let mut lines = Vec::new();
    let header = header(view, object, context, link);
    let in_header: Vec<&str> = header.1.clone();
    if !header.0.is_empty() {
        lines.push(header.0);
        lines.push(String::new());
    }
    let unlimited = view.title.is_empty();
    lines.extend(body(
        view,
        object,
        &[],
        width,
        context,
        &in_header,
        0,
        unlimited,
    ));
    let meta = meta(view, object, context);
    if !meta.is_empty() {
        if lines.last().is_some_and(|line| !line.is_empty()) {
            lines.push(String::new());
        }
        lines.push(context.theme.paint(Role::Dim, "Meta"));
        lines.extend(indent(meta));
    }
    while lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }
    lines.join("\n")
}

/// The header line, and the fields it shows: the title in bold, the status, the first money
/// field of the columns.
fn header(
    view: &View,
    object: &Map<String, Value>,
    context: &Context,
    link: Option<&str>,
) -> (String, Vec<&'static str>) {
    let mut parts = Vec::new();
    let mut shown = Vec::new();
    if let Some(field) = view.field(view.title) {
        let cell = format::field_cell(field, object, &[], view.status.as_ref(), context, false);
        if cell.width() > 0 {
            let title = context.theme.bold(&cell.plain());
            parts.push(match link {
                Some(url) => context.theme.link(url, &title),
                None => title,
            });
            shown.push(field.name);
        } else if let Some(fallback) = list::title_fallback(object) {
            // A draft has no number yet: its short id is what a person types.
            parts.push(fallback.paint(&context.theme));
        }
    }
    if let Some(status) = view.status.as_ref()
        && let Some(value) = format::status_value(object, status)
    {
        parts.push(format::status(value, Some(status), context).paint(&context.theme));
        shown.push(status.field);
    }
    let amount = view
        .columns
        .iter()
        .filter_map(|name| view.field(name))
        .find(|field| matches!(field.kind, FieldKind::Money { .. }));
    if let Some(field) = amount {
        let cell = format::field_cell(field, object, &[], None, context, false);
        if cell.width() > 0 && !parts.is_empty() {
            parts.push(context.theme.bold(&cell.paint(&context.theme)));
            shown.push(field.name);
        }
    }
    (parts.join("  "), shown)
}

/// A field that goes to `Meta`: an id, or a stamp.
fn is_meta(field: &ViewField) -> bool {
    matches!(field.kind, FieldKind::Id | FieldKind::Ulid | FieldKind::Ref)
        || (field.kind == FieldKind::DateTime && STAMPS.contains(&field.name))
}

/// The pairs, sections and sub-tables of an object.
#[allow(clippy::too_many_arguments)]
fn body(
    view: &View,
    object: &Map<String, Value>,
    scope: &[&Map<String, Value>],
    width: Option<usize>,
    context: &Context,
    skip: &[&str],
    depth: usize,
    unlimited: bool,
) -> Lines {
    let mut pairs: Vec<(String, Cell)> = Vec::new();
    let mut sections: Lines = Vec::new();
    // A section repeats nothing its parent shows: the revision of an invoice holds the same
    // dates and amounts as the invoice, the settlement its number and currency.
    let parent = scope.first().filter(|_| depth > 0);
    for field in view.fields {
        if skip.contains(&field.name)
            || is_meta(field)
            || view.hidden.contains(&field.name)
            || format::is_english_of_another(field.name, view.fields)
        {
            continue;
        }
        let value = format::shown_value(field.name, object);
        if list::is_scalar(field)
            && parent.is_some_and(|parent| format::shown_value(field.name, parent) == value)
        {
            continue;
        }
        if value.is_null()
            || value.as_str().is_some_and(str::is_empty)
            || (field.kind == FieldKind::List && value.as_array().is_some_and(Vec::is_empty))
        {
            continue;
        }
        match (field.kind, value) {
            (FieldKind::Object(inner), Value::Object(nested)) => {
                let inner_scope: Vec<&Map<String, Value>> = std::iter::once(object)
                    .chain(scope.iter().copied())
                    .collect();
                let lines = body(
                    inner,
                    nested,
                    &inner_scope,
                    width.map(|w| w.saturating_sub(INDENT)),
                    context,
                    &[],
                    depth + 1,
                    false,
                );
                if !lines.is_empty() {
                    sections.push(String::new());
                    sections.push(context.theme.bold(field.label));
                    sections.extend(indent(lines));
                }
            }
            (FieldKind::Array(inner), Value::Array(items)) => {
                let rows: Vec<&Map<String, Value>> =
                    items.iter().filter_map(Value::as_object).collect();
                if rows.is_empty() {
                    if items.is_empty() {
                        sections.push(String::new());
                        sections.push(format!(
                            "{} {}",
                            context.theme.bold(field.label),
                            context.theme.paint(Role::Dim, "(none)")
                        ));
                    }
                    continue;
                }
                let inner_scope: Vec<&Map<String, Value>> = std::iter::once(object)
                    .chain(scope.iter().copied())
                    .collect();
                sections.push(String::new());
                sections.push(format!(
                    "{} {}",
                    context.theme.bold(field.label),
                    context.theme.paint(Role::Dim, &format!("({})", rows.len()))
                ));
                let limit = if unlimited && depth == 0 {
                    rows.len()
                } else {
                    SUB_ROWS
                };
                let inner_width = width.map(|w| w.saturating_sub(INDENT));
                let shown = &rows[..rows.len().min(limit)];
                let table = list::render(
                    inner,
                    shown,
                    &inner_scope,
                    &Columns::Default,
                    inner_width,
                    context,
                )
                .unwrap_or_default();
                let mut table_lines = table.lines().map(str::to_owned);
                sections.extend(indent(table_lines.next().into_iter().collect()));
                for (row, line) in shown.iter().zip(table_lines) {
                    sections.extend(indent(vec![line]));
                    // A row that holds lists of its own (an account of the general ledger and its
                    // lines): each list right below the row, indented once more.
                    sections.extend(indent(indent(nested(
                        inner,
                        row,
                        &inner_scope,
                        inner_width.map(|w| w.saturating_sub(INDENT)),
                        context,
                        unlimited && depth == 0,
                    ))));
                }
                if rows.len() > limit {
                    sections.push(format!(
                        "{}{}",
                        " ".repeat(INDENT),
                        context.theme.paint(
                            Role::Dim,
                            &format!(
                                "{} {} more (--json for all)",
                                context.theme.symbol(Sym::Ellipsis),
                                rows.len() - limit
                            )
                        )
                    ));
                }
            }
            (FieldKind::MoneyMap { currency_field }, Value::Object(map)) => {
                let currency = format::currency(currency_field, object, scope);
                let pairs: Vec<(String, Cell)> = map
                    .iter()
                    .filter(|(_, value)| !value.is_null())
                    .map(|(key, value)| {
                        let cell = match value.as_i64() {
                            Some(minor) => format::money(i128::from(minor), currency, context),
                            None => Cell::text(format::summary_text(value, context)),
                        };
                        (format::one_line(key), cell)
                    })
                    .collect();
                // The amounts line up on the right, as in a table.
                let widest = pairs
                    .iter()
                    .map(|(_, cell)| cell.width())
                    .max()
                    .unwrap_or(0);
                let lines: Lines = pairs_lines(
                    pairs
                        .into_iter()
                        .map(|(key, mut cell)| {
                            let pad = widest.saturating_sub(cell.width());
                            if let Some(first) = cell.segments.first_mut() {
                                first.text = format!("{}{}", " ".repeat(pad), first.text);
                            }
                            (key, cell)
                        })
                        .collect(),
                    context,
                );
                if !lines.is_empty() {
                    sections.push(String::new());
                    sections.push(context.theme.bold(field.label));
                    sections.extend(indent(lines));
                }
            }
            (FieldKind::Json, Value::Object(map)) => {
                // A snapshot (`seller_snapshot`): readable labels, and each nested value in words.
                let lines: Lines = pairs_lines(
                    map.iter()
                        .filter(|(_, value)| !is_blank(value))
                        .map(|(key, value)| {
                            (
                                super::infer::label(key),
                                Cell::text(format::readable_text(value)),
                            )
                        })
                        .collect(),
                    context,
                );
                if !lines.is_empty() {
                    sections.push(String::new());
                    sections.push(context.theme.bold(field.label));
                    sections.extend(indent(lines));
                }
            }
            _ => {
                let cell = format::value_cell(
                    field,
                    value,
                    object,
                    scope,
                    view.status.as_ref(),
                    context,
                    false,
                );
                pairs.push((field.label.to_owned(), cell));
            }
        }
    }
    let mut lines = pairs_lines(pairs, context);
    lines.extend(sections);
    if lines.first().is_some_and(String::is_empty) {
        lines.remove(0);
    }
    lines
}

/// Null, empty text, or an empty list or object: nothing to show.
fn is_blank(value: &Value) -> bool {
    match value {
        Value::Null => true,
        Value::String(text) => text.trim().is_empty(),
        Value::Array(items) => items.iter().all(is_blank),
        Value::Object(map) => map.values().all(is_blank),
        _ => false,
    }
}

/// The lists inside one row of a sub-table, each a table of its own (the lines of an account of
/// the general ledger). An empty list prints nothing: the row says enough.
fn nested(
    view: &View,
    row: &Map<String, Value>,
    scope: &[&Map<String, Value>],
    width: Option<usize>,
    context: &Context,
    unlimited: bool,
) -> Lines {
    let mut lines = Vec::new();
    let inner_scope: Vec<&Map<String, Value>> =
        std::iter::once(row).chain(scope.iter().copied()).collect();
    for field in view.fields {
        let (FieldKind::Array(inner), Some(Value::Array(items))) =
            (field.kind, row.get(field.name))
        else {
            continue;
        };
        let rows: Vec<&Map<String, Value>> = items.iter().filter_map(Value::as_object).collect();
        if rows.is_empty() {
            continue;
        }
        let limit = if unlimited { rows.len() } else { SUB_ROWS };
        let table = list::render(
            inner,
            &rows[..rows.len().min(limit)],
            &inner_scope,
            &Columns::Default,
            width,
            context,
        )
        .unwrap_or_default();
        lines.extend(table.lines().map(str::to_owned));
        if rows.len() > limit {
            lines.push(context.theme.paint(
                Role::Dim,
                &format!(
                    "{} {} more (--json for all)",
                    context.theme.symbol(Sym::Ellipsis),
                    rows.len() - limit
                ),
            ));
        }
    }
    lines
}

/// Aligned `Label  value` lines.
fn pairs_lines(pairs: Vec<(String, Cell)>, context: &Context) -> Lines {
    let label_width = pairs
        .iter()
        .map(|(label, _)| label.width())
        .max()
        .unwrap_or(0);
    pairs
        .into_iter()
        .map(|(label, cell)| {
            let pad = " ".repeat(label_width - label.width());
            format!(
                "{}{pad}  {}",
                context.theme.paint(Role::Muted, &label),
                cell.paint(&context.theme)
            )
            .trim_end()
            .to_owned()
        })
        .collect()
}

/// The ids and stamps of the object, in full, dim.
fn meta(view: &View, object: &Map<String, Value>, context: &Context) -> Lines {
    let pairs: Vec<(String, String)> = view
        .fields
        .iter()
        .filter(|field| is_meta(field))
        .filter_map(|field| {
            object
                .get(field.name)
                .and_then(Value::as_str)
                .filter(|text| !text.is_empty())
                .map(|text| (field.label.to_owned(), format::one_line(text)))
        })
        .collect();
    let label_width = pairs
        .iter()
        .map(|(label, _)| label.width())
        .max()
        .unwrap_or(0);
    pairs
        .into_iter()
        .map(|(label, value)| {
            let pad = " ".repeat(label_width - label.width());
            context
                .theme
                .paint(Role::Dim, &format!("{label}{pad}  {value}"))
        })
        .collect()
}

fn indent(lines: Lines) -> Lines {
    lines
        .into_iter()
        .map(|line| {
            if line.is_empty() {
                line
            } else {
                format!("{}{line}", " ".repeat(INDENT))
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::generated::{
        VIEW_BOOKS_GENERAL_LEDGER, VIEW_BOOKS_INVOICE_DETAIL, VIEW_BOOKS_OPEN_ITEMS,
        VIEW_BOOKS_VAT_PREVIEW,
    };
    use crate::render::theme::Theme;
    use crate::time;

    fn context(color: bool) -> Context {
        Context {
            theme: Theme {
                color,
                ..Theme::PLAIN
            },
            now: time::parse("2026-10-11T12:00:00Z").unwrap(),
        }
    }

    fn line(no: i64) -> Value {
        json!({"id": format!("L{no}"), "line_no": no, "kind": "item", "description": format!("Line {no}"),
               "net_minor": 1000, "tax_minor": 240, "gross_minor": 1240, "vat_code": "EE24"})
    }

    fn invoice(lines: usize) -> Value {
        json!({
            "id": "01JX8Z3Q4R5S6T7V8W9XAYBZCD", "number": "2026-0042", "party_name": "Acme OÜ",
            "party_id": "01JX8Z3Q4R5S6T7V8W9XAYBZCE", "currency": "EUR", "gross_minor": 122_000,
            "net_minor": 100_000, "due_date": "2026-10-20", "settlement_state": "paid",
            "created_at": "2026-09-01T10:00:00Z", "note": null, "review_note": "",
            "revision": {"id": "R1", "revision_no": 1, "currency": "EUR", "net_minor": 100_000,
                          "lines": (1..=lines as i64).map(line).collect::<Vec<_>>(),
                          "seller_snapshot": {"name": "Seller OÜ", "vat": null}},
        })
    }

    #[test]
    fn a_card_with_header_sections_and_meta() {
        let value = invoice(3);
        let text = render(
            &VIEW_BOOKS_INVOICE_DETAIL,
            value.as_object().unwrap(),
            Some(100),
            &context(false),
            None,
        );
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[0], "2026-0042  ● paid  1 220.00 EUR", "{text}");
        assert!(
            lines.iter().any(|line| line.starts_with("Party name")),
            "{text}"
        );
        assert!(text.contains("\nRevision\n"), "{text}");
        assert!(text.contains("  Lines (3)"), "{text}");
        assert!(text.contains("Line 3"));
        assert!(text.contains("Seller OÜ"));
        assert!(text.contains("\nMeta\n  ID"), "{text}");
        assert!(text.contains("2026-09-01T10:00:00Z"));
        assert!(!text.contains("Review note"));
    }

    #[test]
    fn a_long_sub_table_stops_at_ten_rows() {
        let value = invoice(24);
        let text = render(
            &VIEW_BOOKS_INVOICE_DETAIL,
            value.as_object().unwrap(),
            Some(120),
            &context(false),
            None,
        );
        assert!(
            text.contains("Line 10") && !text.contains("Line 11"),
            "{text}"
        );
        assert!(text.contains("… 14 more (--json for all)"));
    }

    #[test]
    fn a_report_shows_every_row() {
        let items: Vec<Value> = (0..14)
            .map(|n| json!({"line_id": format!("l{n}"), "account_code": "1200", "party_name": format!("P{n}"),
                            "currency": "EUR", "residual_minor": 100, "residual_base_minor": 100,
                            "due_date": "2026-10-01", "aging_bucket": "1_30", "side": "debit"}))
            .collect();
        let value = json!({"as_of": "2026-10-11", "base_currency": "EUR", "items": items});
        let text = render(
            &VIEW_BOOKS_OPEN_ITEMS,
            value.as_object().unwrap(),
            Some(120),
            &context(false),
            None,
        );
        assert!(text.contains("P13"), "{text}");
        assert!(text.starts_with("As of"), "{text}");
    }

    #[test]
    fn a_cancelled_invoice_shows_its_state_and_no_overdue() {
        // Self-test P1-1 and P1-2: `● unposted` on a cancelled invoice, and `overdue 24d` in the
        // revision of a paid one.
        let mut value = invoice(1);
        value["settlement_state"] = json!("unposted");
        value["state"] = json!("cancelled");
        value["due_date"] = json!("2026-09-17");
        value["revision"]["state"] = json!("current");
        value["revision"]["due_date"] = json!("2026-09-17");
        let text = render(
            &VIEW_BOOKS_INVOICE_DETAIL,
            value.as_object().unwrap(),
            Some(120),
            &context(false),
            None,
        );
        assert!(
            text.starts_with("2026-0042  ● cancelled  1 220.00 EUR"),
            "{text}"
        );
        assert!(!text.contains("overdue"), "{text}");
        value["state"] = json!("posted");
        value["settlement_state"] = json!("paid");
        let text = render(
            &VIEW_BOOKS_INVOICE_DETAIL,
            value.as_object().unwrap(),
            Some(120),
            &context(false),
            None,
        );
        assert!(text.starts_with("2026-0042  ● paid"), "{text}");
        assert!(!text.contains("overdue"), "{text}");
        value["settlement_state"] = json!("unpaid");
        let text = render(
            &VIEW_BOOKS_INVOICE_DETAIL,
            value.as_object().unwrap(),
            Some(120),
            &context(false),
            None,
        );
        assert!(text.contains("overdue 24d"), "{text}");
    }

    #[test]
    fn a_draft_has_its_short_id_as_title() {
        // Self-test P2-4: the card of a draft started with blanks.
        let mut value = invoice(1);
        value["number"] = json!(null);
        value["state"] = json!("recorded");
        value["settlement_state"] = json!("unposted");
        let text = render(
            &VIEW_BOOKS_INVOICE_DETAIL,
            value.as_object().unwrap(),
            Some(120),
            &context(false),
            None,
        );
        assert!(text.starts_with("9XAYBZCD  ● recorded"), "{text}");
    }

    #[test]
    fn vat_boxes_are_money() {
        // Self-test P1-3: `KMD.1  248000` was 2 480.00 EUR in minor units.
        let value = json!({"form": "KMD", "form_version": "2026", "period_from": "2026-09-01",
                           "period_to": "2026-09-30", "base_currency": "EUR", "recognitions": 4,
                           "boxes": {"KMD.1": 248_000, "KMD.4": 59_520, "KMD.5": 10_200}});
        let text = render(
            &VIEW_BOOKS_VAT_PREVIEW,
            value.as_object().unwrap(),
            Some(80),
            &context(false),
            None,
        );
        assert!(text.contains("\nBoxes\n  KMD.1  2 480.00 EUR"), "{text}");
        assert!(text.contains("KMD.4    595.20 EUR"), "{text}");
        assert!(!text.contains("248000"), "{text}");
    }

    #[test]
    fn the_general_ledger_shows_the_lines_of_each_account() {
        // Self-test P1-4: the `lines` of each account were dropped.
        let line = |memo: &str| {
            json!({"entry_id": "E1", "entry_no": 7, "journal": "bank", "entry_state": "posted",
                   "economic_date": "2026-09-03", "line_no": 1, "account_code": "1000",
                   "memo": memo, "currency": "EUR", "amount_minor": 12_000, "base_minor": 12_000})
        };
        let value = json!({"base_currency": "EUR", "accounts": [
            {"account_code": "1000", "account_name": "Bank", "account_name_en": null,
             "opening_base_minor": 100_000, "closing_base_minor": 112_000,
             "lines": [line("Payment from Acme"), line("Fee")]},
            {"account_code": "2000", "account_name": "Payables", "account_name_en": null,
             "opening_base_minor": 0, "closing_base_minor": 0, "lines": []},
        ]});
        let text = render(
            &VIEW_BOOKS_GENERAL_LEDGER,
            value.as_object().unwrap(),
            Some(120),
            &context(false),
            None,
        );
        let lines: Vec<&str> = text.lines().collect();
        let bank = lines.iter().position(|l| l.contains("Bank")).unwrap();
        assert!(lines[bank + 1].contains("MEMO"), "{text}");
        assert!(lines[bank + 2].contains("Payment from Acme"), "{text}");
        assert!(lines[bank + 2].starts_with("    "), "{text}");
        assert!(lines[bank + 3].contains("Fee"), "{text}");
        assert!(lines[bank + 4].contains("Payables"), "{text}");
    }

    #[test]
    fn colours_the_card() {
        let value = invoice(1);
        let text = render(
            &VIEW_BOOKS_INVOICE_DETAIL,
            value.as_object().unwrap(),
            Some(100),
            &context(true),
            None,
        );
        assert!(text.starts_with("\u{1b}[1m2026-0042\u{1b}[0m"), "{text}");
        assert!(text.contains("\u{1b}[2mMeta\u{1b}[0m"));
    }

    #[test]
    fn a_section_repeats_nothing_and_snapshots_read_as_words() {
        // Self-test P2-7 and P2-8: `Sign 1`, the revision and the settlement repeating the
        // header, `bank_accounts  [1]` and `reg_code` in the seller snapshot.
        let value = json!({
            "id": "01JX8Z3Q4R5S6T7V8W9XAYBZCD", "number": "EE-2026-004", "party_name": "Echo OÜ",
            "sign": 1, "origin": "human", "state": "posted", "currency": "EUR",
            "invoice_date": "2026-09-21", "gross_minor": 124_000, "settlement_state": "paid",
            "revision": {"revision_no": 1, "state": "current", "invoice_date": "2026-09-21",
                          "currency": "EUR", "gross_minor": 124_000, "fx_rate_source": "same",
                          "source_payload": {"raw": true},
                          "seller_snapshot": {"name": "Seller OÜ", "reg_code": "64453120",
                                              "bank_accounts": [{"iban": "EE12", "bic": "LHVBEE22"}],
                                              "address": "", "vat_number": null},
                          "lines": [{"line_no": 1, "description": "Robot arm", "vat_code": "EE-SAL-STD",
                                     "net_minor": 100_000, "tax_minor": 24_000, "gross_minor": 124_000,
                                     "account_code": null}]},
            "settlement": {"number": "EE-2026-004", "currency": "EUR", "state": "posted",
                           "gross_minor": 124_000, "residual_minor": 0, "status": "settled"},
        });
        let text = render(
            &VIEW_BOOKS_INVOICE_DETAIL,
            value.as_object().unwrap(),
            Some(120),
            &context(false),
            None,
        );
        assert!(!text.contains("Sign"), "{text}");
        assert!(!text.contains("Origin"), "{text}");
        assert!(!text.contains("Source payload"), "{text}");
        assert!(!text.contains("● current"), "{text}");
        let revision = text.split("\nRevision\n").nth(1).unwrap();
        assert!(!revision.contains("Invoice date"), "{text}");
        assert!(revision.contains("FX rate source"), "{text}");
        let settlement = text.split("\nSettlement\n").nth(1).unwrap();
        assert!(!settlement.contains("Number"), "{text}");
        assert!(!settlement.contains("Currency"), "{text}");
        assert!(settlement.contains("Residual"), "{text}");
        assert!(text.contains("  Seller snapshot\n"), "{text}");
        assert!(text.contains("Registry code  64453120"), "{text}");
        assert!(text.contains("Bank accounts  EE12 LHVBEE22"), "{text}");
        assert!(!text.contains("Address"), "{text}");
        assert!(!text.contains("reg_code"), "{text}");
        assert!(!text.contains("LINE NO"), "{text}");
        assert!(!text.contains("ACCOUNT CODE"), "{text}");
        assert!(text.contains("VAT CODE"), "{text}");
        assert!(is_blank(&json!([{"a": null}, ""])));
        assert!(!is_blank(&json!(0)));
    }
}
