//! The answer to "which receipts are missing?" (`oscar bank-transactions list --to-explain`): the
//! receipt state of each payment, and the bank lines the answer was checked against.
//!
//! A server that knows the receipt states adds `receipt_state`, `receipt_detail`,
//! `match_proposal_id` and `matched_invoice_candidates` to each bank transaction, and a
//! `coverage` object to the page of the payments to explain. The CLI keys on these fields, not on
//! its generated views: an older server answers without them, and the output stays as it was.

use std::sync::Mutex;

use serde_json::{Map, Value};
use unicode_width::UnicodeWidthStr;

use super::format::{Context, one_line};
use super::theme::{Role, Sym};
use crate::generated::{FieldKind, View, ViewField};

/// The field of the receipt state of a payment.
pub const RECEIPT_STATE: &str = "receipt_state";

/// The field that says why a received receipt is not booked yet.
pub const RECEIPT_DETAIL: &str = "receipt_detail";

/// The page field of the bank lines the answer covers.
pub const COVERAGE: &str = "coverage";

/// The receipt fields of a bank transaction, for a view that does not have them yet.
static RECEIPT_FIELDS: [ViewField; 4] = [
    ViewField {
        name: RECEIPT_STATE,
        label: "Receipt",
        kind: FieldKind::Enum {
            values: &["missing", "receipt_not_booked", "invoice_not_matched"],
        },
        nullable: true,
    },
    ViewField {
        name: RECEIPT_DETAIL,
        label: "Receipt detail",
        kind: FieldKind::Enum {
            values: &["reading", "review", "recorded", "failed", "no_extraction"],
        },
        nullable: true,
    },
    ViewField {
        name: "matched_invoice_candidates",
        label: "Booked invoices that fit",
        kind: FieldKind::Array(&VIEW_INVOICE_CANDIDATE),
        nullable: false,
    },
    ViewField {
        name: "match_proposal_id",
        label: "Match proposal ID",
        kind: FieldKind::Ref,
        nullable: true,
    },
];

/// One booked invoice that fits a payment.
static VIEW_INVOICE_CANDIDATE: View = View {
    name: "BankTransactionInvoiceCandidate",
    title: "number",
    fields: &[
        ViewField {
            name: "number",
            label: "Number",
            kind: FieldKind::Text,
            nullable: true,
        },
        ViewField {
            name: "invoice_id",
            label: "Invoice ID",
            kind: FieldKind::Ref,
            nullable: false,
        },
    ],
    columns: &["number", "invoice_id"],
    wide: &[],
    status: None,
    hidden: &[],
};

/// The views extended once in this process: the name of their schema, every row with a state.
type Extended = Vec<(&'static str, bool, &'static View)>;
static EXTENDED: Mutex<Extended> = Mutex::new(Vec::new());

/// A receipt state as a person reads it.
pub fn state_label(value: &str) -> Option<&'static str> {
    Some(match value {
        "missing" => "missing",
        "receipt_not_booked" => "received, not booked",
        "invoice_not_matched" => "invoice booked, not matched",
        _ => return None,
    })
}

/// Why a received receipt is not booked yet, as a person reads it.
pub fn detail_label(value: &str) -> Option<&'static str> {
    Some(match value {
        "reading" => "being read",
        "review" => "draft waiting for review",
        "recorded" => "invoice recorded, not posted",
        "failed" => "could not be read",
        "no_extraction" => "not read (not an invoice file)",
        _ => return None,
    })
}

/// The colour of a receipt state: a missing receipt is the one to act on.
pub fn state_role(value: &str) -> Option<Role> {
    match value {
        "missing" => Some(Role::Bad),
        "receipt_not_booked" | "invoice_not_matched" => Some(Role::Warn),
        _ => None,
    }
}

/// The label of a value of a receipt field, when `name` is one.
pub fn label(name: &str, value: &str) -> Option<&'static str> {
    match name {
        RECEIPT_STATE => state_label(value),
        RECEIPT_DETAIL => detail_label(value),
        _ => None,
    }
}

/// What an answer says about receipt states.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum States {
    /// No item has the field: an older server, or another schema.
    None,
    /// Some items have a state: a plain list of bank transactions (money in has none).
    Some,
    /// Every item has a state: the payments to explain, or one payment.
    Every,
}

