//! The conformance runner of the Rust SDK: it plays every case of `clients/spec/conformance`
//! against a `wiremock` server. The format and the rules are in
//! `clients/spec/conformance/README.md`.
//!
//! The runner is a module of the crate, not a test of `tests/`, because a spec call hands a
//! `RequestSpec` to the client directly, and `Client::execute` is `pub(crate)`.
//!
//! `OSCAR_CONFORMANCE_DIR` points the runner to another directory of cases. Run it alone with
//! `cargo test --lib conformance`.

mod calls;
mod mock;

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine;
use serde_json::{Value, json};
use wiremock::matchers::any;
use wiremock::{Mock, MockServer};

use self::calls::Outcome;
use self::mock::{OriginTransport, Player, Responder};
use crate::{Client, Error};

/// The cases the Rust SDK fails today, each with the reason. A listed case that passes fails the
/// run, so the list shrinks when the cause is fixed.
const KNOWN_FAILURES: &[(&str, &str)] = &[];

/// The default `now` of a case.
const DEFAULT_NOW: &str = "2026-09-27T10:00:00Z";

/// The directory of the cases: `OSCAR_CONFORMANCE_DIR`, else `clients/spec/conformance/cases`.
fn cases_dir() -> PathBuf {
    match std::env::var_os("OSCAR_CONFORMANCE_DIR") {
        Some(dir) => PathBuf::from(dir),
        None => PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../spec/conformance/cases"),
    }
}

