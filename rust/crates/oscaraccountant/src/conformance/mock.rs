//! The HTTP mock of the runner: a `wiremock` server that answers the n-th request with the n-th
//! exchange of the case, after it checks the request against the case ("The matching rules" in
//! `clients/spec/conformance/README.md`).
//!
//! The client sends to the origin of the case (`https://api.example.test`, ...). A transport
//! around the real `ReqwestTransport` checks that origin and sends the request to the mock
//! instead. A `fault` exchange is checked like the others; the mock then marks its answer, and the
//! transport turns it into the transport error of the fault, because `wiremock` cannot drop a
//! connection or hold an answer past a timeout of the client without a real wait.

use std::sync::{Arc, Mutex};

use futures::future::BoxFuture;
use serde_json::Value;
use wiremock::{Request, Respond, ResponseTemplate};

use super::{decode_base64, json_equal, sha256_hex};
use crate::{HttpRequest, HttpResponse, HttpTransport, ReqwestTransport, TlsRoots, TransportError};

/// The header of a mock answer that stands for a fault of the exchange.
const FAULT_HEADER: &str = "x-conformance-fault";

/// The exchanges of one case, and what went wrong with them.
pub(super) struct Player {
    exchanges: Vec<Value>,
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    used: usize,
    problems: Vec<String>,
}

impl Player {
    pub(super) fn new(exchanges: Vec<Value>) -> Arc<Self> {
        Arc::new(Self {
            exchanges,
            state: Mutex::default(),
        })
    }

    /// Record a problem of the case.
    pub(super) fn problem(&self, problem: String) {
        self.state.lock().unwrap().problems.push(problem);
    }

    /// Every problem so far, and a problem for each exchange the client did not use.
    pub(super) fn problems(&self) -> Vec<String> {
        let state = self.state.lock().unwrap();
        let mut problems = state.problems.clone();
        if state.used < self.exchanges.len() {
            problems.push(format!(
                "the client sent {} requests, the case has {} exchanges",
                state.used,
                self.exchanges.len()
            ));
        }
        problems
    }
}

/// The `wiremock` answer: the next exchange.
pub(super) struct Responder(pub(super) Arc<Player>);

impl Respond for Responder {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let player = &self.0;
        let index = {
            let mut state = player.state.lock().unwrap();
            state.used += 1;
            state.used - 1
        };
        let Some(exchange) = player.exchanges.get(index) else {
            player.problem(format!(
                "request {}: {} {} after the last exchange",
                index + 1,
                request.method,
                request.url.path()
            ));
            return ResponseTemplate::new(599);
        };
        let differences = differences(&exchange["request"], request);
        if !differences.is_empty() {
            player.problem(format!(
                "request {} does not match: {}",
                index + 1,
                differences.join("; ")
            ));
            return ResponseTemplate::new(599);
        }
        if let Some(fault) = exchange["fault"].as_str() {
            return ResponseTemplate::new(204).insert_header(FAULT_HEADER, fault);
        }
        answer(&exchange["response"])
    }
}

/// The answer of a `response` of a case.
fn answer(response: &Value) -> ResponseTemplate {
    let status = u16::try_from(response["status"].as_u64().expect("a status")).expect("a status");
    let headers = response["headers"].as_object();
    let mut content_type = headers
        .and_then(|headers| headers.get("content-type"))
        .and_then(Value::as_str)
        .map(str::to_owned);
    let mut template = ResponseTemplate::new(status);
    for (name, value) in headers.into_iter().flatten() {
        if name != "content-type" {
            template = template.insert_header(name.as_str(), value.as_str().expect("a header"));
        }
    }
    let body = &response["body"];
    let (bytes, default_type) = match body["kind"].as_str().expect("a body kind") {
        "empty" => (Vec::new(), None),
        "json" => (
            body["value"].to_string().into_bytes(),
            Some(if status < 400 {
                "application/json"
            } else {
                "application/problem+json"
            }),
        ),
        "text" => (
            body["value"]
                .as_str()
                .expect("a text body")
                .as_bytes()
                .to_vec(),
            Some("text/plain; charset=utf-8"),
        ),
        "base64" => (
            decode_base64(body["value"].as_str().expect("a base64 body")),
            Some("application/octet-stream"),
        ),
        other => panic!("the case has the unknown body kind `{other}`"),
    };
    if content_type.is_none() {
        content_type = default_type.map(str::to_owned);
    }
    match content_type {
        Some(content_type) => template.set_body_raw(bytes, &content_type),
        None => template,
    }
}

