//! The companies of a login: the list of `GET
//! /api/me/`, its cache in the profile, and the resolution of `-c/--company <x>`.
//!
//! `<x>` is the sqid, the registry code, or a unique, case-insensitive prefix of the name. The
//! CLI reads the cache of the profile first; when `<x>` does not resolve there, it reads
//! `/api/me/` again and stores the fresh list.

use std::fmt;

use oscaraccountant::{Client, HttpMethod};
use serde_json::Value;

use crate::config::{Company, Config};
use crate::error::{CliError, Result};
use crate::render::theme::{self, Role, Sym};

/// The hint of a missing company.
pub const CHOOSE_HINT: &str =
    "Give -c/--company <x>, set OSCAR_COMPANY, or run `oscar companies use <x>`.";

/// The companies of an answer of `GET /api/me/`.
pub fn from_me(me: &Value) -> Vec<Company> {
    me.get("companies")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| serde_json::from_value::<Company>(item.clone()).ok())
                .collect()
        })
        .unwrap_or_default()
}

/// Read `GET /api/me/`.
pub async fn fetch_me(client: &Client) -> Result<Value> {
    Ok(client
        .request(HttpMethod::Get, "/api/me/")
        .send()
        .await?
        .json::<Value>()?)
}

/// What a value of `--company` names.
#[derive(Debug, PartialEq, Eq)]
pub enum Found<'a> {
    /// One company.
    One(&'a Company),
    /// Several companies: the value is ambiguous.
    Many(Vec<&'a Company>),
    /// No company.
    None,
}

/// Find the company of `value`: the sqid, then the registry code, then a case-insensitive prefix
/// of the name, then (when no name starts so) a part of the name. Among several names with the
/// prefix, the one equal to the value wins.
pub fn find<'a>(value: &str, companies: &'a [Company]) -> Found<'a> {
    let value = value.trim();
    if value.is_empty() {
        return Found::None;
    }
    if let Some(company) = companies.iter().find(|company| company.id == value) {
        return Found::One(company);
    }
    let by_code: Vec<&Company> = companies
        .iter()
        .filter(|company| !company.registry_code.is_empty() && company.registry_code == value)
        .collect();
    if let [one] = by_code.as_slice() {
        return Found::One(one);
    }
    let wanted = value.to_lowercase();
    let named: Vec<&Company> = companies
        .iter()
        .filter(|company| company.name.to_lowercase().starts_with(&wanted))
        .collect();
    let exact: Vec<&Company> = named
        .iter()
        .copied()
        .filter(|company| company.name.to_lowercase() == wanted)
        .collect();
    match (named.as_slice(), exact.as_slice()) {
        ([], _) if !by_code.is_empty() => Found::Many(by_code),
        ([], _) => {
            // `alpha` finds `E2E Alpha Holding OÜ`: a part of the name, when no name starts so.
            let inside: Vec<&Company> = companies
                .iter()
                .filter(|company| company.name.to_lowercase().contains(&wanted))
                .collect();
            match inside.as_slice() {
                [] => Found::None,
                [one] => Found::One(one),
                _ => Found::Many(inside),
            }
        }
        ([one], _) | (_, [one]) => Found::One(one),
        _ => Found::Many(named),
    }
}

/// The usage error of an ambiguous value, with one line per match.
pub fn ambiguous(value: &str, matches: &[&Company]) -> CliError {
    let mut error = CliError::usage(format!(
        "`{value}` matches {} companies. Give more of the name, the registry code or the id.",
        matches.len()
    ));
    error.details = matches.iter().map(|company| describe(company)).collect();
    error
}

/// The usage error of a value that names no company.
pub fn not_found(value: &str) -> CliError {
    CliError::usage(format!(
        "No company matches `{value}`. `oscar whoami` lists your companies."
    ))
}

/// One line for a company: `Name (registry code 12345678, id cmp1)`.
pub fn describe(company: &Company) -> String {
    let mut parts = Vec::new();
    if !company.registry_code.is_empty() {
        parts.push(format!("registry code {}", company.registry_code));
    }
    parts.push(format!("id {}", company.id));
    format!("{} ({})", company.name, parts.join(", "))
}

