//! The output for a program: `--json a,b` (selected fields), `--jq <expr>` (jaq), `--template`
//! (minijinja) and `-o ndjson`. None of them ever writes a colour code.

use serde_json::{Map, Value};

use super::list::unknown_field;
use crate::error::{CliError, Result};
use crate::generated::View;

/// The value with only `fields` in each object: the items of a page, each item of a list, or
/// the object itself. An unknown name fails with suggestions from the view, else from the value.
pub fn select(value: &Value, fields: &[String], view: Option<&View>) -> Result<Value> {
    let sample = first_object(value);
    for name in fields {
        let known_in_view = view.is_some_and(|view| view.field(name).is_some());
        let known_in_value = sample.is_some_and(|object| object.contains_key(name));
        if !known_in_view && !known_in_value {
            let names: Vec<&str> = match (view, sample) {
                (Some(view), _) => view.fields.iter().map(|field| field.name).collect(),
                (None, Some(object)) => object.keys().map(String::as_str).collect(),
                (None, None) => Vec::new(),
            };
            if view.is_some() || sample.is_some() {
                return Err(unknown_field(name, names.into_iter()));
            }
        }
    }
    let pick = |object: &Map<String, Value>| -> Value {
        Value::Object(
            fields
                .iter()
                .map(|name| {
                    (
                        name.clone(),
                        object.get(name).cloned().unwrap_or(Value::Null),
                    )
                })
                .collect(),
        )
    };
    Ok(match value {
        Value::Object(object) if is_page(object) => {
            let mut page = object.clone();
            if let Some(Value::Array(items)) = object.get("items") {
                page.insert(
                    "items".into(),
                    Value::Array(
                        items
                            .iter()
                            .map(|item| item.as_object().map_or(item.clone(), pick))
                            .collect(),
                    ),
                );
            }
            Value::Object(page)
        }
        Value::Object(object) => pick(object),
        Value::Array(items) => Value::Array(
            items
                .iter()
                .map(|item| item.as_object().map_or(item.clone(), pick))
                .collect(),
        ),
        other => other.clone(),
    })
}

/// A page of a list: `items` and `next_cursor`.
pub fn is_page(object: &Map<String, Value>) -> bool {
    object.contains_key("next_cursor") && object.get("items").is_some_and(Value::is_array)
}

fn first_object(value: &Value) -> Option<&Map<String, Value>> {
    match value {
        Value::Object(object) if is_page(object) => object
            .get("items")
            .and_then(Value::as_array)
            .and_then(|items| items.iter().find_map(Value::as_object)),
        Value::Object(object) => Some(object),
        Value::Array(items) => items.iter().find_map(Value::as_object),
        _ => None,
    }
}

/// The lines of `--json a,b`: the names, split on commas, without blanks.
pub fn field_list(text: &str) -> Vec<String> {
    text.split(',')
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
        .collect()
}

/// The output of a jq expression on the value, one line per result. A string result is
/// printed as it is, like `gh --jq`.
pub fn jq(expression: &str, value: &Value) -> Result<Vec<String>> {
    use jaq_core::load::{Arena, File, Loader};
    use jaq_core::{Compiler, Ctx, Vars, data, unwrap_valr};
    use jaq_json::Val;

    let formats = jaq_core::load::parse(FORMATS, |parser| parser.defs()).unwrap_or_default();
    let defs = jaq_core::defs()
        .chain(jaq_std::defs())
        .chain(jaq_json::defs())
        .chain(formats);
    let funs = jaq_core::funs()
        .chain(jaq_std::funs())
        .chain(jaq_json::funs());
    let arena = Arena::default();
    let program = File {
        code: expression,
        path: (),
    };
    let invalid = |why: String| {
        CliError::usage(format!(
            "--jq: `{expression}` is not a valid jq expression: {why}."
        ))
    };
    let modules = Loader::new(defs).load(&arena, program).map_err(|errors| {
        use jaq_core::load::Error;
        let why = errors.into_iter().find_map(|(_, error)| match error {
            Error::Io(found) => found.into_iter().next().map(|(_, why)| why),
            Error::Lex(found) => found
                .into_iter()
                .next()
                .map(|(expect, at)| expected(expect.as_str(), at)),
            Error::Parse(found) => found
                .into_iter()
                .next()
                .map(|(expect, at)| expected(expect.as_str(), at)),
        });
        invalid(why.unwrap_or_else(|| "it does not parse".to_owned()))
    })?;
    let filter = Compiler::default()
        .with_funs(funs)
        .compile(modules)
        .map_err(|errors| {
            let why = errors
                .into_iter()
                .flat_map(|(_, found)| found)
                .next()
                .map(|(name, undefined)| format!("no {} `{name}`", undefined.as_str()));
            invalid(why.unwrap_or_else(|| "it does not compile".to_owned()))
        })?;
    let bytes = serde_json::to_vec(value).unwrap_or_default();
    let input = jaq_json::read::parse_single(&bytes)
        .map_err(|error| CliError::usage(format!("--jq: cannot read the answer: {error}")))?;
    let context = Ctx::<data::JustLut<Val>>::new(&filter.lut, Vars::new([]));
    let mut lines = Vec::new();
    for result in filter.id.run((context, input)).map(unwrap_valr) {
        match result {
            Ok(Val::TStr(text)) => lines.push(String::from_utf8_lossy(&text).into_owned()),
            Ok(other) => lines.push(other.to_string()),
            Err(error) => return Err(CliError::usage(format!("--jq: {error}"))),
        }
    }
    Ok(lines)
}

