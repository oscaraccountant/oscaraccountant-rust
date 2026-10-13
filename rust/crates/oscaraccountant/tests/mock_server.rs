//! The SDK against `clients/mock-server` over real HTTP: every resource, pagination, errors, retries, idempotency, upload, download, and the
//! CLI login with a fake `oscar` on `PATH`.
//!
//! The suite starts `uv run --locked --project <clients/mock-server> oscar-mock-server --port 0` once and
//! plays its steps in order against it, with a reset between steps. Run it alone with
//! `cargo test --test mock_server`. It is part of the default `cargo test`: when `uv` or the mock
//! server project is missing, it prints why and passes, unless `OSCAR_MOCK_SERVER_REQUIRED=1`
//! makes that a failure (CI sets it). `OSCAR_MOCK_SERVER_PROJECT` points to another copy of the
//! project.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::{StreamExt, TryStreamExt};
use oscaraccountant::{
    CancelInvoiceBodyRequest, Client, ClientBuilder, CreatePartyPayloadRequest, Credential, Error,
    HttpMethod, InvoiceLineRequest, InvoiceRevisionRequest, RecordInvoicePayloadRequest,
    ReviseInvoiceBodyRequest, TlsRoots, Upload,
};
use serde_json::{Value, json};

const READY: &str = "OSCAR_MOCK_SERVER_READY ";
const COMPANY: &str = "cmp0000001";
const ORGANIZATION: &str = "org0000001";
const INVOICE: &str = "01J90000000000000000000116";

/// The mock server process. Dropping it stops the server.
struct MockServer {
    child: Child,
    url: String,
    api_key: String,
    http: oscaraccountant::reqwest::Client,
}

