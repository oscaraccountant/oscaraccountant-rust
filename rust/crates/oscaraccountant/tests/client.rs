//! The client against a fake transport: the headers, the credentials, the retries, the
//! idempotency keys, the errors, the files, the pagination and the raw calls.

mod common;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::{
    FakeTransport, Reply, builder, builder_with_env, organization, outcome, stored_document,
};
use futures::{StreamExt, TryStreamExt};
use oscaraccountant::{
    BoxError, CancelInvoiceBodyRequest, Client, Credential, Error, HttpMethod, TlsRoots,
    TransportError, Upload,
};
use serde_json::json;

fn sleeps() -> Arc<Mutex<Vec<Duration>>> {
    Arc::default()
}

fn header<'a>(request: &'a oscaraccountant::HttpRequest, name: &str) -> Option<&'a str> {
    request.header(name)
}

#[tokio::test]
async fn a_get_sends_the_headers_and_the_query() {
    let transport = FakeTransport::new(vec![Reply::json(
        200,
        json!({"items": [organization("o1")], "next_cursor": null}),
    )]);
    let client = builder(&transport, &sleeps()).build().unwrap();
    let page = client
        .organizations()
        .list()
        .limit(50)
        .cursor("a b&c")
        .send()
        .await
        .unwrap();
    assert_eq!(page.items[0].id, "o1");
    let request = &transport.requests()[0];
    assert_eq!(request.method, HttpMethod::Get);
    assert_eq!(
        request.url,
        "https://api.example.com/api/organizations/?cursor=a%20b%26c&limit=50"
    );
    assert_eq!(header(request, "authorization"), Some("Bearer osk_test"));
    assert_eq!(header(request, "accept"), Some("application/json"));
    assert!(
        header(request, "user-agent")
            .unwrap()
            .starts_with("oscaraccountant-rust/")
    );
    assert_eq!(header(request, "content-type"), None);
    assert_eq!(header(request, "idempotency-key"), None);
    assert!(request.body.is_none());
}

#[tokio::test]
async fn a_path_value_is_encoded() {
    let transport = FakeTransport::new(vec![Reply::json(200, organization("o/1"))]);
    let client = builder(&transport, &sleeps()).build().unwrap();
    client
        .organizations()
        .retrieve("o/1 ö")
        .send()
        .await
        .unwrap();
    assert_eq!(
        transport.requests()[0].url,
        "https://api.example.com/api/organizations/o%2F1%20%C3%B6/"
    );
}

#[tokio::test]
async fn extra_headers_never_replace_the_credential_or_the_key() {
    let transport = FakeTransport::new(vec![Reply::json(200, outcome())]);
    let client = builder(&transport, &sleeps())
        .user_agent("custom/1.0")
        .build()
        .unwrap();
    client
        .invoices()
        .cancel("c1", "i1", CancelInvoiceBodyRequest::new("duplicate"))
        .header("Authorization", "Bearer stolen")
        .header("Idempotency-Key", "mine")
        .header("Accept", "application/problem+json")
        .header("X-Trace", "t1")
        .send()
        .await
        .unwrap();
    let request = &transport.requests()[0];
    assert_eq!(header(request, "authorization"), Some("Bearer osk_test"));
    assert_eq!(header(request, "idempotency-key"), Some("uuid-1"));
    assert_eq!(header(request, "accept"), Some("application/problem+json"));
    assert_eq!(header(request, "x-trace"), Some("t1"));
    assert_eq!(header(request, "user-agent"), Some("custom/1.0"));
    assert_eq!(header(request, "content-type"), Some("application/json"));
    let body: serde_json::Value = serde_json::from_slice(request.body.as_ref().unwrap()).unwrap();
    assert_eq!(body, json!({"reason": "duplicate"}));
    assert_eq!(
        request
            .headers
            .iter()
            .filter(|(name, _)| name == "authorization")
            .count(),
        1
    );
}

