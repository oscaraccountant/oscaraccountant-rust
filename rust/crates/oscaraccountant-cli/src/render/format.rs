//! The format of each kind of value in a table cell or a card: money, dates, statuses, ids.
//!
//! A formatted value is a [`Cell`]: one or more segments of text, each with an optional role,
//! and how the cell may be laid out (aligned right, cut short). The table and the card measure
//! the plain text and paint it only when they write it.

use std::time::{Duration, SystemTime};

use serde_json::{Map, Value};
use unicode_width::UnicodeWidthStr;

use super::theme::{Role, Sym, Theme};
use crate::generated::{FieldKind, Status, ViewField};
use crate::time;

/// Seven days: a date-time younger than this is shown relative (`3h ago`).
const RELATIVE: u64 = 7 * 86_400;

/// One piece of a cell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Segment {
    /// The plain text.
    pub text: String,
    /// The role, when the piece has a colour.
    pub role: Option<Role>,
}

/// How a cell is aligned in its column.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Align {
    /// Text, dates, statuses.
    Left,
    /// Money and numbers.
    Right,
}

/// A formatted value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cell {
    /// The pieces of the text.
    pub segments: Vec<Segment>,
    /// The alignment in a column.
    pub align: Align,
    /// The text may be cut short with `…`. Ids, dates, money and statuses never are.
    pub truncate: bool,
}

impl Cell {
    /// Plain text that may be cut short.
    pub fn text(text: String) -> Self {
        Self {
            segments: vec![Segment { text, role: None }],
            align: Align::Left,
            truncate: true,
        }
    }

    /// Text that is never cut short, with a role.
    pub fn fixed(text: String, role: Option<Role>, align: Align) -> Self {
        Self {
            segments: vec![Segment { text, role }],
            align,
            truncate: false,
        }
    }

    /// The plain text.
    pub fn plain(&self) -> String {
        self.segments
            .iter()
            .map(|segment| segment.text.as_str())
            .collect()
    }

    /// The width on a terminal.
    pub fn width(&self) -> usize {
        self.segments
            .iter()
            .map(|segment| segment.text.width())
            .sum()
    }

    /// The text with its colours.
    pub fn paint(&self, theme: &Theme) -> String {
        self.segments
            .iter()
            .map(|segment| match segment.role {
                Some(role) => theme.paint(role, &segment.text),
                None => segment.text.clone(),
            })
            .collect()
    }

    /// The cell cut to `width` columns with an ellipsis at the end. Only the last segment is cut.
    pub fn truncated(&self, width: usize, theme: &Theme) -> Self {
        if self.width() <= width {
            return self.clone();
        }
        let ellipsis = theme.symbol(Sym::Ellipsis);
        let mut segments = Vec::new();
        let mut used = 0;
        for segment in &self.segments {
            let room = width.saturating_sub(used);
            if segment.text.width() <= room.saturating_sub(ellipsis.width()) {
                used += segment.text.width();
                segments.push(segment.clone());
                continue;
            }
            let keep = room.saturating_sub(ellipsis.width());
            let mut text = cut(&segment.text, keep);
            if room >= ellipsis.width() {
                text.push_str(ellipsis);
            }
            segments.push(Segment {
                text,
                role: segment.role,
            });
            break;
        }
        Self {
            segments,
            ..self.clone()
        }
    }
}

/// The longest prefix of `text` that fits in `width` columns.
pub fn cut(text: &str, width: usize) -> String {
    let mut used = 0;
    let mut out = String::new();
    for character in text.chars() {
        let size = unicode_width::UnicodeWidthChar::width(character).unwrap_or(0);
        if used + size > width {
            break;
        }
        used += size;
        out.push(character);
    }
    out
}

/// Text from the API as one safe line: control characters (an escape sequence, a new line)
/// become a space, so a value can never move the cursor or change the colour of the terminal.
pub fn one_line(text: &str) -> String {
    text.chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect()
}

/// What a cell needs to know besides the value.
#[derive(Debug, Clone, Copy)]
pub struct Context {
    /// The theme of the stream.
    pub theme: Theme,
    /// Now, for relative times and overdue dates.
    pub now: SystemTime,
}

impl Context {
    /// The UTC date of today, `YYYY-MM-DD`.
    pub fn today(&self) -> String {
        time::format(self.now)[..10].to_owned()
    }
}

/// The number of decimals of a currency (ISO 4217); two when the CLI does not know it.
pub fn decimals(currency: &str) -> u32 {
    match currency {
        "BIF" | "CLP" | "DJF" | "GNF" | "ISK" | "JPY" | "KMF" | "KRW" | "PYG" | "RWF" | "UGX"
        | "UYI" | "VND" | "VUV" | "XAF" | "XOF" | "XPF" => 0,
        "BHD" | "IQD" | "JOD" | "KWD" | "LYD" | "OMR" | "TND" => 3,
        _ => 2,
    }
}

/// Digits in groups of three, with a space: `1220` is `1 220`.
fn group(digits: &str) -> String {
    let mut out = String::new();
    for (index, character) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            out.push(' ');
        }
        out.push(character);
    }
    out
}