/// How a request differs from the `request` of its exchange. Empty when it matches.
pub(super) fn differences(expected: &Value, request: &Request) -> Vec<String> {
    let mut differences = Vec::new();
    let method = expected["method"].as_str().expect("a method");
    if request.method.as_str() != method {
        differences.push(format!("method {} instead of {method}", request.method));
    }
    let path = expected["path"].as_str().expect("a path");
    if request.url.path() != path {
        differences.push(format!("path {} instead of {path}", request.url.path()));
    }
    let mut query: Vec<(String, String)> = request
        .url
        .query_pairs()
        .map(|(name, value)| (name.into_owned(), value.into_owned()))
        .collect();
    query.sort();
    let mut expected_query: Vec<(String, String)> = expected["query"]
        .as_array()
        .expect("a query")
        .iter()
        .map(|pair| {
            (
                pair[0].as_str().expect("a name").to_owned(),
                pair[1].as_str().expect("a value").to_owned(),
            )
        })
        .collect();
    expected_query.sort();
    if query != expected_query {
        differences.push(format!("query {query:?} instead of {expected_query:?}"));
    }
    for (name, rule) in expected["headers"].as_object().into_iter().flatten() {
        let value = header(request, name);
        let fits = match (rule, &value) {
            (Value::Null, None) => true,
            (Value::String(expected), Some(value)) => value == expected,
            (Value::Object(pattern), Some(value)) => {
                let pattern = pattern["pattern"].as_str().expect("a pattern");
                regex::Regex::new(pattern)
                    .expect("a valid pattern")
                    .is_match(value)
            }
            _ => false,
        };
        if !fits {
            differences.push(format!("header {name} is {value:?}, the case wants {rule}"));
        }
    }
    differences.extend(body_differences(&expected["body"], request));
    differences
}

/// The value of a header, the values joined when it is repeated.
fn header(request: &Request, name: &str) -> Option<String> {
    let values: Vec<&str> = request
        .headers
        .get_all(name)
        .iter()
        .map(|value| value.to_str().unwrap_or("<not text>"))
        .collect();
    (!values.is_empty()).then(|| values.join(", "))
}

fn body_differences(expected: &Value, request: &Request) -> Vec<String> {
    match expected["kind"].as_str().expect("a body kind") {
        "none" if request.body.is_empty() => Vec::new(),
        "none" => vec![format!(
            "a body of {} bytes, the case wants none",
            request.body.len()
        )],
        "json" => match serde_json::from_slice::<Value>(&request.body) {
            Ok(body) if json_equal(&body, &expected["value"]) => Vec::new(),
            Ok(body) => vec![format!(
                "JSON body {body}, the case wants {}",
                expected["value"]
            )],
            Err(error) => vec![format!("the body is not JSON: {error}")],
        },
        "multipart" => {
            let content_type = header(request, "content-type").unwrap_or_default();
            match parse_multipart(&content_type, &request.body) {
                Ok(parts) => multipart_differences(&expected["parts"], &parts),
                Err(error) => vec![error],
            }
        }
        other => panic!("the case has the unknown request body kind `{other}`"),
    }
}

/// One part of a `multipart/form-data` body.
#[derive(Debug, PartialEq)]
pub(super) struct Part {
    pub(super) name: String,
    pub(super) filename: Option<String>,
    pub(super) content_type: Option<String>,
    pub(super) content: Vec<u8>,
}

/// The parts of a `multipart/form-data` body.
pub(super) fn parse_multipart(content_type: &str, body: &[u8]) -> Result<Vec<Part>, String> {
    let boundary = content_type
        .split(';')
        .map(str::trim)
        .find_map(|parameter| parameter.strip_prefix("boundary="))
        .map(|boundary| boundary.trim_matches('"'))
        .ok_or_else(|| format!("no boundary in the content type {content_type:?}"))?;
    let delimiter = format!("--{boundary}").into_bytes();
    let mut parts = Vec::new();
    let mut segments = split(body, &delimiter).into_iter().skip(1);
    loop {
        let Some(segment) = segments.next() else {
            return Err("the body has no closing boundary".to_owned());
        };
        if segment.starts_with(b"--") {
            return Ok(parts);
        }
        let segment = segment.strip_prefix(b"\r\n").unwrap_or(segment);
        let segment = segment.strip_suffix(b"\r\n").unwrap_or(segment);
        let split_at = find(segment, b"\r\n\r\n").ok_or("a part has no blank line")?;
        let head = String::from_utf8_lossy(&segment[..split_at]);
        let mut part = Part {
            name: String::new(),
            filename: None,
            content_type: None,
            content: segment[split_at + 4..].to_vec(),
        };
        for line in head.split("\r\n") {
            let Some((name, value)) = line.split_once(':') else {
                continue;
            };
            match name.trim().to_ascii_lowercase().as_str() {
                "content-disposition" => {
                    for parameter in value.split(';').map(str::trim) {
                        if let Some(name) = parameter.strip_prefix("name=") {
                            part.name = name.trim_matches('"').to_owned();
                        } else if let Some(filename) = parameter.strip_prefix("filename=") {
                            part.filename = Some(filename.trim_matches('"').to_owned());
                        }
                    }
                }
                "content-type" => part.content_type = Some(value.trim().to_owned()),
                _ => {}
            }
        }
        parts.push(part);
    }
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn split<'a>(mut body: &'a [u8], delimiter: &[u8]) -> Vec<&'a [u8]> {
    let mut segments = Vec::new();
    while let Some(at) = find(body, delimiter) {
        segments.push(&body[..at]);
        body = &body[at + delimiter.len()..];
    }
    segments.push(body);
    segments
}