/// The receipt states an answer carries: of an object, or of the items of a page or a list. The
/// field with `null` tells a server that knows the states.
pub fn states(value: &Value) -> States {
    let objects: Vec<&Map<String, Value>> = match value {
        Value::Object(object) => match object.get("items") {
            Some(Value::Array(items)) => items.iter().filter_map(Value::as_object).collect(),
            _ => vec![object],
        },
        Value::Array(items) => items.iter().filter_map(Value::as_object).collect(),
        _ => Vec::new(),
    };
    if !objects
        .iter()
        .any(|object| object.contains_key(RECEIPT_STATE))
    {
        return States::None;
    }
    let stated = |object: &&Map<String, Value>| {
        object
            .get(RECEIPT_STATE)
            .is_some_and(|state| !state.is_null())
    };
    if objects.iter().all(stated) {
        States::Every
    } else {
        States::Some
    }
}

/// The view of an answer with receipt states: the view with the receipt fields it lacks, and the
/// receipt state as a list column. When every row has a state (the payments to explain), the
/// receipt state takes the place of the status, which goes to `-o wide`: it is `unmatched` or
/// `part matched` on every row and says less. Else the receipt state comes after the status.
/// The view as it is for an answer with no state, or a view that already shows the state.
pub fn view_for(view: &'static View, value: &Value) -> &'static View {
    let states = states(value);
    if states == States::None || view.columns.contains(&RECEIPT_STATE) {
        return view;
    }
    let every = states == States::Every;
    let mut known = EXTENDED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some((_, _, extended)) = known
        .iter()
        .find(|(name, mode, _)| *name == view.name && *mode == every)
    {
        return extended;
    }
    let extended: &'static View = Box::leak(Box::new(extend(view, every)));
    known.push((view.name, every, extended));
    extended
}

/// `view` with every receipt field it lacks, and the receipt state in its columns: in the place
/// of the status (`every`), or after it.
fn extend(view: &'static View, every: bool) -> View {
    let mut fields: Vec<ViewField> = view.fields.to_vec();
    for field in &RECEIPT_FIELDS {
        match fields.iter_mut().find(|known| known.name == field.name) {
            // A field the generated view already has keeps its place, with the label of a
            // person ("Booked invoices that fit", not "Matched invoice candidates"). The
            // candidates keep the hand-written view: the generated one shows only the number,
            // which may be empty, and drops the invoice ID.
            Some(known) => {
                known.label = field.label;
                if field.name == "matched_invoice_candidates" {
                    known.kind = field.kind;
                }
            }
            None => fields.push(*field),
        }
    }
    let mut columns: Vec<&'static str> = view.columns.to_vec();
    let mut wide: Vec<&'static str> = view.wide.to_vec();
    let status = view
        .status
        .as_ref()
        .and_then(|status| columns.iter().position(|name| *name == status.field));
    match status {
        Some(at) if every => {
            wide.insert(0, columns[at]);
            columns[at] = RECEIPT_STATE;
        }
        Some(at) => columns.insert(at + 1, RECEIPT_STATE),
        None => columns.push(RECEIPT_STATE),
    }
    View {
        fields: Vec::leak(fields),
        columns: Vec::leak(columns),
        wide: Vec::leak(wide),
        ..*view
    }
}

/// The priority of the receipt state column in a table: kept as long as a status when it stands
/// in the place of the status, else after the amount, like the second state it is.
pub fn column_priority(view: &View) -> usize {
    let beside_status = view
        .status
        .as_ref()
        .is_some_and(|status| view.columns.contains(&status.field));
    if beside_status { 3 } else { 1 }
}

/// `YYYY-MM-DD` as a person reads it: `DD.MM.YYYY`. Anything else as it is, on one line.
pub fn date_text(date: &str) -> String {
    let parts: Vec<&str> = date.split('-').collect();
    match parts.as_slice() {
        [year, month, day]
            if year.len() == 4
                && month.len() == 2
                && day.len() == 2
                && date
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || byte == b'-') =>
        {
            format!("{day}.{month}.{year}")
        }
        _ => one_line(date),
    }
}

/// A month `YYYY-MM` as its year and number, when it is one.
fn month(text: &str) -> Option<(u32, u32)> {
    let (year, month) = text.split_once('-')?;
    if year.len() != 4 || month.len() != 2 {
        return None;
    }
    let year: u32 = year.parse().ok()?;
    let month: u32 = month.parse().ok()?;
    (1..=12).contains(&month).then_some((year, month))
}

/// `YYYY-MM` as a person reads it: `MM.YYYY`.
fn month_text(year: u32, month: u32) -> String {
    format!("{month:02}.{year}")
}