/// An amount in minor units: `1 220.00 EUR`, without its sign.
pub fn money_text(minor: i128, currency: Option<&str>) -> String {
    let places = currency.map_or(2, decimals);
    let scale = 10_i128.pow(places);
    let magnitude = minor.unsigned_abs();
    let whole = group(&(magnitude / scale.unsigned_abs()).to_string());
    let mut text = if places == 0 {
        whole
    } else {
        format!(
            "{whole}.{:0width$}",
            magnitude % scale.unsigned_abs(),
            width = places as usize
        )
    };
    if let Some(currency) = currency.filter(|code| !code.is_empty()) {
        text.push(' ');
        text.push_str(&one_line(currency));
    }
    text
}

/// A money cell: right aligned, a negative amount in red, or in parentheses without colour.
pub fn money(minor: i128, currency: Option<&str>, context: &Context) -> Cell {
    let text = money_text(minor, currency);
    if minor >= 0 {
        Cell::fixed(text, None, Align::Right)
    } else if context.theme.color {
        Cell::fixed(format!("-{text}"), Some(Role::Bad), Align::Right)
    } else {
        Cell::fixed(format!("({text})"), None, Align::Right)
    }
}

/// The role of a status value: the role `x-oscarai-cli` gives it, else the role of its word.
pub fn status_role(value: &str, status: Option<&Status>) -> Option<Role> {
    if let Some(role) = status.and_then(|status| {
        status
            .roles
            .iter()
            .find(|(known, _)| *known == value)
            .and_then(|(_, role)| Role::from_name(role))
    }) {
        return Some(role);
    }
    match value {
        "paid" | "posted" | "active" | "matched" | "classified" | "confirmed" | "done"
        | "completed" | "complete" | "approved" | "verified" | "balanced" | "settled"
        | "reconciled" | "accepted" | "succeeded" | "success" | "valid" | "enabled" | "filed"
        | "booked" | "processed" | "ok" | "pass" | "transfer_matched" | "not_due" => Some(Role::Ok),
        "overdue" | "failed" | "rejected" | "error" | "declined" | "expired" | "blocked"
        | "invalid" | "unbalanced" | "suspended" | "bounced" | "fail" => Some(Role::Bad),
        "draft" | "cancelled" | "canceled" | "void" | "voided" | "archived" | "inactive"
        | "closed" | "superseded" | "skipped" | "deleted" | "disabled" | "exempted"
        | "unposted" | "not_an_invoice" | "none" | "no_due_date" => Some(Role::Muted),
        "pending" | "unpaid" | "part_paid" | "partially_paid" | "open" | "in_progress"
        | "processing" | "needs_review" | "review" | "waiting" | "unmatched" | "part_matched"
        | "transfer_unmatched" | "issued" | "sent" | "due" | "onboarding" | "requested"
        | "received" => Some(Role::Warn),
        _ => None,
    }
}

/// The lifecycle field of an object besides its status: `state` of an invoice whose status is its
/// settlement.
const LIFECYCLE: &str = "state";

/// The value the status of `object` shows. It is the status field, except when the object also
/// has a lifecycle `state` that says more: a cancelled or void invoice (its settlement says
/// nothing), or one that is not posted yet (`recorded`, `issued`).
pub fn status_value<'a>(object: &'a Map<String, Value>, status: &Status) -> Option<&'a str> {
    let value = object.get(status.field).and_then(Value::as_str);
    if status.field == LIFECYCLE {
        return value;
    }
    let state = object
        .get(LIFECYCLE)
        .and_then(Value::as_str)
        .filter(|state| !state.is_empty());
    match (value, state) {
        (_, Some(state))
            if matches!(status_role(state, None), Some(Role::Muted | Role::Bad))
                || value.is_none_or(|value| value == "unposted") =>
        {
            Some(state)
        }
        _ => value,
    }
}

/// A view shows its status from two fields: the status, and the lifecycle `state`.
pub fn has_lifecycle(view_fields: &[ViewField], status: &Status) -> bool {
    status.field != LIFECYCLE && view_fields.iter().any(|field| field.name == LIFECYCLE)
}

/// An enum value as words: `part_paid` is `part paid`, a range `1_30` is `1-30`, and the short
/// words of the books are written out: `rc_eu_goods` is `reverse charge EU goods`, `rc_non_eu` is
/// `reverse charge non-EU`.
pub fn enum_words(value: &str) -> String {
    let characters: Vec<char> = one_line(value).chars().collect();
    let spaced: String = characters
        .iter()
        .enumerate()
        .map(|(index, character)| {
            if *character != '_' {
                return *character;
            }
            let digit = |at: Option<&char>| at.is_some_and(char::is_ascii_digit);
            if index > 0 && digit(characters.get(index - 1)) && digit(characters.get(index + 1)) {
                '-'
            } else {
                ' '
            }
        })
        .collect();
    let mut words: Vec<String> = Vec::new();
    let mut joined = false;
    for word in spaced.split(' ') {
        let written = word_text(word);
        match words.last_mut() {
            // `non eu` is `non-EU`.
            Some(last) if joined => {
                last.push('-');
                last.push_str(&written);
                joined = false;
                continue;
            }
            _ => {}
        }
        joined = word == "non";
        words.push(written);
    }
    words.join(" ")
}

