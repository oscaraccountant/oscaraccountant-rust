//! Step 2, "About your business": the questions of the AML questionnaire (internal rules 6.9),
//! in the order of the web form, and the answers given as flags or in a file.

use serde_json::{Map, Value, json};

use super::api::QUESTIONNAIRE_FIELDS;
use crate::error::{CliError, Result};

/// The kind of an answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Free text.
    Text,
    /// ISO 3166-1 alpha-2 country codes.
    Countries,
    /// Yes or no.
    YesNo,
    /// One of these values, each with its label.
    Choice(&'static [(&'static str, &'static str)]),
}

/// One question.
#[derive(Debug, Clone, Copy)]
pub struct Question {
    /// The field of the questionnaire.
    pub key: &'static str,
    /// The question, as the web form asks it.
    pub label: &'static str,
    /// The kind of the answer.
    pub kind: Kind,
}

/// The expected turnover.
pub const TURNOVER: &[(&str, &str)] = &[
    ("under_50k", "Under 50 000 EUR"),
    ("50k_200k", "50 000 to 200 000 EUR"),
    ("200k_1m", "200 000 to 1 million EUR"),
    ("over_1m", "Over 1 million EUR"),
];

/// The use of cash.
pub const CASH: &[(&str, &str)] = &[("no", "No"), ("a_little", "A little"), ("often", "Often")];

/// Where the board members and the owners live.
pub const RESIDENCE: &[(&str, &str)] = &[
    ("all_estonia", "All in Estonia"),
    ("some_abroad", "Some live abroad"),
];

/// The years in the field.
pub const EXPERIENCE: &[(&str, &str)] = &[
    ("under_1", "Under 1 year"),
    ("1_3", "1 to 3 years"),
    ("3_10", "3 to 10 years"),
    ("over_10", "Over 10 years"),
];

/// Every question, in the order of the form. [`asked`] says which apply.
pub const QUESTIONS: &[Question] = &[
    Question {
        key: "activity_description",
        label: "What does the business do?",
        kind: Kind::Text,
    },
    Question {
        key: "customer_countries",
        label: "Where are your main customers? (country codes, for example EE, FI)",
        kind: Kind::Countries,
    },
    Question {
        key: "supplier_countries",
        label: "Where are your main suppliers? (country codes, for example EE, DE)",
        kind: Kind::Countries,
    },
    Question {
        key: "expected_turnover",
        label: "Expected turnover this year",
        kind: Kind::Choice(TURNOVER),
    },
    Question {
        key: "cash_usage",
        label: "Does the business take or pay cash?",
        kind: Kind::Choice(CASH),
    },
    Question {
        key: "uses_crypto_assets",
        label: "Does it hold, accept or pay with crypto-assets?",
        kind: Kind::YesNo,
    },
    Question {
        key: "needs_licence",
        label: "Does it need a licence?",
        kind: Kind::YesNo,
    },
    Question {
        key: "licence_description",
        label: "Which licence?",
        kind: Kind::Text,
    },
    Question {
        key: "residence",
        label: "Where do the board members and the owners live?",
        kind: Kind::Choice(RESIDENCE),
    },
    Question {
        key: "residence_countries",
        label: "In which countries? (country codes)",
        kind: Kind::Countries,
    },
    Question {
        key: "e_resident",
        label: "Is someone who lives abroad an e-resident of Estonia?",
        kind: Kind::YesNo,
    },
    Question {
        key: "estonia_reason",
        label: "Why is the company in Estonia?",
        kind: Kind::Text,
    },
    Question {
        key: "startup_funds_source",
        label: "Where did the start-up money come from?",
        kind: Kind::Text,
    },
    Question {
        key: "acts_for_others",
        label: "Does anyone act for, or on behalf of, someone else?",
        kind: Kind::YesNo,
    },
    Question {
        key: "acts_for_others_reason",
        label: "Why?",
        kind: Kind::Text,
    },
    Question {
        key: "field_experience",
        label: "How long have you worked in this field?",
        kind: Kind::Choice(EXPERIENCE),
    },
];