/// The cached companies of the profile `name`, when it has a cache.
pub fn cached<'a>(config: &'a Config, name: Option<&str>) -> Option<&'a [Company]> {
    config.profiles.get(name?)?.companies.as_deref()
}

/// The name of the company `id` in the cache of the profile, else the id.
pub fn label(config: &Config, profile: Option<&str>, id: &str) -> String {
    cached(config, profile)
        .and_then(|companies| companies.iter().find(|company| company.id == id))
        .map_or_else(|| id.to_owned(), |company| company.name.clone())
}

/// The success line of a write on a company: `<message> · <company> · <link>`.
pub fn success(message: String, company: Option<&str>, link: Option<&str>) -> String {
    let dot = theme::symbol(Sym::Middot);
    let mut line = message;
    if let Some(company) = company {
        line.push_str(&format!(" {dot} {company}"));
    }
    if let Some(link) = link {
        line.push_str(&format!(" {dot} {}", theme::link(link, link)));
    }
    line
}

/// A line of the company picker: the name, the registry code and the state, in columns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Choice {
    /// The company.
    pub company: Company,
    /// The width of the name column.
    name_width: usize,
    /// The width of the registry code column.
    code_width: usize,
}

/// The lines of the company picker, aligned in columns.
pub fn choices(companies: &[Company]) -> Vec<Choice> {
    let name_width = companies
        .iter()
        .map(|company| company.name.chars().count())
        .max()
        .unwrap_or(0);
    let code_width = companies
        .iter()
        .map(|company| company.registry_code.chars().count())
        .max()
        .unwrap_or(0);
    companies
        .iter()
        .map(|company| Choice {
            company: company.clone(),
            name_width,
            code_width,
        })
        .collect()
}

impl fmt::Display for Choice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let company = &self.company;
        let mut line = format!("{:<width$}", company.name, width = self.name_width);
        if self.code_width > 0 {
            line.push_str(&format!(
                "  {:<width$}",
                company.registry_code,
                width = self.code_width
            ));
        }
        if company.state.is_empty() {
            return f.write_str(line.trim_end());
        }
        let role = match company.state.as_str() {
            "ready" => Role::Ok,
            "failed" => Role::Bad,
            _ => Role::Warn,
        };
        write!(f, "{line}  {}", theme::paint(role, &company.state))
    }
}

#[cfg(test)]
pub mod tests {
    use serde_json::json;

    use super::*;

    pub fn company(id: &str, name: &str, code: &str) -> Company {
        Company {
            id: id.into(),
            name: name.into(),
            registry_code: code.into(),
            state: "ready".into(),
            organization_id: "org1".into(),
        }
    }

    fn companies() -> Vec<Company> {
        vec![
            company("cmp1", "Alpha OÜ", "11111111"),
            company("cmp2", "Alpha Holding OÜ", "22222222"),
            company("cmp3", "Beta AS", "33333333"),
            company("cmp4", "Õun OÜ", ""),
            company("cmp5", "Alpha", "55555555"),
        ]
    }

    fn one(value: &str) -> Option<String> {
        match find(value, &companies()) {
            Found::One(company) => Some(company.id.clone()),
            _ => None,
        }
    }

    #[test]
    fn resolves_an_id_a_registry_code_and_a_name() {
        assert_eq!(one("cmp3").as_deref(), Some("cmp3"));
        assert_eq!(one("22222222").as_deref(), Some("cmp2"));
        assert_eq!(one("beta").as_deref(), Some("cmp3"));
        assert_eq!(one("  BETA as ").as_deref(), Some("cmp3"));
        assert_eq!(one("alpha h").as_deref(), Some("cmp2"));
        assert_eq!(one("õun").as_deref(), Some("cmp4"));
        // The name equal to the value wins over the longer names with that prefix.
        assert_eq!(one("alpha").as_deref(), Some("cmp5"));
    }