/// One word of an enum value as it is written: an acronym in capitals, a short word in full.
fn word_text(word: &str) -> String {
    match word {
        "rc" => "reverse charge",
        "eu" => "EU",
        "vat" => "VAT",
        "oss" => "OSS",
        "fx" => "FX",
        "api" => "API",
        "pdf" => "PDF",
        "oauth" => "OAuth",
        "kmd" => "KMD",
        "prof" => "professional",
        other => other,
    }
    .to_owned()
}

/// The Estonian legal forms as they are written: `ou` is `OÜ`.
fn legal_form(value: &str) -> Option<&'static str> {
    Some(match value {
        "ou" => "OÜ",
        "as" => "AS",
        "fie" => "FIE",
        "mtu" => "MTÜ",
        "sa" => "SA",
        "tu" => "TÜ",
        "uu" => "UÜ",
        _ => return None,
    })
}

/// A status: a dot and the word, coloured by role.
pub fn status(value: &str, status: Option<&Status>, context: &Context) -> Cell {
    let role = status_role(value, status);
    let word = enum_words(value);
    Cell {
        segments: vec![
            Segment {
                text: format!("{} ", context.theme.symbol(Sym::Dot)),
                role: Some(role.unwrap_or(Role::Dim)),
            },
            Segment { text: word, role },
        ],
        align: Align::Left,
        truncate: false,
    }
}

/// A ULID: 26 characters of Crockford base 32.
pub fn is_ulid(text: &str) -> bool {
    text.len() == 26
        && text.bytes().all(|byte| {
            byte.is_ascii_digit() || (byte.is_ascii_uppercase() && !b"ILOU".contains(&byte))
        })
}

/// The short form of an id: the last 8 characters of a ULID, any other id as it is.
pub fn short_id(text: &str) -> String {
    if is_ulid(text) {
        text[18..].to_owned()
    } else {
        one_line(text)
    }
}

/// The days from the date `from` to the date `to`, both `YYYY-MM-DD`.
pub fn days_between(from: &str, to: &str) -> Option<i64> {
    let from = time::parse(&format!("{from}T00:00:00Z"))?;
    let to = time::parse(&format!("{to}T00:00:00Z"))?;
    let seconds = match to.duration_since(from) {
        Ok(after) => i64::try_from(after.as_secs()).ok()?,
        Err(before) => -i64::try_from(before.duration().as_secs()).ok()?,
    };
    Some(seconds / 86_400)
}

/// A date-time: relative under 7 days (`3h ago`, `in 2d`), else its UTC date.
pub fn date_time_text(text: &str, now: SystemTime) -> String {
    let Some(at) = time::parse(text) else {
        return one_line(text);
    };
    let (delta, past) = match now.duration_since(at) {
        Ok(ago) => (ago, true),
        Err(ahead) => (ahead.duration(), false),
    };
    if delta >= Duration::from_secs(RELATIVE) {
        return time::format(at)[..10].to_owned();
    }
    let seconds = delta.as_secs();
    let amount = match seconds {
        0..60 => return "just now".to_owned(),
        60..3600 => format!("{}m", seconds / 60),
        3600..86_400 => format!("{}h", seconds / 3600),
        _ => format!("{}d", seconds / 86_400),
    };
    if past {
        format!("{amount} ago")
    } else {
        format!("in {amount}")
    }
}

/// The objects a value sits in, innermost first: where a currency field is looked up.
pub type Scope<'a> = &'a [&'a Map<String, Value>];

/// The currency of a money field: the field in the object, then in the objects around it, then
/// `base_currency` around it.
pub fn currency<'a>(
    field: &str,
    object: &'a Map<String, Value>,
    scope: Scope<'a>,
) -> Option<&'a str> {
    std::iter::once(object)
        .chain(scope.iter().copied())
        .find_map(|item| item.get(field).and_then(Value::as_str))
        .or_else(|| {
            std::iter::once(object)
                .chain(scope.iter().copied())
                .find_map(|item| item.get("base_currency").and_then(Value::as_str))
        })
}

/// A value in one line: a scalar as itself, a list of scalars joined, a nested value short.
pub fn summary_text(value: &Value, context: &Context) -> String {
    summary(value, &context.theme)
}

/// A short summary of a nested value in a cell: `{…}`, `[3]`.
fn summary(value: &Value, theme: &Theme) -> String {
    match value {
        Value::Array(items)
            if items
                .iter()
                .all(|item| !item.is_object() && !item.is_array()) =>
        {
            items.iter().map(scalar_text).collect::<Vec<_>>().join(", ")
        }
        Value::Array(items) => format!("[{}]", items.len()),
        Value::Object(_) => format!("{{{}}}", theme.symbol(Sym::Ellipsis)),
        other => scalar_text(other),
    }
}