/// Months in words, a run of months that follow each other as one range:
/// `["2026-01", "2026-02", "2026-03", "2026-05"]` is `01.2026 to 03.2026, 05.2026`.
pub fn months_text(months: &[&str]) -> String {
    let mut parts: Vec<String> = Vec::new();
    let mut run: Option<((u32, u32), (u32, u32))> = None;
    let close = |run: ((u32, u32), (u32, u32)), parts: &mut Vec<String>| {
        let (first, last) = run;
        if first == last {
            parts.push(month_text(first.0, first.1));
        } else {
            parts.push(format!(
                "{} to {}",
                month_text(first.0, first.1),
                month_text(last.0, last.1)
            ));
        }
    };
    for text in months {
        let Some(this) = month(text) else {
            if let Some(open) = run.take() {
                close(open, &mut parts);
            }
            parts.push(one_line(text));
            continue;
        };
        run = match run {
            Some((first, last)) if next_month(last) == this => Some((first, this)),
            Some(open) => {
                close(open, &mut parts);
                Some((this, this))
            }
            None => Some((this, this)),
        };
    }
    if let Some(open) = run {
        close(open, &mut parts);
    }
    parts.join(", ")
}

fn next_month((year, month): (u32, u32)) -> (u32, u32) {
    if month == 12 {
        (year + 1, 1)
    } else {
        (year, month + 1)
    }
}

/// The name of a bank account with its IBAN: `LHV (EE001)`.
fn account_name(account: &Map<String, Value>) -> String {
    let text = |name: &str| {
        account
            .get(name)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(one_line)
    };
    match (text("name"), text("iban")) {
        (Some(name), Some(iban)) => format!("{name} ({iban})"),
        (Some(name), None) => name,
        (None, Some(iban)) => iban,
        (None, None) => text("bank_account_id").unwrap_or_else(|| "?".to_owned()),
    }
}

/// Why an account is not covered, in words.
fn reason_text(reason: Option<&str>) -> String {
    match reason {
        Some("no_bank_lines") => "no bank lines in the books".to_owned(),
        Some("feed_not_linked") => "bank feed account, not linked to the books".to_owned(),
        Some(other) => super::format::enum_words(other),
        None => "not covered".to_owned(),
    }
}

/// The text of a list of strings in a field of `object`.
fn strings<'a>(object: &'a Map<String, Value>, name: &str) -> Vec<&'a str> {
    object
        .get(name)
        .and_then(Value::as_array)
        .map(|items| items.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default()
}

/// Rows of a name and a text, the texts aligned after the longest name, indented.
fn aligned(rows: Vec<(String, String)>) -> Vec<String> {
    let widest = rows.iter().map(|(name, _)| name.width()).max().unwrap_or(0);
    rows.into_iter()
        .map(|(name, text)| {
            let pad = " ".repeat(widest.saturating_sub(name.width()));
            format!("  {name}{pad}  {text}").trim_end().to_owned()
        })
        .collect()
}