impl MockServer {
    /// Start the server, or `None` with the reason when it cannot run here.
    fn start() -> Result<Self, String> {
        let project = match std::env::var_os("OSCAR_MOCK_SERVER_PROJECT") {
            Some(project) => PathBuf::from(project),
            None => Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../mock-server"),
        };
        if !project.join("pyproject.toml").is_file() {
            return Err(format!("no mock server project at {}", project.display()));
        }
        let log = tempfile::NamedTempFile::new().map_err(|error| error.to_string())?;
        let mut child = Command::new("uv")
            .args(["run", "--quiet", "--locked", "--project"])
            .arg(&project)
            .args(["oscar-mock-server", "--port", "0"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(log.reopen().map_err(|error| error.to_string())?)
            .spawn()
            .map_err(|error| format!("`uv` cannot run: {error}"))?;
        let stdout = child.stdout.take().expect("a piped stdout");
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if let Some(ready) = line.strip_prefix(READY) {
                    let _ = sender.send(ready.to_owned());
                }
            }
        });
        // The first run of the project may install its environment.
        let Ok(ready) = receiver.recv_timeout(Duration::from_secs(180)) else {
            let _ = child.kill();
            let _ = child.wait();
            let mut errors = String::new();
            let _ = log
                .reopen()
                .map(|mut file| file.read_to_string(&mut errors));
            panic!("the mock server did not start:\n{errors}");
        };
        let ready: Value = serde_json::from_str(&ready).expect("the ready JSON");
        Ok(Self {
            child,
            url: ready["url"].as_str().expect("the url").to_owned(),
            api_key: ready["api_key"].as_str().expect("the API key").to_owned(),
            http: oscaraccountant::reqwest_client(TlsRoots::Bundled).expect("an HTTP client"),
        })
    }

    /// A client of the mock server with its API key, no environment, and no wait between retries.
    fn builder(&self) -> ClientBuilder {
        Client::builder()
            .env(|_| None)
            .base_url(&self.url)
            .api_key(&self.api_key)
            .sleep(|_| Box::pin(async {}))
    }

    fn client(&self) -> Client {
        self.builder().build().expect("a client")
    }

    /// A call of the control API, or of the OAuth server with a form body.
    async fn call(&self, method: &str, path: &str, body: Body) -> (u16, Value) {
        let url = format!("{}{path}", self.url);
        let request = match method {
            "GET" => self.http.get(url),
            "POST" => self.http.post(url),
            "DELETE" => self.http.delete(url),
            other => panic!("no method {other}"),
        };
        let request = match body {
            Body::None => request,
            Body::Json(value) => request
                .header("content-type", "application/json")
                .body(value.to_string()),
            Body::Form(pairs) => request
                .header("content-type", "application/x-www-form-urlencoded")
                .body(
                    url::form_urlencoded::Serializer::new(String::new())
                        .extend_pairs(pairs)
                        .finish(),
                ),
        };
        let response = request.send().await.expect("the mock server answers");
        let status = response.status().as_u16();
        let bytes = response.bytes().await.expect("a body");
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    async fn control(&self, method: &str, path: &str, body: Value) -> Value {
        let body = if body.is_null() {
            Body::None
        } else {
            Body::Json(body)
        };
        let (status, value) = self.call(method, &format!("/_mock/{path}"), body).await;
        assert!(status < 300, "{method} /_mock/{path}: {status} {value}");
        value
    }

    async fn reset(&self) {
        self.control("POST", "reset", Value::Null).await;
    }

    /// Script answers (`POST /_mock/responses`).
    async fn script(&self, responses: Value) {
        self.control("POST", "responses", json!({"responses": responses}))
            .await;
    }

    /// The request log.
    async fn requests(&self) -> Vec<Value> {
        let log = self.control("GET", "requests", Value::Null).await;
        log["requests"].as_array().expect("the log").clone()
    }

    /// Every logged API request was valid for `public.yaml`.
    async fn assert_valid_requests(&self) {
        for request in self.requests().await {
            assert_eq!(request["errors"], json!([]), "{request}");
        }
    }

    /// An access token from the device grant, approved through the control API.
    async fn device_token(&self) -> String {
        let resource = format!("{}/api", self.url);
        let (status, grant) = self
            .call(
                "POST",
                "/oauth/device_authorization",
                Body::Form(vec![
                    ("client_id", "oscar-cli".into()),
                    ("resource", resource.clone()),
                    ("scope", "read write".into()),
                ]),
            )
            .await;
        assert_eq!(status, 200, "{grant}");
        let device_code = grant["device_code"].as_str().expect("a device code");
        self.control(
            "POST",
            "device/approve",
            json!({"device_code": device_code}),
        )
        .await;
        let (status, token) = self
            .call(
                "POST",
                "/oauth/token",
                Body::Form(vec![
                    (
                        "grant_type",
                        "urn:ietf:params:oauth:grant-type:device_code".into(),
                    ),
                    ("device_code", device_code.to_owned()),
                    ("client_id", "oscar-cli".into()),
                    ("resource", resource),
                ]),
            )
            .await;
        assert_eq!(status, 200, "{token}");
        token["access_token"]
            .as_str()
            .expect("an access token")
            .to_owned()
    }
}