    #[test]
    fn an_ambiguous_or_unknown_value() {
        let list = companies();
        let Found::Many(matches) = find("alp", &list) else {
            panic!("ambiguous");
        };
        assert_eq!(matches.len(), 3);
        let error = ambiguous("alp", &matches);
        assert_eq!(error.code, crate::error::EXIT_USAGE);
        assert!(error.message.starts_with("`alp` matches 3 companies."));
        assert_eq!(
            error.details[0],
            "Alpha OÜ (registry code 11111111, id cmp1)"
        );
        assert_eq!(find("gamma", &list), Found::None);
        assert_eq!(find("", &list), Found::None);
        assert!(not_found("gamma").message.contains("oscar whoami"));
        assert_eq!(describe(&company("c", "N", "")), "N (id c)");
        let twins = vec![company("a", "A", "1"), company("b", "B", "1")];
        assert!(matches!(find("1", &twins), Found::Many(found) if found.len() == 2));
    }

    #[test]
    fn reads_me_and_the_cache() {
        let me = json!({"companies": [
            {"id": "c1", "name": "A", "registry_code": "1", "state": "ready", "organization_id": "o1", "vat_number": "EE1"},
            {"name": "no id"},
        ]});
        let list = from_me(&me);
        assert_eq!(
            list,
            vec![Company {
                id: "c1".into(),
                name: "A".into(),
                registry_code: "1".into(),
                state: "ready".into(),
                organization_id: "o1".into(),
            }]
        );
        assert!(from_me(&json!({})).is_empty());
        let mut config = Config::default();
        assert_eq!(cached(&config, Some("p")), None);
        assert_eq!(label(&config, Some("p"), "c1"), "c1");
        config.profiles.insert(
            "p".into(),
            crate::config::Profile {
                base_url: "https://a.test".into(),
                auth: crate::config::AuthKind::Oauth,
                organization: None,
                company: None,
                companies: Some(list),
                extra: serde_json::Map::new(),
            },
        );
        assert_eq!(cached(&config, None), None);
        assert_eq!(cached(&config, Some("p")).unwrap().len(), 1);
        assert_eq!(label(&config, Some("p"), "c1"), "A");
        assert_eq!(label(&config, Some("p"), "c2"), "c2");
    }

    #[test]
    fn the_success_line_names_the_company() {
        let _plain = theme::scoped(theme::Theme::PLAIN);
        assert_eq!(
            success(
                "Issued invoice 1".into(),
                Some("Alpha OÜ"),
                Some("https://x.test/c/1")
            ),
            "Issued invoice 1 · Alpha OÜ · https://x.test/c/1"
        );
        assert_eq!(success("Done".into(), None, None), "Done");
    }

    #[test]
    fn a_choice_shows_the_name_the_code_and_the_state_in_columns() {
        let _plain = theme::scoped(theme::Theme::PLAIN);
        let mut alpha = company("c1", "Alpha OÜ", "11111111");
        let long = company("c2", "E2E Alpha Holding OÜ", "14999001");
        let lines: Vec<String> = choices(&[alpha.clone(), long])
            .iter()
            .map(ToString::to_string)
            .collect();
        assert_eq!(lines[0], "Alpha OÜ              11111111  ready");
        assert_eq!(lines[1], "E2E Alpha Holding OÜ  14999001  ready");
        alpha.state = "provisioning".into();
        alpha.registry_code.clear();
        assert_eq!(
            choices(std::slice::from_ref(&alpha))[0].to_string(),
            "Alpha OÜ  provisioning"
        );
        alpha.state = "failed".into();
        assert!(
            choices(std::slice::from_ref(&alpha))[0]
                .to_string()
                .ends_with("failed")
        );
        alpha.state.clear();
        assert_eq!(choices(&[alpha])[0].to_string(), "Alpha OÜ");
    }

    #[test]
    fn a_part_of_the_name_when_no_name_starts_so() {
        let list = vec![
            company("c1", "E2E Alpha Holding OÜ", "1"),
            company("c2", "E2E Alpha Trading OÜ", "2"),
            company("c3", "Beta AS", "3"),
        ];
        assert!(matches!(find("trading", &list), Found::One(found) if found.id == "c2"));
        assert!(matches!(find("alpha", &list), Found::Many(found) if found.len() == 2));
        assert_eq!(find("gamma", &list), Found::None);
        // A prefix wins over a part.
        assert!(matches!(find("e2e alpha h", &list), Found::One(found) if found.id == "c1"));
    }
}