/// The coverage block printed after the table of the payments to explain: per bank account the
/// range of its bank lines and the months with none, the accounts with no bank lines, and what
/// is not checked. `None` when the page has no coverage (an older server, or another list).
pub fn coverage_lines(page: &Map<String, Value>, context: &Context) -> Option<Vec<String>> {
    let coverage = page.get(COVERAGE)?.as_object()?;
    let theme = &context.theme;
    let dot = theme.symbol(Sym::Middot);
    let objects = |name: &str| -> Vec<&Map<String, Value>> {
        coverage
            .get(name)
            .and_then(Value::as_array)
            .map(|items| items.iter().filter_map(Value::as_object).collect())
            .unwrap_or_default()
    };
    let accounts = objects("accounts");
    let uncovered = objects("uncovered_accounts");
    let mut lines = Vec::new();
    if accounts.is_empty() {
        lines.push(theme.bold("Checked: no bank lines in the books."));
    } else {
        let from = coverage.get("checked_from").and_then(Value::as_str);
        let to = coverage.get("checked_to").and_then(Value::as_str);
        let heading = match (from, to) {
            (Some(from), Some(to)) => format!(
                "Checked against the bank lines in the books, {} to {}:",
                date_text(from),
                date_text(to)
            ),
            _ => "Checked against the bank lines in the books:".to_owned(),
        };
        lines.push(theme.bold(&heading));
        let rows = accounts
            .iter()
            .map(|account| {
                let date = |name: &str| account.get(name).and_then(Value::as_str).map(date_text);
                let mut parts = Vec::new();
                match (date("first_booking_date"), date("last_booking_date")) {
                    (Some(first), Some(last)) if first != last => {
                        parts.push(format!("{first} to {last}"));
                    }
                    (Some(day), _) | (None, Some(day)) => parts.push(day),
                    (None, None) => {}
                }
                if let Some(count) = account.get("transaction_count").and_then(Value::as_i64) {
                    let noun = if count == 1 { "line" } else { "lines" };
                    parts.push(format!("{count} {noun}"));
                }
                let gaps = strings(account, "gap_months");
                if !gaps.is_empty() {
                    parts.push(
                        theme.paint(Role::Warn, &format!("no lines in {}", months_text(&gaps))),
                    );
                }
                (account_name(account), parts.join(&format!(" {dot} ")))
            })
            .collect();
        lines.extend(aligned(rows));
    }
    if !uncovered.is_empty() {
        lines.push(theme.bold("Not checked:"));
        let rows = uncovered
            .iter()
            .map(|account| {
                let reason = account.get("reason").and_then(Value::as_str);
                (
                    account_name(account),
                    theme.paint(Role::Warn, &reason_text(reason)),
                )
            })
            .collect();
        lines.extend(aligned(rows));
    }
    lines.push("Months and accounts outside this range are not checked.".to_owned());
    Some(lines)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::generated::{VIEW_BOOKS_BANK_TRANSACTION, VIEW_BOOKS_INVOICE};
    use crate::render::theme::Theme;
    use crate::time;

    fn context() -> Context {
        Context {
            theme: Theme::PLAIN,
            now: time::parse("2026-11-20T12:00:00Z").unwrap(),
        }
    }

    #[test]
    fn labels_of_the_states() {
        assert_eq!(state_label("missing"), Some("missing"));
        assert_eq!(
            state_label("receipt_not_booked"),
            Some("received, not booked")
        );
        assert_eq!(
            state_label("invoice_not_matched"),
            Some("invoice booked, not matched")
        );
        assert_eq!(state_label("other"), None);
        assert_eq!(
            label(RECEIPT_DETAIL, "review"),
            Some("draft waiting for review")
        );
        assert_eq!(label("status", "review"), None);
        assert_eq!(state_role("missing"), Some(Role::Bad));
        assert_eq!(state_role("invoice_not_matched"), Some(Role::Warn));
        assert_eq!(state_role("x"), None);
        for value in ["reading", "recorded", "failed", "no_extraction"] {
            assert!(detail_label(value).is_some(), "{value}");
        }
        assert_eq!(detail_label("x"), None);
    }

    #[test]
    fn dates_and_months_for_a_person() {
        assert_eq!(date_text("2026-09-04"), "04.09.2026");
        assert_eq!(date_text("soon"), "soon");
        assert_eq!(date_text("2026-9-4"), "2026-9-4");
        assert_eq!(
            months_text(&["2026-01", "2026-02", "2026-03", "2026-05"]),
            "01.2026 to 03.2026, 05.2026"
        );
        assert_eq!(months_text(&["2025-12", "2026-01"]), "12.2025 to 01.2026");
        assert_eq!(
            months_text(&["2026-01", "later", "2026-02"]),
            "01.2026, later, 02.2026"
        );
        assert_eq!(months_text(&["2026-13"]), "2026-13");
        assert_eq!(months_text(&[]), "");
    }

    #[test]
    fn the_view_grows_only_with_receipt_states() {
        let old = json!({"items": [{"id": "a"}], "next_cursor": null});
        assert_eq!(states(&old), States::None);
        assert!(std::ptr::eq(
            view_for(&VIEW_BOOKS_BANK_TRANSACTION, &old),
            &VIEW_BOOKS_BANK_TRANSACTION
        ));
        assert_eq!(states(&json!("x")), States::None);
        assert_eq!(states(&json!({"receipt_state": "missing"})), States::Every);
        assert_eq!(
            states(&json!([{"receipt_state": "missing"}])),
            States::Every
        );

        // A plain list: money in has no state. The receipt comes after the status.
        let mixed = json!({"items": [{"id": "a", "receipt_state": null},
                                     {"id": "b", "receipt_state": "missing"}],
                           "next_cursor": null});
        assert_eq!(states(&mixed), States::Some);
        let view = view_for(&VIEW_BOOKS_BANK_TRANSACTION, &mixed);
        assert_eq!(view.name, VIEW_BOOKS_BANK_TRANSACTION.name);
        let status = view.columns.iter().position(|c| *c == "status").unwrap();
        assert_eq!(view.columns[status + 1], RECEIPT_STATE);
        assert_eq!(view.field(RECEIPT_STATE).unwrap().label, "Receipt");
        let candidates = view.field("matched_invoice_candidates").unwrap();
        match candidates.kind {
            FieldKind::Array(inner) => assert_eq!(inner.columns, &["number", "invoice_id"]),
            _ => panic!("the candidates are a list"),
        }
        assert_eq!(column_priority(view), 3);
        // Extended once: the second call gives the same view.
        assert!(std::ptr::eq(
            view,
            view_for(&VIEW_BOOKS_BANK_TRANSACTION, &mixed)
        ));

        // The payments to explain: the receipt stands in the place of the status.
        let every =
            json!({"items": [{"id": "b", "receipt_state": "missing"}], "next_cursor": null});
        let view = view_for(&VIEW_BOOKS_BANK_TRANSACTION, &every);
        assert!(!view.columns.contains(&"status"));
        assert_eq!(view.wide.first(), Some(&"status"));
        assert_eq!(view.columns[status], RECEIPT_STATE);
        assert_eq!(column_priority(view), 1);
        // A view that already shows the state stays as it is.
        assert!(std::ptr::eq(view_for(view, &every), view));
        // A view with no status in its columns gets the column last.
        let invoice = view_for(&VIEW_BOOKS_INVOICE, &mixed);
        assert_eq!(invoice.columns.last(), Some(&RECEIPT_STATE));
    }

    fn coverage_page() -> Map<String, Value> {
        json!({
            "items": [],
            "next_cursor": null,
            "coverage": {
                "accounts": [
                    {"bank_account_id": "01J0", "name": "LHV", "iban": "EE001", "currency": "EUR",
                     "transaction_count": 120, "first_booking_date": "2026-09-01",
                     "last_booking_date": "2026-11-18", "booked_months": ["2026-09", "2026-11"],
                     "gap_months": ["2026-10"]},
                    {"bank_account_id": "01J1", "name": "Swedbank", "iban": null, "currency": "EUR",
                     "transaction_count": 1, "first_booking_date": "2026-11-02",
                     "last_booking_date": "2026-11-02", "booked_months": ["2026-11"],
                     "gap_months": []}
                ],
                "uncovered_accounts": [
                    {"bank_account_id": "01J2", "name": "TransferWise", "iban": "EE003",
                     "currency": "EUR", "reason": "no_bank_lines"},
                    {"bank_account_id": null, "name": "LHV Investeerimiskonto", "iban": "EE004",
                     "currency": "EUR", "reason": "feed_not_linked"}
                ],
                "checked_from": "2026-09-01",
                "checked_to": "2026-11-18"
            }
        })
        .as_object()
        .unwrap()
        .clone()
    }

    #[test]
    fn the_coverage_block() {
        let lines = coverage_lines(&coverage_page(), &context()).unwrap();
        assert_eq!(
            lines.join("\n"),
            "Checked against the bank lines in the books, 01.09.2026 to 18.11.2026:\n\
             \x20 LHV (EE001)  01.09.2026 to 18.11.2026 · 120 lines · no lines in 10.2026\n\
             \x20 Swedbank     02.11.2026 · 1 line\n\
             Not checked:\n\
             \x20 TransferWise (EE003)            no bank lines in the books\n\
             \x20 LHV Investeerimiskonto (EE004)  bank feed account, not linked to the books\n\
             Months and accounts outside this range are not checked."
        );
    }

    #[test]
    fn a_coverage_with_no_bank_lines() {
        let page = json!({"items": [], "next_cursor": null, "coverage": {
            "accounts": [], "uncovered_accounts": [], "checked_from": null, "checked_to": null}});
        let lines = coverage_lines(page.as_object().unwrap(), &context()).unwrap();
        assert_eq!(
            lines,
            [
                "Checked: no bank lines in the books.",
                "Months and accounts outside this range are not checked."
            ]
        );
        // No coverage, or a null one: nothing to print.
        let page = json!({"items": [], "next_cursor": null, "coverage": null});
        assert!(coverage_lines(page.as_object().unwrap(), &context()).is_none());
        let page = json!({"items": [], "next_cursor": null});
        assert!(coverage_lines(page.as_object().unwrap(), &context()).is_none());
    }

    #[test]
    fn names_and_reasons_of_odd_accounts() {
        let page = json!({"items": [], "next_cursor": null, "coverage": {
            "accounts": [{"bank_account_id": "01J0", "name": "", "iban": "EE9\u{1b}[31m",
                          "first_booking_date": "2026-09-01"}],
            "uncovered_accounts": [{"bank_account_id": "01J2", "reason": "closed_by_bank"},
                                   {"name": "X"}]}});
        let lines = coverage_lines(page.as_object().unwrap(), &context()).unwrap();
        assert_eq!(lines[0], "Checked against the bank lines in the books:");
        assert_eq!(lines[1], "  EE9 [31m  01.09.2026");
        assert_eq!(lines[3], "  01J2  closed by bank");
        assert_eq!(lines[4], "  X     not covered");
    }
}
