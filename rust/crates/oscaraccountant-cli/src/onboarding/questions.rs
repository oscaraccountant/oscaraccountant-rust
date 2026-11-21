//! Step 2, "About your business": the questions of the AML questionnaire,
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
    /// A whole number of percent, from 0 to 100.
    Percent,
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

/// The answers of `cash_usage` that ask the cash share (8a): the share is required then, and
/// cleared on `no`.
pub const CASH_USED: &[&str] = &["a_little", "often"];

/// The line of help under the cash share, as the web form shows it.
pub const CASH_SHARE_HINT: &str = "An estimate is enough.";

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

/// The structure questions (rule 4.2 items 6, 7 and 9): asked only when the register does not show
/// natural persons who hold all of the company (`asked.structure` of the server).
pub const STRUCTURE: &[&str] = &["bearer_shares", "trust_in_chain", "nominees"];

/// The line before the structure questions, as the web form shows it.
pub const STRUCTURE_INTRO: &str = "We could not see the whole ownership of the company in the Business Register, so we ask three more questions.";

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
        key: "cash_share_percent",
        label: "What share of the turnover is received or paid in cash?",
        kind: Kind::Percent,
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
        key: "bearer_shares",
        label: "Does the company, or a company that owns it, have bearer shares?",
        kind: Kind::YesNo,
    },
    Question {
        key: "trust_in_chain",
        label: "Is a trust or a foundation among the owners?",
        kind: Kind::YesNo,
    },
    Question {
        key: "nominees",
        label: "Does anyone hold shares or a board seat for another person?",
        kind: Kind::YesNo,
    },
    Question {
        key: "startup_funds_source",
        label: "Where did the start-up money come from?",
        kind: Kind::Text,
    },
    Question {
        key: "acts_for_others",
        label: "Do you run this company for someone else?",
        kind: Kind::YesNo,
    },
    Question {
        key: "acts_for_others_reason",
        label: "Why?",
        kind: Kind::Text,
    },
    Question {
        key: "field_experience",
        label: "How long have the founders done this kind of work?",
        kind: Kind::Choice(EXPERIENCE),
    },
];

/// The question of `key`.
pub fn question(key: &str) -> Option<&'static Question> {
    QUESTIONS.iter().find(|question| question.key == key)
}

/// The line of help under question `key`, when it has one.
pub fn hint(key: &str) -> Option<&'static str> {
    (key == "cash_share_percent").then_some(CASH_SHARE_HINT)
}

