//! The fake transport and the fixtures of the tests.

#![allow(dead_code)]

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use futures::future::BoxFuture;
use oscaraccountant::{ClientBuilder, HttpRequest, HttpResponse, HttpTransport, TransportError};
use serde_json::{Value, json};

/// One scripted answer of the fake transport.
pub enum Reply {
    Response(u16, Vec<(&'static str, String)>, Vec<u8>),
    Error(TransportError),
    /// Never answers (a timeout).
    Hang,
}

impl Reply {
    pub fn json(status: u16, body: Value) -> Self {
        Self::Response(
            status,
            vec![("content-type", "application/json".into())],
            body.to_string().into_bytes(),
        )
    }

    pub fn problem(status: u16, code: &str) -> Self {
        let body = json!({"type": "about:blank", "title": "Problem", "status": status, "code": code,
                          "detail": format!("{code} happened"), "errors": []});
        Self::Response(
            status,
            vec![
                ("content-type", "application/problem+json".into()),
                ("x-request-id", "req-9".into()),
            ],
            body.to_string().into_bytes(),
        )
    }

    pub fn with_header(self, name: &'static str, value: &str) -> Self {
        match self {
            Self::Response(status, mut headers, body) => {
                headers.push((name, value.to_owned()));
                Self::Response(status, headers, body)
            }
            other => other,
        }
    }
}

/// A transport that records every request and plays scripted replies, in order.
#[derive(Clone, Default)]
pub struct FakeTransport {
    pub requests: Arc<Mutex<Vec<HttpRequest>>>,
    replies: Arc<Mutex<VecDeque<Reply>>>,
}

impl FakeTransport {
    pub fn new(replies: Vec<Reply>) -> Self {
        Self {
            requests: Arc::default(),
            replies: Arc::new(Mutex::new(replies.into())),
        }
    }

    pub fn requests(&self) -> Vec<HttpRequest> {
        self.requests.lock().unwrap().clone()
    }
}

impl HttpTransport for FakeTransport {
    fn send(&self, request: HttpRequest) -> BoxFuture<'_, Result<HttpResponse, TransportError>> {
        self.requests.lock().unwrap().push(request);
        let reply = self
            .replies
            .lock()
            .unwrap()
            .pop_front()
            .expect("no reply left");
        Box::pin(async move {
            match reply {
                Reply::Response(status, headers, body) => Ok(HttpResponse::new(
                    status,
                    headers
                        .into_iter()
                        .map(|(name, value)| (name.to_owned(), value))
                        .collect(),
                    body,
                )),
                Reply::Error(error) => Err(error),
                Reply::Hang => {
                    tokio::time::sleep(Duration::from_secs(3600)).await;
                    unreachable!("the client times out first")
                }
            }
        })
    }
}

/// The fixed "now" of the tests: 27.09.2026 10:00:00 UTC.
pub fn now() -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(1_790_503_200)
}

/// A builder with the fake transport, an API key, no environment, the fixed clock, no jitter, a
/// recorded sleep, and the UUIDs `uuid-1`, `uuid-2`, ...
pub fn builder(transport: &FakeTransport, sleeps: &Arc<Mutex<Vec<Duration>>>) -> ClientBuilder {
    builder_with_env(transport, sleeps, &[])
}

pub fn builder_with_env(
    transport: &FakeTransport,
    sleeps: &Arc<Mutex<Vec<Duration>>>,
    vars: &[(&str, &str)],
) -> ClientBuilder {
    let vars: HashMap<String, String> = vars
        .iter()
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .collect();
    let counter = Arc::new(Mutex::new(0));
    let sleeps = sleeps.clone();
    oscaraccountant::Client::builder()
        .api_key("osk_test")
        .base_url("https://api.example.com")
        .transport(transport.clone())
        .env(move |name| vars.get(name).cloned())
        .clock(now)
        .jitter(|| 0.0)
        .sleep(move |duration| {
            sleeps.lock().unwrap().push(duration);
            Box::pin(async {})
        })
        .uuid_source(move || {
            let mut counter = counter.lock().unwrap();
            *counter += 1;
            format!("uuid-{counter}")
        })
}

pub fn company(id: &str) -> Value {
    json!({"id": id, "name": format!("Company {id}"), "registry_code": "12345678", "vat_number": "",
           "vat_status": "unregistered", "vat_status_from": null, "legal_form": "ou",
           "files_tsd_monthly": false, "state": "ready", "core_company_id": null, "base_currency": "EUR",
           "created_at": "2026-09-27T10:00:00Z", "updated_at": "2026-09-27T10:00:00Z",
           "role": "owner", "permissions": [], "view_as": "owner", "refresh_pending": false,
           "status": {"bank_lines_to_classify": 0, "payments_to_explain": 0, "documents_to_book": 0,
                      "pending_proposals": 0, "open_periods": 0, "vat_returns_due": 0,
                      "next_vat_due_date": null, "last_bank_import_at": null, "overdue_receivables": 0,
                      "overdue_receivables_minor": 0, "refreshed_at": null, "refresh_error": "",
                      "vat_months_not_checked": 0, "vat_not_checked_from": null, "vat_not_checked_to": null}})
}

pub fn outcome() -> Value {
    json!({"command": "cancel_invoice", "command_id": "c1", "commit_seq": 7, "replayed": false, "result": {}})
}

pub fn stored_document() -> Value {
    json!({"id": "f1", "kind": "invoice", "name": "a.pdf", "content_type": "application/pdf", "size_bytes": 4,
           "sha256": "x", "received_on": null, "core_document_id": null, "bank_transaction_ids": [],
           "uploaded_by": null, "created_at": "2026-09-27T10:00:00Z", "deleted_at": null})
}