/// A nested value in words, for a snapshot: a scalar as itself, an object as its values, a list
/// of them joined. `[{"iban": "EE12", "bic": "LHVBEE22"}]` is `EE12 LHVBEE22`.
pub fn readable_text(value: &Value) -> String {
    match value {
        Value::Array(items) => items
            .iter()
            .map(readable_text)
            .filter(|text| !text.is_empty())
            .collect::<Vec<_>>()
            .join("; "),
        Value::Object(map) => map
            .values()
            .map(readable_text)
            .filter(|text| !text.is_empty())
            .collect::<Vec<_>>()
            .join(" "),
        other => scalar_text(other),
    }
}

/// A scalar as one line of text.
pub fn scalar_text(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::String(text) => one_line(text),
        Value::Bool(true) => "yes".to_owned(),
        Value::Bool(false) => "no".to_owned(),
        other => other.to_string(),
    }
}

/// Whether the object around a due date is settled: its status has the role ok or muted, or
/// what is left to pay is zero. An object in a settled object is settled too: the due date in
/// the revision of a paid invoice is not overdue.
fn settled(object: &Map<String, Value>, scope: Scope<'_>, status: Option<&Status>) -> bool {
    one_settled(object, status) || scope.iter().any(|outer| one_settled(outer, None))
}

/// The status fields an object around a value may have, best first.
const STATUS_GUESS: [&str; 2] = ["settlement_state", "status"];

fn one_settled(object: &Map<String, Value>, status: Option<&Status>) -> bool {
    let guessed;
    let status = match status {
        Some(status) => Some(status),
        None => {
            guessed = STATUS_GUESS
                .iter()
                .chain(std::iter::once(&LIFECYCLE))
                .find(|name| object.contains_key(**name))
                .map(|name| Status {
                    field: name,
                    roles: &[],
                });
            guessed.as_ref()
        }
    };
    // The status as it is (`paid`, `unposted`) or as it shows (`cancelled`): either one settles.
    let by_status = status.is_some_and(|status| {
        let raw = object.get(status.field).and_then(Value::as_str);
        [raw, status_value(object, status)]
            .into_iter()
            .flatten()
            .filter_map(|value| status_role(value, Some(status)))
            .any(|role| matches!(role, Role::Ok | Role::Muted))
    });
    let nothing_left = ["open_minor", "residual_minor", "amount_due_minor"]
        .iter()
        .find_map(|name| object.get(*name).and_then(Value::as_i64))
        .is_some_and(|left| left == 0);
    by_status || nothing_left
}

/// Text that is an identifier a person types or reads back: a code or a number. It is never
/// cut short, like an id.
fn is_identifier(name: &str) -> bool {
    name == "code"
        || name.ends_with("_code")
        || name == "number"
        || name.ends_with("_number")
        || name == "iban"
}

/// Basis points as a percentage: `10000` is `100%`, `2450` is `24.5%`.
pub fn bps_text(bps: i64) -> String {
    let sign = if bps < 0 { "-" } else { "" };
    let magnitude = bps.unsigned_abs();
    let whole = magnitude / 100;
    let fraction = magnitude % 100;
    if fraction == 0 {
        format!("{sign}{whole}%")
    } else {
        let digits = format!("{fraction:02}");
        format!("{sign}{whole}.{}%", digits.trim_end_matches('0'))
    }
}

/// The cell of one field of an object.
pub fn field_cell(
    field: &ViewField,
    object: &Map<String, Value>,
    scope: Scope<'_>,
    status_of: Option<&Status>,
    context: &Context,
    short_ids: bool,
) -> Cell {
    let value = shown_value(field.name, object);
    value_cell(field, value, object, scope, status_of, context, short_ids)
}

/// The value of a field as a person reads it: the English text of `<name>_en` when the object
/// has one (`account_name_en` beside `account_name`), else the field itself.
pub fn shown_value<'a>(name: &str, object: &'a Map<String, Value>) -> &'a Value {
    let value = object.get(name).unwrap_or(&Value::Null);
    if value.is_string()
        && let Some(english) = object
            .get(&format!("{name}_en"))
            .filter(|english| english.as_str().is_some_and(|text| !text.trim().is_empty()))
    {
        return english;
    }
    value
}

/// A field that only holds the English text of another field of the object: it is shown in
/// that field's place (`shown_value`), never on its own.
pub fn is_english_of_another(name: &str, object_fields: &[ViewField]) -> bool {
    name.strip_suffix("_en")
        .is_some_and(|base| object_fields.iter().any(|field| field.name == base))
}

/// A size in bytes as a person reads it: `38307` is `37.4 KB`.
pub fn bytes_text(bytes: i64) -> String {
    const UNITS: [&str; 4] = ["KB", "MB", "GB", "TB"];
    if bytes.unsigned_abs() < 1024 {
        return format!("{bytes} B");
    }
    #[allow(clippy::cast_precision_loss)]
    let mut size = bytes as f64 / 1024.0;
    let mut unit = 0;
    while size.abs() >= 1024.0 && unit < UNITS.len() - 1 {
        size /= 1024.0;
        unit += 1;
    }
    format!("{size:.1} {}", UNITS[unit])
}

