//! A view guessed from a JSON value, for an answer with no generated view: a hand-written
//! command (`oscar whoami`, `oscar auth status`), or `oscar api`.
//!
//! A view holds `'static` references, like the generated ones. A guessed view is built once per
//! run and leaked: a few hundred bytes for the life of a short process.

use serde_json::{Map, Value};

use super::format::is_ulid;
use crate::generated::{FieldKind, View, ViewField};
use crate::time;

/// The most columns of a guessed list.
const MAX_COLUMNS: usize = 8;

fn leak(text: &str) -> &'static str {
    Box::leak(text.to_owned().into_boxed_str())
}

/// `due_date` is `Due date`.
pub fn label(name: &str) -> String {
    let mut words: Vec<String> = name
        .split('_')
        .filter(|word| !word.is_empty())
        .map(|word| match word {
            "id" => "ID".to_owned(),
            "url" => "URL".to_owned(),
            "vat" => "VAT".to_owned(),
            "iban" => "IBAN".to_owned(),
            "bic" => "BIC".to_owned(),
            "eu" => "EU".to_owned(),
            "reg" => "registry".to_owned(),
            other => other.to_owned(),
        })
        .collect();
    if words.len() > 1 && words.last().is_some_and(|word| word == "minor") {
        words.pop();
    }
    let mut text = words.join(" ");
    if let Some(first) = text.get(..1) {
        text = first.to_uppercase() + &text[1..];
    }
    text
}

fn is_date(text: &str) -> bool {
    text.len() == 10 && time::parse(&format!("{text}T00:00:00Z")).is_some()
}

/// The kind of a field from its name and one of its values.
fn kind(name: &str, value: &Value, siblings: &Map<String, Value>) -> FieldKind {
    match value {
        Value::Object(object) => FieldKind::Object(infer_object(std::slice::from_ref(&object))),
        Value::Array(items) if items.iter().any(Value::is_object) => {
            let objects: Vec<&Map<String, Value>> =
                items.iter().filter_map(Value::as_object).collect();
            FieldKind::Array(infer_object(&objects))
        }
        Value::Array(_) => FieldKind::List,
        Value::Bool(_) => FieldKind::Bool,
        Value::Number(_) if name.ends_with("_minor") => FieldKind::Money {
            currency_field: if name.contains("base") && !siblings.contains_key("currency") {
                "base_currency"
            } else {
                "currency"
            },
        },
        Value::Number(_) => FieldKind::Number,
        Value::String(text) if name == "id" && is_ulid(text) => FieldKind::Ulid,
        Value::String(_) if name == "id" => FieldKind::Id,
        Value::String(_) if name.ends_with("_id") => FieldKind::Ref,
        Value::String(text) if is_date(text) => FieldKind::Date,
        Value::String(text) if text.len() > 10 && time::parse(text).is_some() => {
            FieldKind::DateTime
        }
        _ => FieldKind::Text,
    }
}

/// The view of a list of objects (or one object): the keys in the order they first appear.
pub fn infer_object(items: &[&Map<String, Value>]) -> &'static View {
    let mut fields: Vec<ViewField> = Vec::new();
    for item in items {
        for (name, value) in *item {
            if value.is_null() || fields.iter().any(|field| field.name == name) {
                continue;
            }
            fields.push(ViewField {
                name: leak(name),
                label: leak(&label(name)),
                kind: kind(name, value, item),
                nullable: true,
            });
        }
    }
    let title = ["name", "title", "number", "label", "email"]
        .into_iter()
        .find(|candidate| fields.iter().any(|field| field.name == *candidate))
        .map_or("", |name| {
            fields
                .iter()
                .find(|field| field.name == name)
                .map_or("", |field| field.name)
        });
    let status = ["status", "state"]
        .into_iter()
        .find_map(|candidate| {
            fields
                .iter()
                .find(|field| field.name == candidate && field.kind == FieldKind::Text)
        })
        .map(|field| crate::generated::Status {
            field: field.name,
            roles: &[],
        });
    let columns: Vec<&'static str> = fields
        .iter()
        .filter(|field| {
            !matches!(
                field.kind,
                FieldKind::Object(_) | FieldKind::Array(_) | FieldKind::List | FieldKind::Json
            )
        })
        .map(|field| field.name)
        .take(MAX_COLUMNS)
        .collect();
    Box::leak(Box::new(View {
        name: "Inferred",
        title,
        fields: Box::leak(fields.into_boxed_slice()),
        columns: Box::leak(columns.into_boxed_slice()),
        wide: &[],
        status,
        hidden: &[],
    }))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn guesses_the_kinds() {
        let value = json!({
            "id": "01JX8Z3Q4R5S6T7V8W9XAYBZCD", "name": "Acme", "company_id": "c1", "state": "active",
            "day": "2026-10-11", "at": "2026-10-11T10:00:00Z", "gross_minor": 100, "currency": "EUR",
            "base_total_minor": 5, "n": 2, "ok": true, "tags": ["a"], "nested": {"a": 1},
            "rows": [{"x": 1}], "nothing": null,
        });
        let view = infer_object(&[value.as_object().unwrap()]);
        let kind = |name| view.field(name).unwrap().kind;
        assert_eq!(kind("id"), FieldKind::Ulid);
        assert_eq!(kind("company_id"), FieldKind::Ref);
        assert_eq!(kind("day"), FieldKind::Date);
        assert_eq!(kind("at"), FieldKind::DateTime);
        assert_eq!(
            kind("gross_minor"),
            FieldKind::Money {
                currency_field: "currency"
            }
        );
        assert_eq!(kind("n"), FieldKind::Number);
        assert_eq!(kind("ok"), FieldKind::Bool);
        assert_eq!(kind("tags"), FieldKind::List);
        assert!(matches!(kind("nested"), FieldKind::Object(_)));
        assert!(matches!(kind("rows"), FieldKind::Array(_)));
        assert!(view.field("nothing").is_none());
        assert_eq!((view.title, view.status.unwrap().field), ("name", "state"));
        assert!(!view.columns.contains(&"nested"));
        assert_eq!(view.field("gross_minor").unwrap().label, "Gross");
        let other = json!({"id": "c1", "total_base_minor": 1});
        let view = infer_object(&[other.as_object().unwrap()]);
        assert_eq!(view.field("id").unwrap().kind, FieldKind::Id);
        assert_eq!(
            view.field("total_base_minor").unwrap().kind,
            FieldKind::Money {
                currency_field: "base_currency"
            }
        );
        assert_eq!(label("vat_id_url"), "VAT ID URL");
        assert_eq!(label(""), "");
    }
}