enum Body {
    None,
    Json(Value),
    Form(Vec<(&'static str, String)>),
}

impl Drop for MockServer {
    fn drop(&mut self) {
        // `uv run` may not pass a kill on to Python, so ask the server itself to stop first.
        let address = self.url.trim_start_matches("http://");
        if let Ok(mut stream) = TcpStream::connect(address) {
            let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
            let _ = write!(
                stream,
                "POST /_mock/shutdown HTTP/1.1\r\nHost: {address}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            );
            let _ = stream.read(&mut [0; 256]);
        }
        for _ in 0..50 {
            if matches!(self.child.try_wait(), Ok(Some(_))) {
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn the_sdk_against_the_mock_server() {
    let server = match MockServer::start() {
        Ok(server) => server,
        Err(reason) if std::env::var("OSCAR_MOCK_SERVER_REQUIRED").as_deref() != Ok("1") => {
            eprintln!("skipped the mock server suite: {reason}");
            return;
        }
        Err(reason) => panic!("OSCAR_MOCK_SERVER_REQUIRED=1, but {reason}"),
    };
    every_resource(&server).await;
    pagination(&server).await;
    errors(&server).await;
    retries(&server).await;
    idempotency(&server).await;
    files(&server).await;
    cli_login(&server).await;
}

fn revision() -> InvoiceRevisionRequest {
    let line = InvoiceLineRequest {
        description: Some(Some("Consulting".into())),
        net_minor: Some(Some(10_000)),
        vat_code: Some(Some("VAT24".into())),
        tax_minor: Some(Some(2_400)),
        ..InvoiceLineRequest::default()
    };
    let mut revision = InvoiceRevisionRequest::new("2026-09-01", "EUR", vec![line]);
    revision.due_date = Some(Some("2026-09-15".into()));
    revision
}

/// Every public operation once, so every resource; each request is valid for `public.yaml`.
async fn every_resource(server: &MockServer) {
    server.reset().await;
    let oscar = server.client();

    let me = oscar.me().retrieve().send().await.unwrap();
    assert_eq!(me.credential.kind.as_str(), "api_key");
    let organizations = oscar.organizations().list().send().await.unwrap();
    assert_eq!(organizations.items[0].id, ORGANIZATION);
    oscar
        .organizations()
        .retrieve(ORGANIZATION)
        .send()
        .await
        .unwrap();
    let companies = oscar.companies().list(ORGANIZATION).send().await.unwrap();
    assert_eq!(companies.items[0].id, COMPANY);
    oscar.companies().retrieve(COMPANY).send().await.unwrap();

    let accounts = oscar
        .accounts()
        .list(COMPANY)
        .limit(10)
        .send()
        .await
        .unwrap();
    assert!(!accounts.items.is_empty());
    oscar.bank_accounts().list(COMPANY).send().await.unwrap();
    let transactions = oscar
        .bank_transactions()
        .list(COMPANY)
        .date_from("2026-01-01")
        .to_explain(false)
        .send()
        .await
        .unwrap();
    oscar
        .bank_transactions()
        .retrieve(COMPANY, transactions.items[0].id.clone())
        .send()
        .await
        .unwrap();
    oscar.dimensions().list(COMPANY).send().await.unwrap();
    let documents = oscar.documents().list(COMPANY).send().await.unwrap();
    oscar
        .documents()
        .retrieve(COMPANY, documents.items[0].id.clone())
        .send()
        .await
        .unwrap();
    oscar
        .general_ledger()
        .retrieve(COMPANY)
        .date_from("2026-01-01")
        .date_to("2026-09-30")
        .send()
        .await
        .unwrap();
    let invoices = oscar
        .invoices()
        .list(COMPANY)
        .state("recorded")
        .send()
        .await
        .unwrap();
    assert_eq!(invoices.items[0].id, INVOICE);
    oscar
        .invoices()
        .retrieve(COMPANY, INVOICE)
        .send()
        .await
        .unwrap();
    oscar
        .open_items()
        .retrieve(COMPANY)
        .as_of("2026-09-30")
        .send()
        .await
        .unwrap();
    let parties = oscar
        .parties()
        .list(COMPANY)
        .search("Mock")
        .send()
        .await
        .unwrap();
    let party_id = match parties.items.first() {
        Some(party) => party.id.clone(),
        None => "01J90000000000000000000179".to_owned(),
    };
    oscar
        .parties()
        .retrieve(COMPANY, party_id.clone())
        .send()
        .await
        .unwrap();
    oscar.periods().list(COMPANY).send().await.unwrap();
    oscar
        .reports()
        .balance_sheet(COMPANY, "2026-09-30")
        .simplified(true)
        .send()
        .await
        .unwrap();
    oscar
        .reports()
        .profit_and_loss(COMPANY, "2026-01-01", "2026-09-30")
        .send()
        .await
        .unwrap();
    oscar
        .trial_balance()
        .retrieve(COMPANY)
        .period("2026-09")
        .send()
        .await
        .unwrap();
    oscar
        .vat_codes()
        .list(COMPANY)
        .direction("sales")
        .send()
        .await
        .unwrap();
    oscar
        .vat_preview()
        .retrieve(COMPANY, "2026-09-01", "2026-09-30")
        .send()
        .await
        .unwrap();

    // The invoice inbox.
    let inbox = oscar.inbox().retrieve(COMPANY).send().await.unwrap();
    assert!(inbox.address.ends_with("@inbox.oscaraccountant.com"));
    let emails = oscar.inbox().list_emails(COMPANY).send().await.unwrap();
    assert_eq!(emails.items.len(), 3);
    oscar.inbox().rotate(COMPANY).send().await.unwrap();

    // The commands.
    oscar
        .invoices()
        .record(
            COMPANY,
            RecordInvoicePayloadRequest::new("purchase", party_id, revision()),
        )
        .send()
        .await
        .unwrap();
    oscar
        .invoices()
        .issue(COMPANY, INVOICE)
        .send()
        .await
        .unwrap();
    oscar
        .invoices()
        .revise(
            COMPANY,
            INVOICE,
            ReviseInvoiceBodyRequest::new("Wrong due date", revision()),
        )
        .send()
        .await
        .unwrap();
    oscar
        .invoices()
        .cancel(
            COMPANY,
            INVOICE,
            CancelInvoiceBodyRequest::new("Entered twice"),
        )
        .send()
        .await
        .unwrap();
    let mut party = CreatePartyPayloadRequest::new("Mock Supplier OÜ", "supplier");
    party.country = Some(Some("EE".into()));
    oscar.parties().create(COMPANY, party).send().await.unwrap();

    // The files.
    let files = oscar.files().list(COMPANY).send().await.unwrap();
    let file_id = files.items[0].id.clone();
    oscar
        .files()
        .retrieve(COMPANY, file_id.clone())
        .send()
        .await
        .unwrap();
    oscar
        .files()
        .download(COMPANY, file_id)
        .send()
        .await
        .unwrap()
        .bytes()
        .await
        .unwrap();
    oscar
        .files()
        .upload(COMPANY, Upload::new("%PDF-1.4\n").file_name("a.pdf"))
        .send()
        .await
        .unwrap();

    let log = server.requests().await;
    assert_eq!(log.len(), 36, "one request per operation");
    for request in &log {
        assert_eq!(request["errors"], json!([]), "{request}");
        assert!(request["status"].as_u64().unwrap() < 300, "{request}");
    }
}

/// `.stream()` walks every page with the cursor of the previous one; `.pages()` gives the pages.
async fn pagination(server: &MockServer) {
    server.reset().await;
    let oscar = server.client();
    let invoices: Vec<_> = oscar
        .invoices()
        .list(COMPANY)
        .limit(1)
        .stream()
        .try_collect()
        .await
        .unwrap();
    assert_eq!(invoices.len(), 3);
    let log = server.requests().await;
    assert_eq!(log.len(), 3, "one request per page");
    assert_eq!(log[0]["query"], json!([["limit", "1"]]));
    assert!(
        log[1]["query"]
            .as_array()
            .unwrap()
            .iter()
            .any(|pair| pair[0] == "cursor")
    );

    let pages: Vec<_> = oscar
        .invoices()
        .list(COMPANY)
        .limit(2)
        .stream()
        .pages()
        .try_collect()
        .await
        .unwrap();
    assert_eq!(pages.len(), 2);
    assert_eq!(pages[0].items.len(), 2);
    assert_eq!(pages[1].next_cursor, None);

    // The stream fetches nothing until it is polled, and stops at the first item asked for.
    server.control("DELETE", "requests", Value::Null).await;
    let mut stream = oscar.invoices().list(COMPANY).limit(1).stream();
    assert!(server.requests().await.is_empty());
    stream.next().await.unwrap().unwrap();
    assert_eq!(server.requests().await.len(), 1);
    server.assert_valid_requests().await;
}

/// Each error status of the mock server becomes its variant, with the parsed problem.
async fn errors(server: &MockServer) {
    server.reset().await;
    let oscar = server.client();

    let error = oscar
        .invoices()
        .retrieve(COMPANY, "01J9ZZZZZZZZZZZZZZZZZZZZZZ")
        .send()
        .await
        .unwrap_err();
    assert!(matches!(error, Error::NotFound(_)), "{error:?}");
    assert_eq!(error.code(), Some("not_found"));

    let error = oscar
        .companies()
        .retrieve("cmp9999999")
        .send()
        .await
        .unwrap_err();
    assert!(matches!(error, Error::NotFound(_)), "{error:?}");

    let error = oscar
        .invoices()
        .list(COMPANY)
        .cursor("zz")
        .send()
        .await
        .unwrap_err();
    assert!(matches!(error, Error::BadRequest(_)), "{error:?}");
    assert_eq!(error.code(), Some("validation_error"));
    assert!(!error.api_error().unwrap().errors.is_empty());

    let wrong_key = server.builder().api_key("osk_test_wrong").build().unwrap();
    let error = wrong_key.me().retrieve().send().await.unwrap_err();
    assert!(
        matches!(&error, Error::Authentication(auth) if auth.api.is_some()),
        "{error:?}"
    );
    assert_eq!(error.code(), Some("authentication_failed"));

    let read_only = server
        .control("POST", "api-keys", json!({"scope": "read"}))
        .await;
    let read_only = server
        .builder()
        .api_key(read_only["key"].as_str().unwrap())
        .build()
        .unwrap();
    read_only.me().retrieve().send().await.unwrap();
    let error = read_only
        .invoices()
        .cancel(COMPANY, INVOICE, CancelInvoiceBodyRequest::new("No scope"))
        .send()
        .await
        .unwrap_err();
    assert!(matches!(error, Error::PermissionDenied(_)), "{error:?}");
    assert_eq!(error.code(), Some("insufficient_scope"));

    // No credential at all: nothing is sent.
    server.control("DELETE", "requests", Value::Null).await;
    let anonymous = Client::builder()
        .env(|_| None)
        .base_url(&server.url)
        .build()
        .unwrap();
    let error = anonymous.me().retrieve().send().await.unwrap_err();
    assert!(
        matches!(&error, Error::Authentication(auth) if auth.api.is_none()),
        "{error:?}"
    );
    assert!(server.requests().await.is_empty());
}

/// The retried statuses and faults, and the limits.
async fn retries(server: &MockServer) {
    server.reset().await;
    let sleeps = Arc::new(Mutex::new(Vec::new()));
    let recorded = sleeps.clone();
    let oscar = server
        .builder()
        .jitter(|| 0.0)
        .sleep(move |duration| {
            recorded.lock().unwrap().push(duration);
            Box::pin(async {})
        })
        .build()
        .unwrap();
    let busy = json!({"status": 503, "body": {"kind": "json", "value": {
        "type": "about:blank", "title": "Service Unavailable", "status": 503,
        "code": "service_unavailable", "detail": "Busy.", "errors": []}}});

    // Two 503 answers, then the real one.
    server
        .script(json!([{"match": {"path": "/api/me/"}, "times": 2, "response": busy}]))
        .await;
    oscar.me().retrieve().send().await.unwrap();
    assert_eq!(server.requests().await.len(), 3);
    assert_eq!(
        *sleeps.lock().unwrap(),
        [Duration::from_millis(500), Duration::from_secs(1)]
    );

    // A closed connection, then the real answer.
    server.reset().await;
    server
        .script(json!([{"match": {"path": "/api/me/"}, "fault": "close"}]))
        .await;
    oscar.me().retrieve().send().await.unwrap();
    assert_eq!(server.requests().await.len(), 2);

    // The retries run out: the last answer is the error.
    server.reset().await;
    server
        .script(json!([{"match": {"path": "/api/me/"}, "times": 3, "response": busy}]))
        .await;
    let error = oscar.me().retrieve().send().await.unwrap_err();
    assert!(matches!(error, Error::InternalServer(_)), "{error:?}");
    assert_eq!(error.status(), Some(503));
    assert_eq!(server.requests().await.len(), 3);

    // A slow answer is a timeout; with no retry, the call fails at once.
    server.reset().await;
    server
        .script(json!([{"match": {"path": "/api/me/"}, "delay_ms": 1500}]))
        .await;
    let error = oscar
        .me()
        .retrieve()
        .timeout(Duration::from_millis(200))
        .max_retries(0)
        .send()
        .await
        .unwrap_err();
    assert!(matches!(error, Error::Timeout(_)), "{error:?}");
    assert!(error.is_network());

    // A raw POST without a key is never sent twice.
    server.reset().await;
    server
        .script(json!([{"match": {"path": "/api/me/"}, "response": busy}]))
        .await;
    let error = oscar
        .request(HttpMethod::Post, "/api/me/")
        .send()
        .await
        .unwrap_err();
    assert_eq!(error.status(), Some(503));
    assert_eq!(server.requests().await.len(), 1);
}

/// The automatic key, the caller's key, the replay, the conflict, and the key on a retry.
async fn idempotency(server: &MockServer) {
    server.reset().await;
    let oscar = server.client();
    let cancel = |reason: &str| {
        oscar
            .invoices()
            .cancel(COMPANY, INVOICE, CancelInvoiceBodyRequest::new(reason))
    };

    let first = cancel("Entered twice")
        .idempotency_key("cancel-1")
        .send()
        .await
        .unwrap();
    assert!(!first.replayed);
    let again = cancel("Entered twice")
        .idempotency_key("cancel-1")
        .send()
        .await
        .unwrap();
    assert!(again.replayed);
    let error = cancel("Another reason")
        .idempotency_key("cancel-1")
        .send()
        .await
        .unwrap_err();
    assert!(matches!(error, Error::Conflict(_)), "{error:?}");
    assert_eq!(error.code(), Some("idempotency_conflict"));

    // Two calls without a key get two different automatic keys.
    server.control("DELETE", "requests", Value::Null).await;
    cancel("Entered twice").send().await.unwrap();
    cancel("Entered twice").send().await.unwrap();
    let keys: Vec<Value> = server
        .requests()
        .await
        .iter()
        .map(|request| request["headers"]["idempotency-key"].clone())
        .collect();
    assert!(keys[0].is_string());
    assert_ne!(keys[0], keys[1]);

    // A retry sends the same key.
    server.reset().await;
    server
        .script(json!([{"match": {"operation_id": "commands_cancel_invoice"}, "fault": "close"}]))
        .await;
    cancel("Entered twice").send().await.unwrap();
    let log = server.requests().await;
    assert_eq!(log.len(), 2);
    assert_eq!(
        log[0]["headers"]["idempotency-key"],
        log[1]["headers"]["idempotency-key"]
    );
    server.assert_valid_requests().await;
}

/// An upload with every field, then the same file back.
async fn files(server: &MockServer) {
    server.reset().await;
    let oscar = server.client();
    let content = b"%PDF-1.4\n% an uploaded receipt\n".to_vec();
    let stored = oscar
        .files()
        .upload(
            COMPANY,
            Upload::new(content.clone())
                .file_name("receipt.pdf")
                .content_type("application/pdf"),
        )
        .kind("receipt")
        .received_on("2026-09-20")
        .send()
        .await
        .unwrap();
    assert_eq!(stored.name, "receipt.pdf");
    let log = server.requests().await;
    let parts = log[0]["body"]["parts"].as_array().unwrap();
    let file = parts.iter().find(|part| part["name"] == "file").unwrap();
    assert_eq!(file["filename"], "receipt.pdf");
    assert_eq!(file["content_type"], "application/pdf");

    let listed = oscar.files().list(COMPANY).send().await.unwrap();
    assert_eq!(listed.items[0].id, stored.id, "the newest file first");

    let download = oscar
        .files()
        .download(COMPANY, stored.id.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(download.file_name(), Some("receipt.pdf"));
    assert_eq!(download.content_type(), Some("application/pdf"));
    assert_eq!(download.bytes().await.unwrap().to_vec(), content);

    // An upload from a file on disk.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("statement.csv");
    std::fs::write(&path, "date,amount\n").unwrap();
    let stored = oscar
        .files()
        .upload(COMPANY, Upload::from_path(&path).await.unwrap())
        .kind("statement")
        .send()
        .await
        .unwrap();
    assert_eq!(stored.name, "statement.csv");
    server.assert_valid_requests().await;
}

/// A fake `oscar` in its own directory: it answers `--version`, and `auth token` with the JSON of
/// the file `answer.json` next to it. Each call is a line of the file `calls`.
struct FakeCli {
    dir: tempfile::TempDir,
}

impl FakeCli {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let script = "#!/bin/sh\n\
                dir=$(dirname \"$0\")\n\
                echo \"$*\" >> \"$dir/calls\"\n\
                if [ \"$1\" = \"--version\" ]; then echo 'oscar-cli 0.1.0'; exit 0; fi\n\
                cat \"$dir/answer.json\"\n";
            let program = dir.path().join("oscar");
            std::fs::write(&program, script).unwrap();
            std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        #[cfg(windows)]
        {
            let script = "@echo off\r\n\
                echo %* >> \"%~dp0calls\"\r\n\
                if \"%1\"==\"--version\" (echo oscar-cli 0.1.0& exit /b 0)\r\n\
                type \"%~dp0answer.json\"\r\n";
            std::fs::write(dir.path().join("oscar.cmd"), script).unwrap();
        }
        Self { dir }
    }

    fn answer(&self, token: &str, base_url: &str) {
        let answer = json!({"token": token, "expires_at": "2099-01-01T00:00:00Z", "kind": "oauth",
                            "base_url": base_url, "profile": "default"});
        std::fs::write(self.dir.path().join("answer.json"), answer.to_string()).unwrap();
    }

    fn calls(&self) -> Vec<String> {
        std::fs::read_to_string(self.dir.path().join("calls"))
            .unwrap_or_default()
            .lines()
            .map(|line| line.trim().to_owned())
            .collect()
    }

    /// The environment of the client: this directory is the whole `PATH`.
    fn env(&self) -> impl Fn(&str) -> Option<String> + Send + Sync + 'static {
        let path = self.dir.path().display().to_string();
        let pathext = std::env::var("PATHEXT").ok();
        move |name| match name {
            "PATH" => Some(path.clone()),
            "PATHEXT" => pathext.clone(),
            "OSCAR_USE_CLI_LOGIN" => Some("1".to_owned()),
            _ => None,
        }
    }
}

/// Step 3 of the credentials: the token of the CLI login, and a new one after a 401.
async fn cli_login(server: &MockServer) {
    server.reset().await;
    server
        .control("POST", "config", json!({"device_interval": 0}))
        .await;
    let cli = FakeCli::new();
    let first = server.device_token().await;
    cli.answer(&first, &server.url);
    server.control("DELETE", "requests", Value::Null).await;
    // No API key: `OSCAR_USE_CLI_LOGIN=1` in the environment turns the CLI login on.
    let oscar = Client::builder()
        .env(cli.env())
        .base_url(&server.url)
        .build()
        .unwrap();

    let me = oscar.me().retrieve().send().await.unwrap();
    assert_eq!(me.credential.kind.as_str(), "oauth");
    oscar.organizations().list().send().await.unwrap();
    let calls = cli.calls();
    assert_eq!(calls[0], "--version");
    assert_eq!(
        calls[1..],
        [format!("auth token --json --base-url {}", server.url)],
        "the token is cached"
    );
    let log = server.requests().await;
    assert_eq!(
        log[0]["headers"]["authorization"],
        json!(format!("Bearer {first}"))
    );

    // The token expires, and the CLI has a new one: a 401 asks the CLI again, once.
    server
        .control("POST", "tokens/expire", json!({"token": first}))
        .await;
    let second = server.device_token().await;
    cli.answer(&second, &server.url);
    server.control("DELETE", "requests", Value::Null).await;
    oscar.me().retrieve().send().await.unwrap();
    assert_eq!(cli.calls().len(), 3);
    let log = server.requests().await;
    assert_eq!(log.len(), 2);
    assert_eq!(log[0]["status"], 401);
    assert_eq!(
        log[1]["headers"]["authorization"],
        json!(format!("Bearer {second}"))
    );

    // The explicit form of the same step.
    let explicit = Client::builder()
        .env(cli.env())
        .credential(Credential::Cli)
        .base_url(&server.url)
        .build()
        .unwrap();
    explicit.me().retrieve().send().await.unwrap();
}