/// Days past the due date: a positive count, `due today`, or `due in 9d` for an item not due.
pub fn days_overdue_text(days: i64) -> String {
    match days {
        0 => "due today".to_owned(),
        days if days < 0 => format!("due in {}d", days.unsigned_abs()),
        days => days.to_string(),
    }
}

/// The cell of a value of a field.
pub fn value_cell(
    field: &ViewField,
    value: &Value,
    object: &Map<String, Value>,
    scope: Scope<'_>,
    status_of: Option<&Status>,
    context: &Context,
    short_ids: bool,
) -> Cell {
    if value.is_null() {
        return Cell::fixed(String::new(), None, Align::Left);
    }
    let is_status = status_of.is_some_and(|status| status.field == field.name);
    match (field.kind, value) {
        (_, Value::String(text)) if is_status => {
            let shown = status_of
                .and_then(|known| status_value(object, known))
                .unwrap_or(text);
            status(shown, status_of, context)
        }
        (FieldKind::Money { currency_field }, Value::Number(number)) => {
            match number.as_i64().map(i128::from) {
                Some(minor) => money(minor, currency(currency_field, object, scope), context),
                None => Cell::fixed(number.to_string(), None, Align::Right),
            }
        }
        (FieldKind::Id | FieldKind::Ulid | FieldKind::Ref, Value::String(text)) => {
            let text = if short_ids {
                short_id(text)
            } else {
                one_line(text)
            };
            Cell::fixed(text, None, Align::Left)
        }
        (FieldKind::Date, Value::String(text)) => {
            let today = context.today();
            // Only an object with a status (an invoice, an open item) is overdue: a due date in a
            // revision or a line is a fact, not a state.
            let overdue = (field.name == "due_date"
                && status_of.is_some()
                && !settled(object, scope, status_of))
            .then(|| days_between(text, &today))
            .flatten()
            .filter(|days| *days > 0);
            match overdue {
                Some(days) => Cell::fixed(format!("overdue {days}d"), Some(Role::Bad), Align::Left),
                None => Cell::fixed(one_line(text), None, Align::Left),
            }
        }
        (FieldKind::DateTime, Value::String(text)) => {
            Cell::fixed(date_time_text(text, context.now), None, Align::Left)
        }
        (FieldKind::Bool, Value::Bool(flag)) => Cell::fixed(
            scalar_text(value),
            (!flag).then_some(Role::Muted),
            Align::Left,
        ),
        (FieldKind::Number, Value::Number(number)) if field.name.ends_with("_bps") => {
            match number.as_i64() {
                Some(bps) => Cell::fixed(bps_text(bps), None, Align::Right),
                None => Cell::fixed(number.to_string(), None, Align::Right),
            }
        }
        (FieldKind::Number, Value::Number(number)) if field.name == "days_overdue" => {
            match number.as_i64() {
                Some(days) => Cell::fixed(
                    days_overdue_text(days),
                    (days > 0).then_some(Role::Bad),
                    Align::Right,
                ),
                None => Cell::fixed(number.to_string(), None, Align::Right),
            }
        }
        (FieldKind::Number, Value::Number(number))
            if field.name.ends_with("_bytes") || field.name == "size" =>
        {
            match number.as_i64() {
                Some(bytes) => Cell::fixed(bytes_text(bytes), None, Align::Right),
                None => Cell::fixed(number.to_string(), None, Align::Right),
            }
        }
        (FieldKind::Number, Value::Number(_)) => Cell::fixed(value.to_string(), None, Align::Right),
        (FieldKind::Enum { .. }, Value::String(text)) => {
            let words = match legal_form(text).filter(|_| field.name == "legal_form") {
                Some(written) => written.to_owned(),
                None => enum_words(text),
            };
            // `reverse charge EU goods` may be cut short in a narrow table, like text.
            Cell::text(words)
        }
        (_, Value::String(text)) if is_identifier(field.name) => {
            Cell::fixed(one_line(text), None, Align::Left)
        }
        (_, Value::String(text)) => Cell::text(one_line(text)),
        (_, Value::Number(_) | Value::Bool(_)) => {
            Cell::fixed(scalar_text(value), None, Align::Left)
        }
        (_, nested) => Cell::text(summary(nested, &context.theme)),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::generated::FieldKind;

    fn context(color: bool) -> Context {
        Context {
            theme: Theme {
                color,
                ..Theme::PLAIN
            },
            now: time::parse("2026-10-11T12:00:00Z").unwrap(),
        }
    }

    fn field(name: &'static str, kind: FieldKind) -> ViewField {
        ViewField {
            name,
            label: name,
            kind,
            nullable: true,
        }
    }

    #[test]
    fn money_formats() {
        assert_eq!(money_text(122_000, Some("EUR")), "1 220.00 EUR");
        assert_eq!(money_text(5, Some("EUR")), "0.05 EUR");
        assert_eq!(money_text(-123_456_789, Some("EUR")), "1 234 567.89 EUR");
        assert_eq!(money_text(1500, Some("JPY")), "1 500 JPY");
        assert_eq!(money_text(1500, Some("KWD")), "1.500 KWD");
        assert_eq!(money_text(100, None), "1.00");
        assert_eq!(money_text(0, Some("")), "0.00");
        let plain = money(-1000, Some("EUR"), &context(false));
        assert_eq!(plain.plain(), "(10.00 EUR)");
        assert_eq!(plain.align, Align::Right);
        let red = money(-1000, Some("EUR"), &context(true));
        assert_eq!(red.plain(), "-10.00 EUR");
        assert_eq!(
            red.paint(&context(true).theme),
            "\u{1b}[31m-10.00 EUR\u{1b}[0m"
        );
        assert_eq!(
            money(1000, Some("EUR"), &context(true)).paint(&context(true).theme),
            "10.00 EUR"
        );
    }

    #[test]
    fn dates_and_times() {
        let now = context(false).now;
        assert_eq!(date_time_text("2026-10-11T11:59:30Z", now), "just now");
        assert_eq!(date_time_text("2026-10-11T11:55:00Z", now), "5m ago");
        assert_eq!(date_time_text("2026-10-11T09:00:00Z", now), "3h ago");
        assert_eq!(date_time_text("2026-10-06T12:00:00Z", now), "5d ago");
        assert_eq!(date_time_text("2026-10-12T15:00:00Z", now), "in 1d");
        assert_eq!(date_time_text("2026-09-01T10:00:00Z", now), "2026-09-01");
        assert_eq!(date_time_text("not a date", now), "not a date");
        assert_eq!(days_between("2026-10-06", "2026-10-11"), Some(5));
        assert_eq!(days_between("2026-10-12", "2026-10-11"), Some(-1));
        assert_eq!(days_between("x", "2026-10-11"), None);
        assert_eq!(context(false).today(), "2026-10-11");
    }

    #[test]
    fn overdue_due_dates() {
        let due = field("due_date", FieldKind::Date);
        let status = Status {
            field: "settlement_state",
            roles: &[("paid", "ok")],
        };
        let open =
            json!({"due_date": "2026-10-06", "settlement_state": "unpaid", "open_minor": 10});
        let open = open.as_object().unwrap();
        let cell = field_cell(&due, open, &[], Some(&status), &context(false), false);
        assert_eq!(
            (cell.plain(), cell.segments[0].role),
            ("overdue 5d".into(), Some(Role::Bad))
        );
        let cell = field_cell(&due, open, &[], None, &context(false), false);
        assert_eq!(cell.plain(), "2026-10-06", "no status: no overdue");
        let paid = json!({"due_date": "2026-10-06", "open_minor": 0});
        let cell = field_cell(
            &due,
            paid.as_object().unwrap(),
            &[],
            Some(&status),
            &context(false),
            false,
        );
        assert_eq!(cell.plain(), "2026-10-06");
        let paid = json!({"due_date": "2026-10-06", "settlement_state": "paid"});
        let cell = field_cell(
            &due,
            paid.as_object().unwrap(),
            &[],
            Some(&status),
            &context(false),
            false,
        );
        assert_eq!(cell.plain(), "2026-10-06");
        let future = json!({"due_date": "2026-10-20"});
        let cell = field_cell(
            &due,
            future.as_object().unwrap(),
            &[],
            Some(&status),
            &context(false),
            false,
        );
        assert_eq!(cell.plain(), "2026-10-20");
        let other = field("invoice_date", FieldKind::Date);
        let old = json!({"invoice_date": "2020-01-01"});
        assert_eq!(
            field_cell(
                &other,
                old.as_object().unwrap(),
                &[],
                None,
                &context(false),
                false
            )
            .plain(),
            "2020-01-01"
        );
    }

    #[test]
    fn statuses() {
        let status_of = Status {
            field: "state",
            roles: &[("issued", "ok")],
        };
        assert_eq!(status_role("issued", Some(&status_of)), Some(Role::Ok));
        assert_eq!(status_role("issued", None), Some(Role::Warn));
        assert_eq!(status_role("paid", None), Some(Role::Ok));
        assert_eq!(status_role("overdue", None), Some(Role::Bad));
        assert_eq!(status_role("draft", None), Some(Role::Muted));
        assert_eq!(status_role("strange", None), None);
        let cell = status("part_paid", None, &context(false));
        assert_eq!(cell.plain(), "● part paid");
        let colored = status("paid", None, &context(true)).paint(&context(true).theme);
        assert!(colored.contains("\u{1b}[32m"), "{colored}");
        let ascii = Context {
            theme: Theme {
                unicode: false,
                ..Theme::PLAIN
            },
            ..context(false)
        };
        assert_eq!(status("paid", None, &ascii).plain(), "* paid");
        let field = field("state", FieldKind::Enum { values: &[] });
        let object = json!({"state": "issued"});
        let cell = field_cell(
            &field,
            object.as_object().unwrap(),
            &[],
            Some(&status_of),
            &context(false),
            false,
        );
        assert_eq!(cell.plain(), "● issued");
    }

    #[test]
    fn ids_and_scalars() {
        let ulid = "01JX8Z3Q4R5S6T7V8W9XAYBZCD";
        assert!(is_ulid(ulid));
        assert!(!is_ulid("01jx8z3q4r5s6t7v8w9xaybzcd"));
        assert!(!is_ulid("abc"));
        assert_eq!(short_id(ulid), "W9XAYBZCD"[1..]);
        assert_eq!(short_id("c_x1"), "c_x1");
        let object = json!({
            "id": ulid, "ok": true, "off": false, "n": 3, "list": ["a", "b"], "deep": [{"a": 1}],
            "obj": {"a": 1}, "text": "two\nlines\u{1b}[31m", "kind": "sales_invoice", "amount_minor": 1.5,
            "at": "2026-10-11T10:00:00Z",
        });
        let object = object.as_object().unwrap();
        let cell = |name, kind, short| {
            field_cell(
                &field(name, kind),
                object,
                &[],
                None,
                &context(false),
                short,
            )
            .plain()
        };
        assert_eq!(cell("id", FieldKind::Ulid, true), "8W9XAYBZCD"[2..]);
        assert_eq!(cell("id", FieldKind::Id, false), ulid);
        assert_eq!(cell("ok", FieldKind::Bool, false), "yes");
        assert_eq!(cell("off", FieldKind::Bool, false), "no");
        assert_eq!(cell("n", FieldKind::Number, false), "3");
        assert_eq!(cell("list", FieldKind::List, false), "a, b");
        assert_eq!(cell("deep", FieldKind::Json, false), "[1]");
        assert_eq!(cell("obj", FieldKind::Json, false), "{…}");
        assert_eq!(cell("text", FieldKind::Text, false), "two lines [31m");
        assert_eq!(
            cell("kind", FieldKind::Enum { values: &[] }, false),
            "sales invoice"
        );
        assert_eq!(
            cell(
                "amount_minor",
                FieldKind::Money {
                    currency_field: "currency"
                },
                false
            ),
            "1.5"
        );
        assert_eq!(cell("at", FieldKind::DateTime, false), "2h ago");
        assert_eq!(cell("missing", FieldKind::Text, false), "");
        assert_eq!(cell("n", FieldKind::Text, false), "3");
        assert_eq!(cell("n", FieldKind::Date, false), "3");
    }

    #[test]
    fn currency_from_the_scope() {
        let parent = json!({"base_currency": "EUR", "currency": "USD"});
        let parent = parent.as_object().unwrap();
        let row = json!({"amount_minor": 100});
        let row = row.as_object().unwrap();
        assert_eq!(currency("currency", row, &[parent]), Some("USD"));
        assert_eq!(currency("base_currency", row, &[parent]), Some("EUR"));
        assert_eq!(currency("orig_currency", row, &[]), None);
        let only_base = json!({"base_currency": "EUR"});
        assert_eq!(
            currency("currency", row, &[only_base.as_object().unwrap()]),
            Some("EUR")
        );
        let money = field(
            "amount_minor",
            FieldKind::Money {
                currency_field: "currency",
            },
        );
        assert_eq!(
            field_cell(&money, row, &[parent], None, &context(false), false).plain(),
            "1.00 USD"
        );
    }

    #[test]
    fn enum_words_bps_and_legal_forms() {
        // Self-test P2-6 and P2-10: `● 1 30`, `ou`, `10000`.
        assert_eq!(enum_words("1_30"), "1-30");
        assert_eq!(enum_words("over_90"), "over 90");
        assert_eq!(enum_words("rc_eu_goods"), "reverse charge EU goods");
        assert_eq!(enum_words("rc_non_eu"), "reverse charge non-EU");
        assert_eq!(enum_words("vat_input_rc"), "VAT input reverse charge");
        assert_eq!(enum_words("prof_fees"), "professional fees");
        assert_eq!(enum_words("non"), "non");
        assert_eq!(status("1_30", None, &context(false)).plain(), "● 1-30");
        assert_eq!(bps_text(10_000), "100%");
        assert_eq!(bps_text(2_450), "24.5%");
        assert_eq!(bps_text(5), "0.05%");
        assert_eq!(bps_text(-150), "-1.5%");
        let object =
            json!({"legal_form": "ou", "kind": "ou", "deductible_bps": 2200, "x_bps": 1.5});
        let object = object.as_object().unwrap();
        let cell = |name, kind| {
            field_cell(
                &field(name, kind),
                object,
                &[],
                None,
                &context(false),
                false,
            )
            .plain()
        };
        assert_eq!(cell("legal_form", FieldKind::Enum { values: &[] }), "OÜ");
        assert_eq!(cell("kind", FieldKind::Enum { values: &[] }), "ou");
        assert_eq!(cell("deductible_bps", FieldKind::Number), "22%");
        assert_eq!(cell("x_bps", FieldKind::Number), "1.5");
        let other = json!({"legal_form": "other"});
        assert_eq!(
            field_cell(
                &field("legal_form", FieldKind::Enum { values: &[] }),
                other.as_object().unwrap(),
                &[],
                None,
                &context(false),
                false
            )
            .plain(),
            "other"
        );
    }

    #[test]
    fn identifiers_are_never_cut() {
        let object =
            json!({"code": "EE-PUR-FOREIGN-VAT", "reference_number": "123456789", "name": "x"});
        let object = object.as_object().unwrap();
        for name in ["code", "reference_number"] {
            assert!(
                !field_cell(
                    &field(name, FieldKind::Text),
                    object,
                    &[],
                    None,
                    &context(false),
                    false
                )
                .truncate
            );
        }
        assert!(
            field_cell(
                &field("name", FieldKind::Text),
                object,
                &[],
                None,
                &context(false),
                false
            )
            .truncate
        );
    }

    #[test]
    fn the_lifecycle_state_says_more_than_the_settlement() {
        let status = Status {
            field: "settlement_state",
            roles: &[("unposted", "muted")],
        };
        let shown =
            |value: Value| status_value(value.as_object().unwrap(), &status).map(str::to_owned);
        assert_eq!(
            shown(json!({"settlement_state": "unposted", "state": "cancelled"})).as_deref(),
            Some("cancelled")
        );
        assert_eq!(
            shown(json!({"settlement_state": "unposted", "state": "issued"})).as_deref(),
            Some("issued")
        );
        assert_eq!(
            shown(json!({"settlement_state": "paid", "state": "posted"})).as_deref(),
            Some("paid")
        );
        assert_eq!(
            shown(json!({"settlement_state": "paid", "state": "void"})).as_deref(),
            Some("void")
        );
        assert_eq!(
            shown(json!({"state": "recorded"})).as_deref(),
            Some("recorded")
        );
        assert_eq!(
            shown(json!({"settlement_state": "paid"})).as_deref(),
            Some("paid")
        );
        let state = Status {
            field: "state",
            roles: &[],
        };
        assert_eq!(
            status_value(json!({"state": "x"}).as_object().unwrap(), &state),
            Some("x")
        );
    }

    #[test]
    fn cuts_cells() {
        let theme = Theme::PLAIN;
        let cell = Cell::text("Acme Holdings OÜ".into());
        assert_eq!(cell.truncated(8, &theme).plain(), "Acme Ho…");
        assert_eq!(cell.truncated(40, &theme).plain(), "Acme Holdings OÜ");
        let ascii = Theme {
            unicode: false,
            ..theme
        };
        assert_eq!(cell.truncated(8, &ascii).plain(), "Acme ...");
        assert_eq!(
            Cell::text("日本語のテキスト".into())
                .truncated(5, &theme)
                .plain(),
            "日本…"
        );
        let status = status("needs_review", None, &context(false));
        assert_eq!(status.truncated(6, &theme).plain(), "● nee…");
        assert_eq!(cut("abc", 0), "");
        assert_eq!(Cell::text("abc".into()).truncated(0, &theme).plain(), "");
    }

    #[test]
    fn days_sizes_english_names_and_snapshot_words() {
        // Self-test P2-6 (`DAYS OVERDUE  -9`), N3-16 (`SIZE BYTES 38307`) and N2-6 (Estonian
        // account names beside `account_name_en`).
        assert_eq!(days_overdue_text(10), "10");
        assert_eq!(days_overdue_text(0), "due today");
        assert_eq!(days_overdue_text(-9), "due in 9d");
        assert_eq!(bytes_text(38_307), "37.4 KB");
        assert_eq!(bytes_text(900), "900 B");
        assert_eq!(bytes_text(5 * 1024 * 1024), "5.0 MB");
        let object = json!({"days_overdue": -9, "size_bytes": 38_307, "account_name": "Nõuded ostjate vastu",
                            "account_name_en": "Trade receivables", "name": "Pank", "name_en": " "});
        let object = object.as_object().unwrap();
        let cell = |name, kind| {
            field_cell(
                &field(name, kind),
                object,
                &[],
                None,
                &context(false),
                false,
            )
            .plain()
        };
        assert_eq!(cell("days_overdue", FieldKind::Number), "due in 9d");
        assert_eq!(cell("size_bytes", FieldKind::Number), "37.4 KB");
        assert_eq!(cell("account_name", FieldKind::Text), "Trade receivables");
        assert_eq!(cell("name", FieldKind::Text), "Pank");
        let fields = [field("account_name", FieldKind::Text)];
        assert!(is_english_of_another("account_name_en", &fields));
        assert!(!is_english_of_another("name_en", &fields));
        assert_eq!(
            readable_text(&json!([{"iban": "EE12", "bic": null}, "x", {}])),
            "EE12; x"
        );
    }
}