#[tokio::test]
async fn the_caller_may_choose_the_idempotency_key() {
    let transport = FakeTransport::new(vec![
        Reply::json(200, outcome()),
        Reply::json(200, outcome()),
    ]);
    let client = builder(&transport, &sleeps()).build().unwrap();
    let body = || CancelInvoiceBodyRequest::new("x");
    client
        .invoices()
        .cancel("c1", "i1", body())
        .idempotency_key("key-7")
        .send()
        .await
        .unwrap();
    client
        .invoices()
        .cancel("c1", "i1", body())
        .idempotency_key("")
        .send()
        .await
        .unwrap();
    let requests = transport.requests();
    assert_eq!(header(&requests[0], "idempotency-key"), Some("key-7"));
    assert_eq!(header(&requests[1], "idempotency-key"), Some("uuid-1"));
}

#[tokio::test]
async fn the_base_url_comes_from_the_option_then_the_environment_then_the_default() {
    let transport = FakeTransport::default();
    let vars = [("OSCAR_BASE_URL", "http://localhost:3500/")];
    let client = builder_with_env(&transport, &sleeps(), &vars)
        .build()
        .unwrap();
    assert_eq!(client.base_url(), "https://api.example.com");
    let client = builder_with_env(&transport, &sleeps(), &vars)
        .base_url("")
        .build()
        .unwrap();
    assert_eq!(client.base_url(), "http://localhost:3500");
    let client = builder_with_env(&transport, &sleeps(), &[])
        .base_url("")
        .build()
        .unwrap();
    assert_eq!(client.base_url(), oscaraccountant::DEFAULT_BASE_URL);
    let error = builder(&transport, &sleeps())
        .base_url("http://api.example.com")
        .build()
        .unwrap_err();
    assert!(matches!(error, Error::Config(_)));
    assert!(format!("{client:?}").contains("api_key"));
    assert!(!format!("{client:?}").contains("osk_test"));
    assert!(!format!("{:?}", builder(&transport, &sleeps())).contains("osk_test"));
}

#[tokio::test]
async fn without_a_credential_nothing_is_sent() {
    let transport = FakeTransport::default();
    let client = builder(&transport, &sleeps()).api_key("").build().unwrap();
    let error = client.me().retrieve().send().await.unwrap_err();
    assert!(matches!(error, Error::Authentication(_)), "{error:?}");
    assert!(error.to_string().contains("OSCAR_API_KEY"));
    assert!(transport.requests().is_empty());
}

#[tokio::test]
async fn the_environment_key_is_used_when_no_key_is_given() {
    let transport = FakeTransport::new(vec![Reply::json(200, organization("o1"))]);
    let client = builder_with_env(&transport, &sleeps(), &[("OSCAR_API_KEY", "osk_env")])
        .api_key("")
        .build()
        .unwrap();
    client.organizations().retrieve("o1").send().await.unwrap();
    assert_eq!(
        header(&transport.requests()[0], "authorization"),
        Some("Bearer osk_env")
    );
}