/// Every case, in the order of the file names, then of the files.
fn load_cases() -> Vec<Value> {
    let dir = cases_dir();
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap_or_else(|error| panic!("no conformance cases in {}: {error}", dir.display()))
        .map(|entry| entry.expect("a directory entry").path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "json")
        })
        .collect();
    files.sort();
    files
        .iter()
        .flat_map(|path| {
            let text = std::fs::read_to_string(path).expect("a readable case file");
            let file: Value = serde_json::from_str(&text).expect("a JSON case file");
            file["cases"].as_array().expect("the cases").clone()
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn every_conformance_case() {
    let cases = load_cases();
    assert!(!cases.is_empty(), "no conformance cases");
    let mut failures = Vec::new();
    let (mut passed, mut skipped, mut known) = (0, 0, 0);
    for case in &cases {
        let id = case["id"].as_str().expect("an id");
        if let Some(reason) = case["skip"]["rust"].as_str() {
            eprintln!("skipped {id}: {reason}");
            skipped += 1;
            continue;
        }
        let problems = play(case).await;
        let known_reason = KNOWN_FAILURES
            .iter()
            .find(|(known_id, _)| *known_id == id)
            .map(|(_, reason)| reason);
        match (problems.is_empty(), known_reason) {
            (true, None) => passed += 1,
            (true, Some(_)) => failures.push(format!(
                "{id}: passes, but is in KNOWN_FAILURES; remove it from the list"
            )),
            (false, Some(reason)) => {
                eprintln!("known failure {id}: {reason}");
                known += 1;
            }
            (false, None) => failures.push(format!("{id}:\n    {}", problems.join("\n    "))),
        }
    }
    eprintln!(
        "conformance: {passed} passed, {known} known failures, {skipped} skipped, {} failed",
        failures.len()
    );
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// Play one case. The problems, empty when the case passes.
async fn play(case: &Value) -> Vec<String> {
    let exchanges = case["exchanges"].as_array().expect("the exchanges").clone();
    let player = Player::new(exchanges);
    let server = MockServer::start().await;
    Mock::given(any())
        .respond_with(Responder(player.clone()))
        .mount(&server)
        .await;
    let sleeps = Arc::new(Mutex::new(Vec::new()));
    let origin = case["origin"].as_str().expect("an origin");
    let transport = OriginTransport::new(origin, &server.uri(), player.clone());
    let client = match client(case, transport, &sleeps, &player) {
        Ok(client) => client,
        Err(error) => return vec![format!("the client cannot be built: {error}")],
    };
    let outcome = calls::call(&client, &case["call"]).await;
    let mut problems = player.problems();
    let sleeps = sleeps.lock().unwrap().clone();
    let expected_sleeps: Vec<f64> = case["sleeps"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|sleep| sleep.as_f64().expect("a number of seconds"))
        .collect();
    let same_sleeps = sleeps.len() == expected_sleeps.len()
        && sleeps
            .iter()
            .zip(&expected_sleeps)
            .all(|(actual, expected)| (actual - expected).abs() <= 1e-6);
    if !same_sleeps {
        problems.push(format!(
            "slept {sleeps:?}, the case wants {expected_sleeps:?}"
        ));
    }
    problems.extend(outcome_problems(&case["expect"], outcome).await);
    problems
}

/// The client of a case, with the injected environment, clock, sleep, jitter and UUID source.
fn client(
    case: &Value,
    transport: OriginTransport,
    sleeps: &Arc<Mutex<Vec<f64>>>,
    player: &Arc<Player>,
) -> Result<Client, Error> {
    let options = &case["client"];
    let inject = &case["inject"];
    let env: HashMap<String, String> = case["env"]
        .as_object()
        .into_iter()
        .flatten()
        .map(|(name, value)| (name.clone(), value.as_str().expect("a variable").to_owned()))
        .collect();
    let now = inject["now"].as_str().unwrap_or(DEFAULT_NOW);
    let clock = Arc::new(Mutex::new(
        crate::rfc3339::parse(now).expect("an RFC 3339 now"),
    ));
    let jitter = inject["jitter"].as_f64().unwrap_or(0.0);
    let uuids: VecDeque<String> = inject["uuids"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|uuid| uuid.as_str().expect("a UUID").to_owned())
        .collect();
    let uuids = Mutex::new(uuids);
    let uuid_player = player.clone();
    let sleep_clock = clock.clone();
    let sleeps = sleeps.clone();
    let mut builder = Client::builder()
        .transport(transport)
        .env(move |name| env.get(name).cloned())
        .clock(move || *clock.lock().unwrap())
        .sleep(move |duration| {
            sleeps.lock().unwrap().push(duration.as_secs_f64());
            *sleep_clock.lock().unwrap() += duration;
            Box::pin(async {})
        })
        .jitter(move || jitter)
        .uuid_source(move || {
            uuids.lock().unwrap().pop_front().unwrap_or_else(|| {
                uuid_player
                    .problem("the client asked the UUID source for one UUID too many".into());
                "no-uuid-left".to_owned()
            })
        });
    if let Some(base_url) = options["base_url"].as_str() {
        builder = builder.base_url(base_url);
    }
    if let Some(api_key) = options["api_key"].as_str() {
        builder = builder.api_key(api_key);
    }
    if let Some(tokens) = options["token_provider"].as_array() {
        let tokens: VecDeque<String> = tokens
            .iter()
            .map(|token| token.as_str().expect("a token").to_owned())
            .collect();
        let tokens = Arc::new(Mutex::new(tokens));
        builder = builder.token_provider(move || {
            let next = tokens.lock().unwrap().pop_front();
            async move { next.ok_or("the token provider has no token left") }
        });
    }
    if let Some(retries) = options["max_retries"].as_u64() {
        builder = builder.max_retries(u32::try_from(retries).expect("a small number"));
    }
    if let Some(milliseconds) = options["timeout_ms"].as_u64() {
        builder = builder.timeout(Duration::from_millis(milliseconds));
    }
    builder.build()
}

/// How the outcome differs from `expect`. Empty when it matches.
async fn outcome_problems(expect: &Value, outcome: Result<Outcome, Error>) -> Vec<String> {
    let result = &expect["result"];
    match (outcome, expect.get("error")) {
        (Err(error), Some(expected)) => error_problems(expected, &error),
        (Err(error), None) => vec![format!("the call failed: {error:?}")],
        (Ok(outcome), Some(expected)) => {
            vec![format!(
                "the call gave {outcome:?}, the case wants the error {expected}"
            )]
        }
        (Ok(Outcome::Json(actual)), None) if result["kind"] == "json" => {
            if matches(&result["value"], &actual) {
                Vec::new()
            } else {
                vec![format!(
                    "the result {actual} does not match {}",
                    result["value"]
                )]
            }
        }
        (Ok(Outcome::Items(items)), None) if result["kind"] == "items" => {
            let actual = Value::Array(items);
            if matches(&result["value"], &actual) {
                Vec::new()
            } else {
                vec![format!(
                    "the items {actual} do not match {}",
                    result["value"]
                )]
            }
        }
        (Ok(Outcome::None), None) if result["kind"] == "none" => Vec::new(),
        (
            Ok(Outcome::Binary {
                bytes,
                content_type,
                file_name,
            }),
            None,
        ) if result["kind"] == "binary" => {
            let actual = json!({
                "kind": "binary",
                "sha256": sha256_hex(&bytes),
                "size": bytes.len(),
                "content_type": content_type,
                "filename": file_name,
            });
            if json_equal(&actual, result) {
                Vec::new()
            } else {
                vec![format!("the download {actual} is not {result}")]
            }
        }
        (Ok(outcome), None) => vec![format!(
            "the call gave {outcome:?}, the case wants {result}"
        )],
    }
}

/// The name of the error type of section 3.7 that an [`Error`] stands for.
fn error_type(error: &Error) -> &'static str {
    match error {
        Error::BadRequest(_) => "BadRequestError",
        Error::Authentication(_) => "AuthenticationError",
        Error::PermissionDenied(_) => "PermissionDeniedError",
        Error::NotFound(_) => "NotFoundError",
        Error::Conflict(_) => "ConflictError",
        Error::UnprocessableEntity(_) => "UnprocessableEntityError",
        Error::RateLimit(_) => "RateLimitError",
        Error::InternalServer(_) => "InternalServerError",
        Error::Api(_) => "ApiError",
        Error::Connection(_) => "ConnectionError",
        Error::Timeout(_) => "TimeoutError",
        Error::Config(_) => "UsageError",
        Error::Json(_) => "JsonError",
    }
}

/// How an error differs from the expected one: its type, and each field the case lists.
fn error_problems(expected: &Value, error: &Error) -> Vec<String> {
    let api = error.api_error();
    let actual = json!({
        "type": error_type(error),
        "status": error.status(),
        "code": api.map(|api| &api.code),
        "title": api.map(|api| &api.title),
        "detail": api.map(|api| &api.detail),
        "errors": api.map(|api| &api.errors),
        "request_id": api.and_then(|api| api.request_id.as_ref()),
        "message": error.to_string(),
    });
    expected
        .as_object()
        .expect("an error")
        .iter()
        .filter(|(field, value)| !json_equal(&actual[field.as_str()], value))
        .map(|(field, value)| {
            format!(
                "the error {field} is {}, the case wants {value}",
                actual[field.as_str()]
            )
        })
        .collect()
}

/// The "match" rule of the cases: every expected key matches, the actual object may have more
/// keys, and an expected null may be absent.
fn matches(expected: &Value, actual: &Value) -> bool {
    match (expected, actual) {
        (Value::Object(expected), Value::Object(actual)) => {
            expected.iter().all(|(key, value)| match actual.get(key) {
                Some(actual) => matches(value, actual),
                None => value.is_null(),
            })
        }
        (Value::Array(expected), Value::Array(actual)) => {
            expected.len() == actual.len()
                && expected
                    .iter()
                    .zip(actual)
                    .all(|(expected, actual)| matches(expected, actual))
        }
        _ => json_equal(expected, actual),
    }
}

/// Equal as parsed JSON: `1` equals `1.0`, and the order of keys does not matter.
fn json_equal(left: &Value, right: &Value) -> bool {
    match (left, right) {
        (Value::Number(left), Value::Number(right)) => left.as_f64() == right.as_f64(),
        (Value::Object(left), Value::Object(right)) => {
            left.len() == right.len()
                && left.iter().all(|(key, value)| {
                    right.get(key).is_some_and(|other| json_equal(value, other))
                })
        }
        (Value::Array(left), Value::Array(right)) => {
            left.len() == right.len() && left.iter().zip(right).all(|(l, r)| json_equal(l, r))
        }
        _ => left == right,
    }
}

fn decode_base64(value: &str) -> Vec<u8> {
    base64::engine::general_purpose::STANDARD
        .decode(value)
        .expect("valid base64")
}

fn sha256_hex(content: &[u8]) -> String {
    ring::digest::digest(&ring::digest::SHA256, content)
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[test]
fn the_match_rule() {
    assert!(matches(
        &json!({"a": 1, "b": null}),
        &json!({"a": 1.0, "c": 2})
    ));
    assert!(!matches(&json!({"a": 1}), &json!({})));
    assert!(!matches(&json!([1]), &json!([1, 2])));
    assert!(!matches(&json!(true), &json!(1)));
    assert!(json_equal(
        &json!({"a": [1, "x"]}),
        &json!({"a": [1.0, "x"]})
    ));
    assert!(!json_equal(&json!({"a": 1}), &json!({"a": 1, "b": 2})));
    assert!(!json_equal(&json!([1]), &json!([2])));
}

#[tokio::test]
async fn a_case_that_the_client_breaks_fails() {
    let cases = load_cases();
    let case = |id: &str| {
        cases
            .iter()
            .find(|case| case["id"] == id)
            .unwrap_or_else(|| panic!("no case {id}"))
            .clone()
    };

    // Another header, another sleep, another result.
    let mut wrong = case("retries.get-503-then-ok");
    wrong["exchanges"][0]["request"]["headers"]["accept"] = json!("text/html");
    let problems = play(&wrong).await;
    assert!(problems[0].contains("header accept"), "{problems:?}");
    let mut wrong = case("retries.get-503-then-ok");
    wrong["sleeps"] = json!([9.0]);
    wrong["expect"]["result"]["value"] = json!({"user": {"id": "someone else"}});
    let problems = play(&wrong).await;
    assert!(problems[0].starts_with("slept [0.5]"), "{problems:?}");
    assert!(problems[1].starts_with("the result"), "{problems:?}");

    // One exchange too many, and a result where the case wants an error.
    let mut wrong = case("auth.api-key-option");
    let exchange = wrong["exchanges"][0].clone();
    wrong["exchanges"].as_array_mut().unwrap().push(exchange);
    wrong["expect"] = json!({"error": {"type": "NotFoundError"}});
    let problems = play(&wrong).await;
    assert!(problems[0].contains("sent 1 requests"), "{problems:?}");
    assert!(problems[1].contains("wants the error"), "{problems:?}");

    // No exchange at all, another origin, and a UUID source that runs dry.
    let mut wrong = case("idempotency.automatic-key");
    wrong["exchanges"] = json!([]);
    wrong["origin"] = json!("https://elsewhere.test");
    wrong["inject"]["uuids"] = json!([]);
    let problems = play(&wrong).await;
    assert!(
        problems.iter().any(|p| p.contains("one UUID too many")),
        "{problems:?}"
    );
    assert!(
        problems.iter().any(|p| p.contains("not under the origin")),
        "{problems:?}"
    );

    // A request after the last exchange, and a wrong download and error.
    let mut wrong = case("retries.get-503-then-ok");
    wrong["exchanges"].as_array_mut().unwrap().pop();
    let problems = play(&wrong).await;
    assert!(
        problems[0].contains("after the last exchange"),
        "{problems:?}"
    );
    let mut wrong = case("files.download-bytes");
    wrong["expect"]["result"]["size"] = json!(1);
    assert!(play(&wrong).await[0].starts_with("the download"));
    let mut wrong = case("errors.status-404");
    wrong["expect"]["error"]["code"] = json!("gone");
    assert!(play(&wrong).await[0].starts_with("the error code"));
    let mut wrong = case("pagination.all-pages");
    wrong["expect"]["result"]["value"] = json!([]);
    assert!(play(&wrong).await[0].starts_with("the items"));
}