/// The question of `key`.
pub fn question(key: &str) -> Option<&'static Question> {
    QUESTIONS.iter().find(|question| question.key == key)
}

/// Whether `key` is asked, after these answers. `server` is the `asked` object of the server,
/// for the questions it decides (11, why Estonia; 12, the start-up money).
pub fn asked(key: &str, answers: &Map<String, Value>, server: &Value) -> bool {
    let yes = |field: &str| answers.get(field) == Some(&Value::Bool(true));
    let abroad = answers.get("residence").and_then(Value::as_str) == Some("some_abroad");
    match key {
        "licence_description" => yes("needs_licence"),
        "residence_countries" | "e_resident" => abroad,
        "acts_for_others_reason" => yes("acts_for_others"),
        "estonia_reason" | "startup_funds_source" => {
            server.get(key).and_then(Value::as_bool).unwrap_or(false)
        }
        _ => true,
    }
}

/// The answer is given: not null, not an empty text, not an empty list.
pub fn answered(value: Option<&Value>) -> bool {
    match value {
        None | Some(Value::Null) => false,
        Some(Value::String(text)) => !text.trim().is_empty(),
        Some(Value::Array(items)) => !items.is_empty(),
        Some(_) => true,
    }
}

/// Check a list of country codes and give them in upper case.
pub fn countries(text: &str) -> std::result::Result<Vec<String>, String> {
    let codes: Vec<String> = text
        .split([',', ' '])
        .map(str::trim)
        .filter(|code| !code.is_empty())
        .map(str::to_ascii_uppercase)
        .collect();
    if codes.is_empty() {
        return Err("Give at least one country code, for example EE.".into());
    }
    if let Some(bad) = codes
        .iter()
        .find(|code| code.len() != 2 || !code.chars().all(|ch| ch.is_ascii_uppercase()))
    {
        return Err(format!(
            "`{bad}` is not a country code: use two letters, for example EE or FI."
        ));
    }
    Ok(codes)
}

/// Yes or no, as a person writes it.
pub fn yes_no(text: &str) -> Option<bool> {
    match text.trim().to_ascii_lowercase().as_str() {
        "yes" | "y" | "true" => Some(true),
        "no" | "n" | "false" => Some(false),
        _ => None,
    }
}

/// The value of an answer given as text (`--answer key=value`), checked.
pub fn parse(key: &str, text: &str) -> Result<Value> {
    let Some(question) = question(key) else {
        return Err(CliError::usage(format!(
            "`{key}` is not a question of the questionnaire. The questions: {}.",
            QUESTIONNAIRE_FIELDS.join(", ")
        )));
    };
    match question.kind {
        Kind::Text => Ok(json!(text.trim())),
        Kind::Countries => countries(text)
            .map(|codes| json!(codes))
            .map_err(|message| CliError::usage(format!("{key}: {message}"))),
        Kind::YesNo => yes_no(text)
            .map(Value::Bool)
            .ok_or_else(|| CliError::usage(format!("{key}: answer yes or no."))),
        Kind::Choice(values) => {
            if values.iter().any(|(value, _)| *value == text.trim()) {
                Ok(json!(text.trim()))
            } else {
                let names: Vec<&str> = values.iter().map(|(value, _)| *value).collect();
                Err(CliError::usage(format!(
                    "{key}: `{text}` is not a value; use one of {}.",
                    names.join(", ")
                )))
            }
        }
    }
}

/// The answers of `--answers <file.json>`: a JSON object with fields of the questionnaire.
pub fn from_file(text: &str) -> Result<Map<String, Value>> {
    let value: Value = serde_json::from_str(text)
        .map_err(|error| CliError::usage(format!("--answers is not valid JSON: {error}")))?;
    let Value::Object(map) = value else {
        return Err(CliError::usage(
            "--answers is a JSON object of the answers, for example {\"cash_usage\": \"no\"}.",
        ));
    };
    for (key, value) in &map {
        if !QUESTIONNAIRE_FIELDS.contains(&key.as_str()) {
            return Err(CliError::usage(format!(
                "--answers: `{key}` is not a question of the questionnaire."
            )));
        }
        if let (Some(question), Value::String(text)) = (question(key), value)
            && matches!(question.kind, Kind::Choice(_))
        {
            parse(key, text)?;
        }
    }
    Ok(map)
}