/// The formats of jq that jaq does not have: `@csv` and `@tsv` (an array of scalars to one line).
const FORMATS: &str = r#"
def @csv: if isarray then . else error("@csv takes an array") end
  | map(if isstring then "\"" + (split("\"") | join("\"\"")) + "\""
        elif . == null then ""
        elif isboolean or isnumber then tojson
        else error("@csv takes an array of scalars") end)
  | join(",");
def @tsv: if isarray then . else error("@tsv takes an array") end
  | map(if isstring then split("\\") | join("\\\\") | split("\t") | join("\\t")
          | split("\n") | join("\\n") | split("\r") | join("\\r")
        elif . == null then ""
        elif isboolean or isnumber then tojson
        else error("@tsv takes an array of scalars") end)
  | join("\t");
"#;

/// Where a jq expression stops parsing: `expected a term at `| .x``.
fn expected(what: &str, at: &str) -> String {
    let at = super::format::cut(&super::format::one_line(at), 20);
    if at.is_empty() {
        format!("expected {what} at the end")
    } else {
        format!("expected {what} at `{at}`")
    }
}

/// The value through a minijinja template. An object gives its keys as variables; a list is
/// `items`. The whole value is also `value`.
///
/// A template on a page or a list that names none of `items`, `next_cursor` and `value` is a
/// template of one item (`--template '{{ number }} {{ state }}'`): it runs once per item, one
/// line each.
pub fn template(source: &str, value: &Value) -> Result<String> {
    let mut environment = minijinja::Environment::new();
    environment.set_undefined_behavior(minijinja::UndefinedBehavior::Chainable);
    let template = environment
        .template_from_str(source)
        .map_err(|error| CliError::usage(format!("--template: {error}")))?;
    let items = match value {
        Value::Object(object) if is_page(object) => object.get("items").and_then(Value::as_array),
        Value::Array(items) => Some(items),
        _ => None,
    };
    let names = template.undeclared_variables(false);
    if let Some(items) = items
        && !["items", "next_cursor", "value"]
            .iter()
            .any(|name| names.contains(*name))
    {
        let mut lines = Vec::with_capacity(items.len());
        for item in items {
            lines.push(render(&template, item)?.trim_end_matches('\n').to_owned());
        }
        return Ok(lines.join("\n"));
    }
    render(&template, value)
}

fn render(template: &minijinja::Template<'_, '_>, value: &Value) -> Result<String> {
    let mut context = match value {
        Value::Object(object) => object.clone(),
        Value::Array(items) => Map::from_iter([("items".to_owned(), Value::Array(items.clone()))]),
        _ => Map::new(),
    };
    context.insert("value".into(), value.clone());
    template
        .render(Value::Object(context))
        .map_err(|error| CliError::usage(format!("--template: {error}")))
}