#[tokio::test]
async fn the_token_provider_is_called_before_each_attempt() {
    let transport = FakeTransport::new(vec![
        Reply::problem(503, "unavailable"),
        Reply::json(200, organization("o1")),
    ]);
    let calls = Arc::new(AtomicUsize::new(0));
    let counter = calls.clone();
    let client = builder(&transport, &sleeps())
        .token_provider(move || {
            let call = counter.fetch_add(1, Ordering::SeqCst) + 1;
            async move { Ok::<_, BoxError>(format!("token-{call}")) }
        })
        .build()
        .unwrap();
    client.organizations().retrieve("o1").send().await.unwrap();
    let requests = transport.requests();
    assert_eq!(
        header(&requests[0], "authorization"),
        Some("Bearer token-1")
    );
    assert_eq!(
        header(&requests[1], "authorization"),
        Some("Bearer token-2")
    );
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn a_failing_token_provider_sends_nothing() {
    let transport = FakeTransport::default();
    let client = builder(&transport, &sleeps())
        .credential(Credential::TokenProvider(Arc::new(|| async {
            Err::<String, _>("expired session")
        })))
        .build()
        .unwrap();
    let error = client.me().retrieve().send().await.unwrap_err();
    assert_eq!(
        error.to_string(),
        "The token provider failed: expired session"
    );
    assert!(transport.requests().is_empty());
}

#[tokio::test]
async fn a_get_is_retried_with_backoff() {
    let transport = FakeTransport::new(vec![
        Reply::problem(503, "unavailable"),
        Reply::Error(TransportError::Connection("reset".into())),
        Reply::json(200, organization("o1")),
    ]);
    let sleeps = sleeps();
    let client = builder(&transport, &sleeps).build().unwrap();
    client.organizations().retrieve("o1").send().await.unwrap();
    assert_eq!(transport.requests().len(), 3);
    assert_eq!(
        *sleeps.lock().unwrap(),
        vec![Duration::from_millis(500), Duration::from_secs(1)]
    );
}

#[tokio::test]
async fn the_jitter_shortens_the_delay() {
    let transport = FakeTransport::new(vec![
        Reply::problem(502, "bad_gateway"),
        Reply::json(200, organization("o1")),
    ]);
    let sleeps = sleeps();
    let client = builder(&transport, &sleeps).jitter(|| 0.5).build().unwrap();
    client.organizations().retrieve("o1").send().await.unwrap();
    assert_eq!(
        *sleeps.lock().unwrap(),
        vec![Duration::from_micros(437_500)]
    );
}

#[tokio::test]
async fn retry_after_wins_and_a_long_one_ends_the_retries() {
    let transport = FakeTransport::new(vec![
        Reply::problem(429, "throttled").with_header("retry-after", "3"),
        Reply::problem(429, "throttled")
            .with_header("retry-after", "Sun, 27 Sep 2026 10:00:05 GMT"),
        Reply::json(200, organization("o1")),
    ]);
    let sleeps = sleeps();
    let client = builder(&transport, &sleeps).build().unwrap();
    client.organizations().retrieve("o1").send().await.unwrap();
    assert_eq!(
        *sleeps.lock().unwrap(),
        vec![Duration::from_secs(3), Duration::from_secs(5)]
    );

    let transport = FakeTransport::new(vec![
        Reply::problem(429, "throttled").with_header("retry-after", "120"),
    ]);
    let sleeps = self::sleeps();
    let client = builder(&transport, &sleeps).build().unwrap();
    let error = client
        .organizations()
        .retrieve("o1")
        .send()
        .await
        .unwrap_err();
    assert!(matches!(error, Error::RateLimit(_)));
    assert_eq!(transport.requests().len(), 1);
    assert!(sleeps.lock().unwrap().is_empty());
}

#[tokio::test]
async fn the_retries_stop_after_the_maximum() {
    let transport = FakeTransport::new(vec![
        Reply::problem(500, "server_error"),
        Reply::problem(500, "server_error"),
        Reply::problem(500, "server_error"),
    ]);
    let client = builder(&transport, &sleeps()).build().unwrap();
    let error = client
        .organizations()
        .retrieve("o1")
        .send()
        .await
        .unwrap_err();
    assert!(matches!(error, Error::InternalServer(_)));
    assert_eq!(error.request_id(), Some("req-9"));
    assert_eq!(transport.requests().len(), 3);

    let transport = FakeTransport::new(vec![Reply::problem(500, "server_error")]);
    let client = builder(&transport, &sleeps())
        .max_retries(0)
        .build()
        .unwrap();
    client
        .organizations()
        .retrieve("o1")
        .send()
        .await
        .unwrap_err();
    assert_eq!(transport.requests().len(), 1);

    let transport = FakeTransport::new(vec![
        Reply::problem(500, "server_error"),
        Reply::problem(500, "server_error"),
    ]);
    let client = builder(&transport, &sleeps())
        .max_retries(0)
        .build()
        .unwrap();
    client
        .organizations()
        .retrieve("o1")
        .max_retries(1)
        .send()
        .await
        .unwrap_err();
    assert_eq!(transport.requests().len(), 2);
}

#[tokio::test]
async fn an_idempotent_post_is_retried_with_the_same_key() {
    let transport = FakeTransport::new(vec![
        Reply::problem(409, "idempotency_in_progress"),
        Reply::Error(TransportError::Timeout),
        Reply::json(200, outcome()),
    ]);
    let client = builder(&transport, &sleeps()).build().unwrap();
    let result = client
        .invoices()
        .cancel("c1", "i1", CancelInvoiceBodyRequest::new("x"))
        .send()
        .await
        .unwrap();
    assert_eq!(result.commit_seq, 7);
    let keys: Vec<_> = transport
        .requests()
        .iter()
        .map(|r| r.header("idempotency-key").unwrap().to_owned())
        .collect();
    assert_eq!(keys, vec!["uuid-1", "uuid-1", "uuid-1"]);
}

#[tokio::test]
async fn a_conflict_that_is_not_in_progress_is_not_retried() {
    let transport = FakeTransport::new(vec![Reply::problem(409, "idempotency_conflict")]);
    let client = builder(&transport, &sleeps()).build().unwrap();
    let error = client
        .invoices()
        .cancel("c1", "i1", CancelInvoiceBodyRequest::new("x"))
        .send()
        .await
        .unwrap_err();
    assert!(matches!(error, Error::Conflict(_)));
    assert_eq!(error.code(), Some("idempotency_conflict"));
    assert_eq!(transport.requests().len(), 1);
}

#[tokio::test]
async fn a_post_without_a_key_is_sent_once() {
    let transport = FakeTransport::new(vec![Reply::problem(503, "unavailable")]);
    let client = builder(&transport, &sleeps()).build().unwrap();
    let error = client
        .request(HttpMethod::Post, "/api/things/")
        .send()
        .await
        .unwrap_err();
    assert!(matches!(error, Error::InternalServer(_)));
    assert_eq!(transport.requests().len(), 1);
}

#[tokio::test]
async fn a_connection_error_after_the_last_retry() {
    let transport = FakeTransport::new(vec![
        Reply::Error(TransportError::Connection("refused".into())),
        Reply::Error(TransportError::Connection("refused".into())),
    ]);
    let client = builder(&transport, &sleeps())
        .max_retries(1)
        .build()
        .unwrap();
    let error = client
        .organizations()
        .retrieve("o1")
        .send()
        .await
        .unwrap_err();
    assert!(matches!(&error, Error::Connection(message) if message == "refused"));
    assert!(error.is_network());
}

#[tokio::test(start_paused = true)]
async fn an_attempt_that_hangs_times_out() {
    let transport = FakeTransport::new(vec![Reply::Hang, Reply::Hang]);
    let client = builder(&transport, &sleeps())
        .timeout(Duration::from_secs(5))
        .max_retries(1)
        .build()
        .unwrap();
    let error = client
        .organizations()
        .retrieve("o1")
        .send()
        .await
        .unwrap_err();
    assert!(matches!(error, Error::Timeout(timeout) if timeout == Duration::from_secs(5)));
    assert_eq!(transport.requests().len(), 2);

    let transport = FakeTransport::new(vec![Reply::Hang]);
    let client = builder(&transport, &sleeps())
        .max_retries(0)
        .build()
        .unwrap();
    let error = client
        .organizations()
        .retrieve("o1")
        .timeout(Duration::from_millis(10))
        .send()
        .await
        .unwrap_err();
    assert!(matches!(error, Error::Timeout(timeout) if timeout == Duration::from_millis(10)));
}

#[tokio::test]
async fn each_status_has_its_error() {
    type Check = fn(&Error) -> bool;
    let cases: Vec<(u16, Check)> = vec![
        (400, |e| matches!(e, Error::BadRequest(_))),
        (401, |e| matches!(e, Error::Authentication(_))),
        (403, |e| matches!(e, Error::PermissionDenied(_))),
        (404, |e| matches!(e, Error::NotFound(_))),
        (422, |e| matches!(e, Error::UnprocessableEntity(_))),
        (418, |e| matches!(e, Error::Api(_))),
    ];
    for (status, check) in cases {
        let transport = FakeTransport::new(vec![Reply::problem(status, "some_code")]);
        let client = builder(&transport, &sleeps()).build().unwrap();
        let error = client
            .organizations()
            .retrieve("o1")
            .send()
            .await
            .unwrap_err();
        assert!(check(&error), "{status}: {error:?}");
        assert_eq!(
            error.to_string(),
            format!("{status} some_code: some_code happened (request req-9)")
        );
    }
}

#[tokio::test]
async fn a_body_that_is_not_the_type_is_a_json_error() {
    let transport = FakeTransport::new(vec![Reply::json(200, json!({"unexpected": true}))]);
    let client = builder(&transport, &sleeps()).build().unwrap();
    let error = client
        .organizations()
        .retrieve("o1")
        .send()
        .await
        .unwrap_err();
    assert!(matches!(error, Error::Json(_)));
}

#[tokio::test]
async fn an_upload_is_multipart() {
    let transport = FakeTransport::new(vec![Reply::json(201, stored_document())]);
    let client = builder(&transport, &sleeps()).build().unwrap();
    let upload = Upload::new(&b"%PDF"[..])
        .file_name("a.pdf")
        .content_type("application/pdf");
    let document = client
        .files()
        .upload("c1", upload)
        .kind("invoice")
        .received_on("2026-09-27")
        .send()
        .await
        .unwrap();
    assert_eq!(document.id, "f1");
    let request = &transport.requests()[0];
    assert_eq!(request.method, HttpMethod::Post);
    assert_eq!(
        request.url,
        "https://api.example.com/api/companies/c1/files/"
    );
    let content_type = header(request, "content-type").unwrap();
    let boundary = content_type
        .strip_prefix("multipart/form-data; boundary=")
        .unwrap();
    let body = std::str::from_utf8(request.body.as_ref().unwrap()).unwrap();
    assert!(body.starts_with(&format!("--{boundary}\r\n")));
    assert!(body.contains(
        "name=\"file\"; filename=\"a.pdf\"\r\nContent-Type: application/pdf\r\n\r\n%PDF\r\n"
    ));
    assert!(body.contains("name=\"kind\"\r\n\r\ninvoice\r\n"));
    assert!(body.contains("name=\"received_on\"\r\n\r\n2026-09-27\r\n"));
    assert!(!body.contains("bank_transaction_id"));
    assert!(body.ends_with(&format!("--{boundary}--\r\n")));
    assert_eq!(header(request, "idempotency-key"), Some("uuid-1"));
}

#[tokio::test]
async fn a_retried_upload_sends_the_same_bytes() {
    let transport = FakeTransport::new(vec![
        Reply::problem(503, "unavailable"),
        Reply::json(201, stored_document()),
    ]);
    let client = builder(&transport, &sleeps()).build().unwrap();
    client
        .files()
        .upload("c1", Upload::new("abc"))
        .send()
        .await
        .unwrap();
    let requests = transport.requests();
    assert_eq!(requests[0].body, requests[1].body);
    assert_eq!(
        requests[0].header("content-type"),
        requests[1].header("content-type")
    );
}

#[tokio::test]
async fn a_download_is_a_stream_with_its_metadata() {
    let transport = FakeTransport::new(vec![Reply::Response(
        200,
        vec![
            ("content-type", "application/pdf".into()),
            ("content-length", "4".into()),
            (
                "content-disposition",
                "inline; filename=\"arve.pdf\"".into(),
            ),
        ],
        b"%PDF".to_vec(),
    )]);
    let client = builder(&transport, &sleeps()).build().unwrap();
    let download = client.files().download("c1", "f1").send().await.unwrap();
    assert_eq!(download.content_type(), Some("application/pdf"));
    assert_eq!(download.content_length(), Some(4));
    assert_eq!(download.file_name(), Some("arve.pdf"));
    assert_eq!(download.bytes().await.unwrap().as_ref(), b"%PDF");
    let request = &transport.requests()[0];
    assert_eq!(header(request, "accept"), Some("*/*"));
    assert_eq!(
        request.url,
        "https://api.example.com/api/companies/c1/files/f1/content/"
    );
}

#[tokio::test]
async fn a_failed_download_is_an_error_with_its_body() {
    let transport = FakeTransport::new(vec![Reply::problem(404, "not_found")]);
    let client = builder(&transport, &sleeps()).build().unwrap();
    let error = client
        .files()
        .download("c1", "f1")
        .send()
        .await
        .unwrap_err();
    assert!(matches!(error, Error::NotFound(_)));
    assert!(
        error
            .api_error()
            .unwrap()
            .body
            .contains("not_found happened")
    );
}

#[tokio::test]
async fn the_stream_walks_every_page() {
    let transport = FakeTransport::new(vec![
        Reply::json(
            200,
            json!({"items": [organization("o1"), organization("o2")], "next_cursor": "c2"}),
        ),
        Reply::json(
            200,
            json!({"items": [organization("o3")], "next_cursor": null}),
        ),
    ]);
    let client = builder(&transport, &sleeps()).build().unwrap();
    let stream = client
        .organizations()
        .list()
        .limit(2)
        .cursor("start")
        .stream();
    assert!(format!("{stream:?}").contains("PageStream"));
    assert!(
        transport.requests().is_empty(),
        "nothing is fetched before the first poll"
    );
    let items: Vec<_> = stream.try_collect().await.unwrap();
    let ids: Vec<_> = items.iter().map(|item| item.id.as_str()).collect();
    assert_eq!(ids, vec!["o1", "o2", "o3"]);
    let urls: Vec<_> = transport.requests().iter().map(|r| r.url.clone()).collect();
    assert_eq!(
        urls,
        vec![
            "https://api.example.com/api/organizations/?cursor=start&limit=2",
            "https://api.example.com/api/organizations/?limit=2&cursor=c2",
        ]
    );
}

#[tokio::test]
async fn the_stream_fetches_a_page_only_when_needed() {
    let transport = FakeTransport::new(vec![
        Reply::json(
            200,
            json!({"items": [organization("o1")], "next_cursor": "c2"}),
        ),
        Reply::json(
            200,
            json!({"items": [organization("o2")], "next_cursor": ""}),
        ),
    ]);
    let client = builder(&transport, &sleeps()).build().unwrap();
    let mut stream = client.organizations().list().stream();
    assert_eq!(stream.next().await.unwrap().unwrap().id, "o1");
    assert_eq!(transport.requests().len(), 1);
    assert_eq!(stream.next().await.unwrap().unwrap().id, "o2");
    assert!(
        stream.next().await.is_none(),
        "an empty cursor ends the walk"
    );
    assert!(stream.next().await.is_none());
    assert_eq!(transport.requests().len(), 2);
}

#[tokio::test]
async fn the_stream_ends_with_an_error() {
    let transport = FakeTransport::new(vec![
        Reply::json(
            200,
            json!({"items": [organization("o1")], "next_cursor": "c2"}),
        ),
        Reply::problem(403, "forbidden"),
    ]);
    let client = builder(&transport, &sleeps()).build().unwrap();
    let results: Vec<_> = client.organizations().list().stream().collect().await;
    assert_eq!(results.len(), 2);
    assert!(results[0].is_ok());
    assert!(matches!(results[1], Err(Error::PermissionDenied(_))));

    let transport = FakeTransport::new(vec![Reply::json(200, json!("not a page"))]);
    let client = builder(&transport, &sleeps()).build().unwrap();
    let results: Vec<_> = client.organizations().list().stream().collect().await;
    assert!(matches!(results[..], [Err(Error::Json(_))]));
}

#[tokio::test]
async fn the_pages_of_a_list() {
    let transport = FakeTransport::new(vec![
        Reply::json(
            200,
            json!({"items": [organization("o1")], "next_cursor": "c2"}),
        ),
        Reply::json(200, json!({"items": [], "next_cursor": null})),
    ]);
    let client = builder(&transport, &sleeps()).build().unwrap();
    let pages = client.organizations().list().stream().pages();
    assert!(format!("{pages:?}").contains("Pages"));
    let pages: Vec<_> = pages.try_collect().await.unwrap();
    assert_eq!(pages.len(), 2);
    assert_eq!(pages[0].next_cursor.as_deref(), Some("c2"));

    let transport = FakeTransport::new(vec![Reply::json(
        200,
        json!({"items": [organization("o1")], "next_cursor": null}),
    )]);
    let client = builder(&transport, &sleeps()).build().unwrap();
    let mut stream = client.organizations().list().stream();
    stream.next().await.unwrap().unwrap();
    let rest: Vec<_> = stream.pages().collect().await;
    assert!(rest.is_empty(), "a polled stream has no pages left to give");
}

#[tokio::test]
async fn a_raw_call() {
    let transport = FakeTransport::new(vec![
        Reply::json(200, json!({"ok": true})).with_header("x-request-id", "r1"),
        Reply::json(201, json!({"id": "x"})),
        Reply::json(200, json!({"access_token": "a"})),
    ]);
    let client = builder(&transport, &sleeps()).build().unwrap();
    let response = client
        .request(HttpMethod::Get, "api/me/?a=1")
        .query("b", "2 3")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status, 200);
    assert_eq!(response.header("X-Request-Id"), Some("r1"));
    assert_eq!(
        response.json::<serde_json::Value>().unwrap(),
        json!({"ok": true})
    );

    let response = client
        .request(HttpMethod::Post, "/api/things/")
        .json(&json!({"name": "n"}))
        .unwrap()
        .idempotent()
        .accept("application/json")
        .header("x-a", "b")
        .timeout(Duration::from_secs(1))
        .max_retries(0)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status, 201);

    client
        .request(HttpMethod::Post, "/oauth/token")
        .without_credential()
        .body(
            "application/x-www-form-urlencoded",
            "grant_type=refresh_token",
        )
        .idempotency_key("k")
        .send()
        .await
        .unwrap();

    let requests = transport.requests();
    assert_eq!(
        requests[0].url,
        "https://api.example.com/api/me/?a=1&b=2%203"
    );
    assert_eq!(requests[1].header("idempotency-key"), Some("uuid-1"));
    assert_eq!(requests[1].header("content-type"), Some("application/json"));
    assert_eq!(requests[1].body.as_deref(), Some(&b"{\"name\":\"n\"}"[..]));
    assert_eq!(requests[2].header("authorization"), None);
    assert_eq!(
        requests[2].header("content-type"),
        Some("application/x-www-form-urlencoded")
    );
    assert_eq!(requests[2].header("idempotency-key"), Some("k"));
}

#[tokio::test]
async fn a_raw_call_with_an_error_keeps_the_body() {
    let body = br#"{"error": "authorization_pending"}"#.to_vec();
    let transport = FakeTransport::new(vec![Reply::Response(400, vec![], body)]);
    let client = builder(&transport, &sleeps()).build().unwrap();
    let error = client
        .request(HttpMethod::Post, "/oauth/token")
        .without_credential()
        .send()
        .await
        .unwrap_err();
    let api = error.api_error().unwrap();
    assert_eq!(api.code, "unknown");
    assert_eq!(api.body, r#"{"error": "authorization_pending"}"#);
}

#[test]
fn the_default_transport_and_the_environment() {
    // The process environment: this only checks that it builds without a panic.
    let _ = Client::from_env();
    let client = Client::builder()
        .env(|_| None)
        .api_key("osk_x")
        .tls_roots(TlsRoots::Bundled)
        .build()
        .unwrap();
    assert_eq!(client.base_url(), oscaraccountant::DEFAULT_BASE_URL);
    let _ = Client::builder().tls_roots(TlsRoots::Platform).build();
}