/// The answer of `--answer key=value`.
pub fn from_flag(pair: &str) -> Result<(String, Value)> {
    let (key, value) = pair
        .split_once('=')
        .ok_or_else(|| CliError::usage(format!("--answer takes key=value, not `{pair}`.")))?;
    let key = key.trim();
    Ok((key.to_owned(), parse(key, value)?))
}

/// An answer as a person reads it in the review.
pub fn show(key: &str, value: &Value) -> String {
    let label = question(key).and_then(|question| match question.kind {
        Kind::Choice(values) => values
            .iter()
            .find(|(name, _)| Some(*name) == value.as_str())
            .map(|(_, label)| *label),
        _ => None,
    });
    match (label, value) {
        (Some(label), _) => label.to_owned(),
        (None, Value::Bool(true)) => "Yes".to_owned(),
        (None, Value::Bool(false)) => "No".to_owned(),
        (None, Value::Array(items)) => items
            .iter()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>()
            .join(", "),
        (None, Value::String(text)) => text.clone(),
        (None, Value::Null) => "-".to_owned(),
        (None, other) => other.to_string(),
    }
}

/// What `--answer <key>=` takes, for the list of missing answers (N3-6): the values of a
/// choice, `yes|no`, country codes, or `<text>`.
pub fn value_hint(key: &str) -> String {
    match question(key).map(|question| question.kind) {
        Some(Kind::Choice(values)) => values
            .iter()
            .map(|(value, _)| *value)
            .collect::<Vec<_>>()
            .join("|"),
        Some(Kind::YesNo) => "yes|no".to_owned(),
        Some(Kind::Countries) => "<codes, for example EE,FI>".to_owned(),
        Some(Kind::Text) | None => "<text>".to_owned(),
    }
}

/// The questions that are asked and have no answer yet: each one is required.
pub fn missing(answers: &Map<String, Value>, server: &Value) -> Vec<&'static str> {
    QUESTIONS
        .iter()
        .filter(|question| asked(question.key, answers, server))
        .filter(|question| !answered(answers.get(question.key)))
        .map(|question| question.key)
        .collect()
}

/// The number of question `key` among the questions asked, and their count.
pub fn place(key: &str, answers: &Map<String, Value>, server: &Value) -> (usize, usize) {
    let shown: Vec<&str> = QUESTIONS
        .iter()
        .filter(|question| asked(question.key, answers, server))
        .map(|question| question.key)
        .collect();
    let at = shown.iter().position(|shown| *shown == key).unwrap_or(0);
    (at + 1, shown.len())
}

