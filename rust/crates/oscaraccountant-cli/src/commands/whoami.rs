//! `oscar whoami`: `GET /api/me/`, with the default company of the profile.

use serde_json::{Map, Value, json};

use crate::company;
use crate::error::Result;
use crate::session::App;

/// Show the user, the credential, the organizations, the companies and the default company.
/// The companies refresh the cache of the profile.
pub async fn run(app: &mut App) -> Result<()> {
    let (client, selection) = app.client().await?;
    let me = company::fetch_me(&client).await?;
    if let Some(name) = &selection.name {
        app.store_companies(name, &selection.base_url, &company::from_me(&me))?;
    }
    if app.json() {
        app.out.json(&me);
        return Ok(());
    }
    let default = app
        .env
        .var("OSCAR_COMPANY")
        .map(str::to_owned)
        .or_else(|| selection.profile.as_ref()?.company.clone());
    let mut summary = summarize(&me, &selection.base_url, selection.name.as_deref());
    summary["company"] = json!(default_company(&me, default.as_deref()));
    let json = app.json();
    app.out.value(&summary, json);
    Ok(())
}

/// The default company for a person: `Name (id)`, or how to choose one.
pub fn default_company(me: &Value, default: Option<&str>) -> String {
    let companies = company::from_me(me);
    match default {
        Some(value) => match company::find(value, &companies) {
            company::Found::One(found) => format!("{} ({})", found.name, found.id),
            _ => format!("{value} (not one of your companies)"),
        },
        None => format!("none. {}", company::CHOOSE_HINT),
    }
}

/// The lines of `whoami` for a person.
pub fn summarize(me: &Value, base_url: &str, profile: Option<&str>) -> Value {
    let text = |value: &Value, key: &str| {
        value
            .get(key)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned()
    };
    let named = |list: Option<&Value>| {
        list.and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .map(|item| format!("{} ({})", text(item, "name"), text(item, "id")))
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_default()
    };
    let user = me.get("user").cloned().unwrap_or(Value::Null);
    let credential = me.get("credential").cloned().unwrap_or(Value::Null);
    let scopes = credential
        .get("scopes")
        .and_then(Value::as_array)
        .map(|scopes| {
            scopes
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(" ")
        })
        .unwrap_or_default();
    let mut summary = Map::new();
    summary.insert(
        "user".into(),
        json!(format!(
            "{} <{}>",
            text(&user, "name"),
            text(&user, "email")
        )),
    );
    summary.insert(
        "credential".into(),
        json!(format!("{} ({scopes})", text(&credential, "kind"))),
    );
    summary.insert(
        "organizations".into(),
        json!(named(me.get("organizations"))),
    );
    summary.insert("companies".into(), json!(named(me.get("companies"))));
    summary.insert("base_url".into(), json!(base_url));
    summary.insert("profile".into(), json!(profile));
    Value::Object(summary)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summarizes() {
        let me = json!({
            "user": {"name": "Ann", "email": "ann@example.com"},
            "credential": {"kind": "oauth", "scopes": ["read", "write"]},
            "organizations": [{"id": "o1", "name": "Org"}],
            "companies": [{"id": "c1", "name": "A"}, {"id": "c2", "name": "B"}],
        });
        assert_eq!(
            summarize(&me, "https://x.test", Some("default")),
            json!({
                "user": "Ann <ann@example.com>",
                "credential": "oauth (read write)",
                "organizations": "Org (o1)",
                "companies": "A (c1), B (c2)",
                "base_url": "https://x.test",
                "profile": "default",
            })
        );
        assert_eq!(default_company(&me, Some("c2")), "B (c2)");
        assert_eq!(
            default_company(&me, Some("zz")),
            "zz (not one of your companies)"
        );
        assert!(default_company(&me, None).starts_with("none. Give -c/--company"));
        let empty = summarize(&json!({}), "https://x.test", None);
        assert_eq!(empty["user"], " <>");
        assert_eq!(empty["profile"], Value::Null);
    }
}