/// Whether `key` is asked, after these answers. `server` is the `asked` object of the server,
/// for the questions it decides (11, why Estonia; 12, the start-up money; the structure questions).
pub fn asked(key: &str, answers: &Map<String, Value>, server: &Value) -> bool {
    let yes = |field: &str| answers.get(field) == Some(&Value::Bool(true));
    let abroad = answers.get("residence").and_then(Value::as_str) == Some("some_abroad");
    match key {
        "cash_share_percent" => answers
            .get("cash_usage")
            .and_then(Value::as_str)
            .is_some_and(|usage| CASH_USED.contains(&usage)),
        "licence_description" => yes("needs_licence"),
        "residence_countries" | "e_resident" => abroad,
        "acts_for_others_reason" => yes("acts_for_others"),
        "estonia_reason" | "startup_funds_source" => {
            server.get(key).and_then(Value::as_bool).unwrap_or(false)
        }
        key if STRUCTURE.contains(&key) => server
            .get("structure")
            .and_then(Value::as_bool)
            .unwrap_or(false),
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

/// A whole number of percent from 0 to 100, as a person writes it (`40`, `40 %` or `40%`).
pub fn percent(text: &str) -> std::result::Result<u8, String> {
    let number = text.trim().trim_end_matches('%').trim_end();
    match number.parse::<u8>() {
        Ok(value) if value <= 100 => Ok(value),
        _ => Err(format!(
            "`{}` is not a share: give a whole number from 0 to 100, for example 20.",
            text.trim()
        )),
    }
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
        Kind::Percent => percent(text)
            .map(|value| json!(value))
            .map_err(|message| CliError::usage(format!("{key}: {message}"))),
    }
}

/// The answers of `--answers <file.json>`: a JSON object with fields of the questionnaire.
pub fn from_file(text: &str) -> Result<Map<String, Value>> {
    let value: Value = serde_json::from_str(text)
        .map_err(|error| CliError::usage(format!("--answers is not valid JSON: {error}")))?;
    let Value::Object(mut map) = value else {
        return Err(CliError::usage(
            "--answers is a JSON object of the answers, for example {\"cash_usage\": \"no\"}.",
        ));
    };
    for (key, value) in &mut map {
        if !QUESTIONNAIRE_FIELDS.contains(&key.as_str()) {
            return Err(CliError::usage(format!(
                "--answers: `{key}` is not a question of the questionnaire."
            )));
        }
        match (question(key).map(|question| question.kind), &*value) {
            (Some(Kind::Choice(_)), Value::String(text)) => {
                parse(key, text)?;
            }
            // A share is a whole number from 0 to 100; "40" or "40 %" in the file is read too.
            (Some(Kind::Percent), Value::Null) => {}
            (Some(Kind::Percent), Value::String(text)) => *value = parse(key, text)?,
            (Some(Kind::Percent), other) => {
                *value = parse(key, &other.to_string())?;
            }
            _ => {}
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
        (None, Value::Number(number))
            if question(key).is_some_and(|question| question.kind == Kind::Percent) =>
        {
            // As the review shows a share elsewhere in the CLI: "40 %".
            format!("{number} %")
        }
        (None, other) => other.to_string(),
    }
}

/// What `--answer <key>=` takes, for the list of missing answers: the values of a
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
        Some(Kind::Percent) => "<0 to 100>".to_owned(),
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

/// The body of a draft save: every answered field, but not the cash share when cash is not used,
/// so a share kept from an earlier "a little" or "often" never goes with a "no".
pub fn draft(answers: &Map<String, Value>) -> Map<String, Value> {
    answers
        .iter()
        .filter(|(_, value)| answered(Some(value)))
        .filter(|(key, _)| *key != "cash_share_percent" || asked(key, answers, &Value::Null))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
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
        // Money as everywhere in the CLI: a space between thousands.
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
    fn asks_the_structure_questions_when_the_server_says_so() {
        let answers = Map::new();
        let not_asked = json!({"structure": false});
        let asked_now = json!({"structure": true});
        for key in STRUCTURE {
            assert_eq!(question(key).unwrap().kind, Kind::YesNo);
            assert!(!asked(key, &answers, &not_asked));
            assert!(!asked(key, &answers, &json!({})));
            assert!(asked(key, &answers, &asked_now));
        }
        assert_eq!(missing(&answers, &asked_now).len(), 13);
        assert!(missing(&answers, &asked_now).contains(&"nominees"));
        // In the order of the web form: after "why Estonia", before the start-up money.
        assert_eq!(place("bearer_shares", &answers, &asked_now), (9, 13));
        let mut given = Map::new();
        given.insert("bearer_shares".into(), json!(false));
        given.insert("trust_in_chain".into(), json!(false));
        given.insert("nominees".into(), json!(true));
        assert_eq!(
            body(&given, &asked_now),
            json!({"bearer_shares": false, "trust_in_chain": false, "nominees": true})
        );
        // Not asked: the server would drop them, so they are not sent.
        assert_eq!(body(&given, &not_asked), json!({}));
        assert_eq!(parse("nominees", "no").unwrap(), json!(false));
        assert_eq!(value_hint("trust_in_chain"), "yes|no");
    }

    #[test]
    fn asks_the_cash_share_when_cash_is_used() {
        let server = json!({});
        let mut answers = Map::new();
        assert!(!asked("cash_share_percent", &answers, &server));
        answers.insert("cash_usage".into(), json!("no"));
        assert!(!asked("cash_share_percent", &answers, &server));
        assert!(!missing(&answers, &server).contains(&"cash_share_percent"));
        for usage in CASH_USED {
            answers.insert("cash_usage".into(), json!(usage));
            assert!(asked("cash_share_percent", &answers, &server));
            assert!(missing(&answers, &server).contains(&"cash_share_percent"));
        }
        // Right after the cash question, as the web form asks it.
        assert_eq!(place("cash_usage", &answers, &server), (5, 11));
        assert_eq!(place("cash_share_percent", &answers, &server), (6, 11));
        answers.insert("cash_share_percent".into(), json!(40));
        assert!(!missing(&answers, &server).contains(&"cash_share_percent"));
        assert_eq!(body(&answers, &server)["cash_share_percent"], json!(40));
        assert_eq!(draft(&answers)["cash_share_percent"], json!(40));
        // A share kept from an earlier answer never goes with a "no".
        answers.insert("cash_usage".into(), json!("no"));
        assert!(body(&answers, &server).get("cash_share_percent").is_none());
        assert!(!draft(&answers).contains_key("cash_share_percent"));
        assert_eq!(draft(&answers)["cash_usage"], json!("no"));
        assert_eq!(hint("cash_share_percent"), Some("An estimate is enough."));
        assert_eq!(hint("cash_usage"), None);
        assert_eq!(value_hint("cash_share_percent"), "<0 to 100>");
        assert_eq!(question("cash_share_percent").unwrap().kind, Kind::Percent);
    }

    #[test]
    fn parses_a_share() {
        assert_eq!(parse("cash_share_percent", "40").unwrap(), json!(40));
        assert_eq!(parse("cash_share_percent", " 0 ").unwrap(), json!(0));
        assert_eq!(parse("cash_share_percent", "100").unwrap(), json!(100));
        assert_eq!(parse("cash_share_percent", "40 %").unwrap(), json!(40));
        assert_eq!(parse("cash_share_percent", "40%").unwrap(), json!(40));
        for bad in ["101", "-1", "40.5", "forty", "", "%"] {
            let error = parse("cash_share_percent", bad).unwrap_err();
            assert!(
                error.message.contains("whole number from 0 to 100"),
                "{bad}: {}",
                error.message
            );
        }
        assert_eq!(
            from_flag("cash_share_percent=60").unwrap(),
            ("cash_share_percent".to_owned(), json!(60))
        );
        let map = from_file(r#"{"cash_usage": "often", "cash_share_percent": 60}"#).unwrap();
        assert_eq!(map["cash_share_percent"], json!(60));
        let map = from_file(r#"{"cash_share_percent": "25 %"}"#).unwrap();
        assert_eq!(map["cash_share_percent"], json!(25));
        let map = from_file(r#"{"cash_share_percent": null}"#).unwrap();
        assert_eq!(map["cash_share_percent"], Value::Null);
        for bad in [
            r#"{"cash_share_percent": 101}"#,
            r#"{"cash_share_percent": 40.5}"#,
            r#"{"cash_share_percent": true}"#,
            r#"{"cash_share_percent": "lots"}"#,
        ] {
            assert!(from_file(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn shows_answers() {
        assert_eq!(show("cash_share_percent", &json!(40)), "40 %");
        assert_eq!(show("cash_share_percent", &Value::Null), "-");
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