/// The body of the submission: the answered fields of the questions that are asked.
pub fn body(answers: &Map<String, Value>, server: &Value) -> Value {
    let mut body = Map::new();
    for question in QUESTIONS {
        if asked(question.key, answers, server)
            && let Some(value) = answers.get(question.key)
            && answered(Some(value))
        {
            body.insert(question.key.to_owned(), value.clone());
        }
    }
    Value::Object(body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_question_is_a_field() {
        for question in QUESTIONS {
            assert!(
                QUESTIONNAIRE_FIELDS.contains(&question.key),
                "{}",
                question.key
            );
        }
        assert_eq!(QUESTIONS.len(), QUESTIONNAIRE_FIELDS.len());
    }

    #[test]
    fn value_hints() {
        assert_eq!(
            value_hint("expected_turnover"),
            "under_50k|50k_200k|200k_1m|over_1m"
        );
        assert_eq!(value_hint("needs_licence"), "yes|no");
        assert_eq!(
            value_hint("customer_countries"),
            "<codes, for example EE,FI>"
        );
        assert_eq!(value_hint("estonia_reason"), "<text>");
        assert_eq!(value_hint("nope"), "<text>");
        // Money as everywhere in the CLI: a space between thousands (N3-7).
        assert_eq!(
            show("expected_turnover", &json!("under_50k")),
            "Under 50 000 EUR"
        );
    }

    #[test]
    fn parses_answers() {
        assert_eq!(parse("cash_usage", "no").unwrap(), json!("no"));
        assert!(
            parse("cash_usage", "lots")
                .unwrap_err()
                .message
                .contains("a_little")
        );
        assert_eq!(
            parse("customer_countries", "ee, fi").unwrap(),
            json!(["EE", "FI"])
        );
        assert!(
            parse("customer_countries", "Estonia")
                .unwrap_err()
                .message
                .contains("not a country code")
        );
        assert!(parse("customer_countries", " , ").is_err());
        assert_eq!(parse("needs_licence", "Yes").unwrap(), json!(true));
        assert_eq!(parse("needs_licence", "n").unwrap(), json!(false));
        assert!(parse("needs_licence", "maybe").is_err());
        assert_eq!(
            parse("activity_description", " Software ").unwrap(),
            json!("Software")
        );
        assert!(
            parse("nope", "x")
                .unwrap_err()
                .message
                .contains("not a question")
        );
        assert_eq!(
            from_flag("uses_crypto_assets=no").unwrap(),
            ("uses_crypto_assets".to_owned(), json!(false))
        );
        assert!(from_flag("no-equals").is_err());
        assert_eq!(yes_no("TRUE"), Some(true));
    }

    #[test]
    fn reads_a_file() {
        let map = from_file(r#"{"cash_usage": "no", "customer_countries": ["EE"]}"#).unwrap();
        assert_eq!(map.len(), 2);
        assert!(from_file("[1]").is_err());
        assert!(from_file("{").is_err());
        assert!(
            from_file(r#"{"x": 1}"#)
                .unwrap_err()
                .message
                .contains("`x`")
        );
        assert!(from_file(r#"{"cash_usage": "lots"}"#).is_err());
    }

    #[test]
    fn decides_what_is_asked() {
        let mut answers = Map::new();
        let server = json!({"estonia_reason": false, "startup_funds_source": true});
        assert!(!asked("licence_description", &answers, &server));
        assert!(!asked("residence_countries", &answers, &server));
        assert!(asked("startup_funds_source", &answers, &server));
        assert!(!asked("estonia_reason", &answers, &server));
        assert!(asked("cash_usage", &answers, &server));
        answers.insert("needs_licence".into(), json!(true));
        answers.insert("residence".into(), json!("some_abroad"));
        answers.insert("acts_for_others".into(), json!(true));
        assert!(asked("licence_description", &answers, &server));
        assert!(asked("e_resident", &answers, &server));
        assert!(asked("acts_for_others_reason", &answers, &server));
        answers.insert("licence_description".into(), json!(""));
        answers.insert("cash_usage".into(), json!("no"));
        answers.insert("customer_countries".into(), json!([]));
        let open = missing(&answers, &server);
        assert!(open.contains(&"licence_description") && open.contains(&"startup_funds_source"));
        assert!(!open.contains(&"cash_usage") && !open.contains(&"estonia_reason"));
        assert_eq!(place("activity_description", &answers, &server), (1, 15));
        assert_eq!(place("field_experience", &answers, &server), (15, 15));
        assert_eq!(place("activity_description", &Map::new(), &json!({})).1, 10);
        assert_eq!(
            body(&answers, &server),
            json!({"cash_usage": "no", "needs_licence": true, "residence": "some_abroad", "acts_for_others": true})
        );
    }

    #[test]
    fn shows_answers() {
        assert_eq!(show("cash_usage", &json!("a_little")), "A little");
        assert_eq!(show("needs_licence", &json!(true)), "Yes");
        assert_eq!(show("needs_licence", &json!(false)), "No");
        assert_eq!(show("customer_countries", &json!(["EE", "FI"])), "EE, FI");
        assert_eq!(show("activity_description", &json!("Apps")), "Apps");
        assert_eq!(show("e_resident", &Value::Null), "-");
        assert_eq!(show("x", &json!(3)), "3");
        assert!(answered(Some(&json!(false))));
        assert!(!answered(Some(&json!(" "))));
        assert!(!answered(None));
    }
}