/// The lines of `-o ndjson`: one per item of a page or a list, else the value itself.
pub fn ndjson(value: &Value) -> Vec<String> {
    let items: Vec<&Value> = match value {
        Value::Object(object) if is_page(object) => object
            .get("items")
            .and_then(Value::as_array)
            .map(|items| items.iter().collect())
            .unwrap_or_default(),
        Value::Array(items) => items.iter().collect(),
        other => vec![other],
    };
    items
        .into_iter()
        .map(|item| serde_json::to_string(item).unwrap_or_default())
        .collect()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::generated::VIEW_BOOKS_INVOICE;

    #[test]
    fn selects_fields() {
        let page = json!({"items": [{"id": "a", "number": "1", "x": 2}, 3], "next_cursor": null});
        let fields = field_list("id, number,,");
        assert_eq!(fields, ["id", "number"]);
        assert_eq!(
            select(&page, &fields, Some(&VIEW_BOOKS_INVOICE)).unwrap(),
            json!({"items": [{"id": "a", "number": "1"}, 3], "next_cursor": null})
        );
        assert_eq!(
            select(&json!({"a": 1, "b": 2}), &["b".into()], None).unwrap(),
            json!({"b": 2})
        );
        assert_eq!(
            select(&json!([{"a": 1}]), &["a".into()], None).unwrap(),
            json!([{"a": 1}])
        );
        assert_eq!(select(&json!(3), &["a".into()], None).unwrap(), json!(3));
        let error = select(&json!({"amount": 1}), &["amont".into()], None).unwrap_err();
        assert!(error.details[0].contains("amount"));
        let error = select(&page, &["nmber".into()], Some(&VIEW_BOOKS_INVOICE)).unwrap_err();
        assert!(error.details[0].contains("number"));
        // A field of the view that this item lacks is null.
        assert_eq!(
            select(
                &json!({"id": "a"}),
                &["due_date".into()],
                Some(&VIEW_BOOKS_INVOICE)
            )
            .unwrap(),
            json!({"due_date": null})
        );
    }

    #[test]
    fn runs_jq() {
        let page = json!({"items": [{"number": "1", "n": 2}, {"number": "2", "n": 3}], "next_cursor": null});
        assert_eq!(jq(".items[].number", &page).unwrap(), ["1", "2"]);
        assert_eq!(jq("[.items[].n] | add", &page).unwrap(), ["5"]);
        assert_eq!(jq(".items[0]", &page).unwrap(), [r#"{"number":"1","n":2}"#]);
        assert_eq!(jq(".items | length", &page).unwrap(), ["2"]);
        assert!(
            jq(".items[", &page)
                .unwrap_err()
                .message
                .contains("not a valid jq")
        );
        assert!(
            jq("nosuchfunction", &page)
                .unwrap_err()
                .message
                .contains("not a valid jq")
        );
        assert!(
            jq("error(\"boom\")", &page)
                .unwrap_err()
                .message
                .contains("boom")
        );
    }

    #[test]
    fn renders_templates() {
        let value = json!({"number": "2026-1", "items": [1, 2]});
        assert_eq!(
            template("{{ number }}: {{ items | length }}", &value).unwrap(),
            "2026-1: 2"
        );
        assert_eq!(
            template("{% for x in items %}{{ x }};{% endfor %}", &json!([1, 2])).unwrap(),
            "1;2;"
        );
        assert_eq!(template("{{ value }}", &json!(3)).unwrap(), "3");
        assert_eq!(template("{{ missing.deeper }}", &value).unwrap(), "");
        assert!(
            template("{{ broken", &value)
                .unwrap_err()
                .message
                .starts_with("--template")
        );
    }

    #[test]
    fn a_template_of_one_item_runs_per_item_of_a_list() {
        // Self-test P2-12: the README form printed an empty line.
        let page = json!({"items": [{"number": "A", "state": "issued"}, {"number": "B", "state": "posted"}],
                          "next_cursor": null});
        assert_eq!(
            template("{{ number }} {{ state }}\n", &page).unwrap(),
            "A issued\nB posted"
        );
        assert_eq!(
            template("{{ x }}", &json!([{"x": 1}, {"x": 2}])).unwrap(),
            "1\n2"
        );
        assert_eq!(
            template("{{ items | length }} {{ next_cursor }}", &page).unwrap(),
            "2 None"
        );
        assert!(
            template("{{ number | nope }}", &page)
                .unwrap_err()
                .message
                .starts_with("--template")
        );
    }

    #[test]
    fn writes_ndjson() {
        let page = json!({"items": [{"a": 1}, {"a": 2}], "next_cursor": "c"});
        assert_eq!(ndjson(&page), [r#"{"a":1}"#, r#"{"a":2}"#]);
        assert_eq!(ndjson(&json!([1, 2])), ["1", "2"]);
        assert_eq!(ndjson(&json!({"a": 1})), [r#"{"a":1}"#]);
    }

    #[test]
    fn csv_tsv_and_why_an_expression_fails() {
        // Self-test P2-14: `@tsv` and `@csv` were "not a valid jq expression", with no reason.
        let page = json!({"items": [{"number": "EE-1", "name": "A \"B\", C", "gross": 12.5, "x": null},
                                    {"number": "EE-2", "name": "tab\there\nnew\\line", "gross": 3, "x": true}]});
        assert_eq!(
            jq(".items[] | [.number, .name, .gross, .x] | @csv", &page).unwrap(),
            [
                r#""EE-1","A ""B"", C",12.5,"#,
                r#""EE-2","tab	here
new\line",3,true"#
            ]
        );
        assert_eq!(
            jq(".items[] | [.number, .name, .x] | @tsv", &page).unwrap(),
            ["EE-1\tA \"B\", C\t", "EE-2\ttab\\there\\nnew\\\\line\ttrue"]
        );
        let error = jq(".items | @csv", &json!({"items": {"a": 1}})).unwrap_err();
        assert!(
            error.message.contains("@csv takes an array"),
            "{}",
            error.message
        );
        let error = jq("[{}] | @tsv", &json!(null)).unwrap_err();
        assert!(error.message.contains("scalars"), "{}", error.message);
        let error = jq(".items[", &page).unwrap_err();
        assert!(error.message.contains(": expected"), "{}", error.message);
        let error = jq("nosuchfunction", &page).unwrap_err();
        assert!(
            error.message.ends_with("no filter `nosuchfunction`."),
            "{}",
            error.message
        );
        assert_eq!(expected("a term", ""), "expected a term at the end");
    }
}