fn multipart_differences(expected: &Value, parts: &[Part]) -> Vec<String> {
    let expected = expected.as_array().expect("the parts");
    let mut names: Vec<&str> = parts.iter().map(|part| part.name.as_str()).collect();
    let mut expected_names: Vec<&str> = expected
        .iter()
        .map(|part| part["name"].as_str().expect("a part name"))
        .collect();
    names.sort_unstable();
    expected_names.sort_unstable();
    if names != expected_names {
        return vec![format!("parts {names:?} instead of {expected_names:?}")];
    }
    let mut differences = Vec::new();
    for want in expected {
        let name = want["name"].as_str().expect("a part name");
        let part = parts.iter().find(|part| part.name == name).expect("a part");
        if let Some(value) = want["value"].as_str() {
            if part.content != value.as_bytes() || part.filename.is_some() {
                differences.push(format!("part {name} is not the text {value:?}"));
            }
            continue;
        }
        if part.filename.as_deref() != want["filename"].as_str() {
            differences.push(format!(
                "part {name} has the file name {:?}, the case wants {}",
                part.filename, want["filename"]
            ));
        }
        if let Some(content_type) = want["content_type"].as_str() {
            if part.content_type.as_deref() != Some(content_type) {
                differences.push(format!(
                    "part {name} has the content type {:?}, the case wants {content_type}",
                    part.content_type
                ));
            }
        }
        let sha256 = sha256_hex(&part.content);
        if want["sha256"].as_str() != Some(sha256.as_str()) {
            differences.push(format!("part {name} has the SHA-256 {sha256}"));
        }
    }
    differences
}

/// The transport of the runner: it checks the origin of the case, sends the request to the mock,
/// and turns a fault answer into its transport error.
pub(super) struct OriginTransport {
    pub(super) origin: String,
    pub(super) mock: String,
    pub(super) player: Arc<Player>,
    pub(super) inner: ReqwestTransport,
}

impl OriginTransport {
    pub(super) fn new(origin: &str, mock: &str, player: Arc<Player>) -> Self {
        Self {
            origin: origin.to_owned(),
            mock: mock.to_owned(),
            player,
            inner: ReqwestTransport::new(TlsRoots::Bundled).expect("a transport"),
        }
    }
}

impl HttpTransport for OriginTransport {
    fn send(
        &self,
        mut request: HttpRequest,
    ) -> BoxFuture<'_, Result<HttpResponse, TransportError>> {
        let rest = request
            .url
            .strip_prefix(&self.origin)
            .filter(|rest| rest.starts_with('/'))
            .map(str::to_owned);
        let Some(rest) = rest else {
            self.player.problem(format!(
                "a request to {}, not under the origin {}",
                request.url, self.origin
            ));
            return Box::pin(async { Err(TransportError::Connection("another origin".into())) });
        };
        request.url = format!("{}{rest}", self.mock);
        Box::pin(async move {
            let response = self.inner.send(request).await?;
            match response.header(FAULT_HEADER) {
                Some("timeout") => Err(TransportError::Timeout),
                Some("connection_error") => Err(TransportError::Connection(
                    "the case drops the connection".into(),
                )),
                Some(other) => panic!("the case has the unknown fault `{other}`"),
                None => Ok(response),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_multipart_body() {
        let body = b"preamble\r\n--b\r\nContent-Disposition: form-data; name=\"kind\"\r\n\r\nreceipt\r\n--b\r\nContent-Disposition: form-data; name=\"file\"; filename=\"a.pdf\"\r\nContent-Type: application/pdf\r\n\r\n%PDF\r\n--b--\r\n";
        let parts = parse_multipart("multipart/form-data; boundary=\"b\"", body).unwrap();
        assert_eq!(
            parts,
            vec![
                Part {
                    name: "kind".into(),
                    filename: None,
                    content_type: None,
                    content: b"receipt".to_vec(),
                },
                Part {
                    name: "file".into(),
                    filename: Some("a.pdf".into()),
                    content_type: Some("application/pdf".into()),
                    content: b"%PDF".to_vec(),
                },
            ]
        );
    }

    #[test]
    fn refuses_a_broken_multipart_body() {
        assert!(parse_multipart("multipart/form-data", b"").is_err());
        assert!(parse_multipart("multipart/form-data; boundary=b", b"--b\r\nx").is_err());
        assert!(parse_multipart("multipart/form-data; boundary=b", b"--b\r\nx\r\n--b--").is_err());
    }
}
