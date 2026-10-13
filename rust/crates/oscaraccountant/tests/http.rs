//! The client over real HTTP (the default `reqwest` transport) against a wiremock server.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::TryStreamExt;
use oscaraccountant::{CancelInvoiceBodyRequest, Client, ClientBuilder, Error, Upload};
use serde_json::json;
use wiremock::matchers::{
    body_json, header, header_exists, method, path, query_param, query_param_is_missing,
};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

fn organization(id: &str) -> serde_json::Value {
    json!({"id": id, "name": "Org", "role": null, "permissions": [], "companies": [],
           "created_at": "2026-09-27T10:00:00Z", "updated_at": "2026-09-27T10:00:00Z"})
}

fn builder(server: &MockServer) -> ClientBuilder {
    Client::builder()
        .env(|_| None)
        .api_key("osk_test")
        .base_url(server.uri())
        .sleep(|_| Box::pin(async {}))
}

#[tokio::test]
async fn a_get_over_http() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/organizations/"))
        .and(query_param("limit", "10"))
        .and(header("authorization", "Bearer osk_test"))
        .and(header("accept", "application/json"))
        .and(header_exists("user-agent"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"items": [organization("o1")], "next_cursor": null})),
        )
        .expect(1)
        .mount(&server)
        .await;
    let page = builder(&server)
        .build()
        .unwrap()
        .organizations()
        .list()
        .limit(10)
        .send()
        .await
        .unwrap();
    assert_eq!(page.items.len(), 1);
}

#[tokio::test]
async fn a_json_post_with_its_key() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/companies/c1/invoices/i1/cancel"))
        .and(header("idempotency-key", "k1"))
        .and(header("content-type", "application/json"))
        .and(body_json(json!({"reason": "duplicate"})))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!({"command": "cancel_invoice", "command_id": "c", "commit_seq": 1, "replayed": true, "result": null}),
        ))
        .expect(1)
        .mount(&server)
        .await;
    let client = builder(&server).build().unwrap();
    let outcome = client
        .invoices()
        .cancel("c1", "i1", CancelInvoiceBodyRequest::new("duplicate"))
        .idempotency_key("k1")
        .send()
        .await
        .unwrap();
    assert!(outcome.replayed);
}

#[tokio::test]
async fn a_problem_and_a_retry() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/organizations/o1/"))
        .respond_with(
            ResponseTemplate::new(503)
                .insert_header("x-request-id", "req-1")
                .insert_header("retry-after", "0")
                .set_body_json(
                    json!({"code": "unavailable", "title": "Unavailable", "detail": "Try later.",
                                      "status": 503, "errors": []}),
                ),
        )
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/organizations/o1/"))
        .respond_with(ResponseTemplate::new(200).set_body_json(organization("o1")))
        .mount(&server)
        .await;
    let client = builder(&server).build().unwrap();
    assert_eq!(
        client
            .organizations()
            .retrieve("o1")
            .send()
            .await
            .unwrap()
            .id,
        "o1"
    );

    let error = client
        .organizations()
        .retrieve("o2")
        .send()
        .await
        .unwrap_err();
    assert!(matches!(error, Error::NotFound(_)));
    assert_eq!(error.code(), Some("unknown"));
    assert_eq!(server.received_requests().await.unwrap().len(), 3);
}

#[tokio::test]
async fn an_upload_and_a_download() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/companies/c1/files/"))
        .and(header_exists("idempotency-key"))
        .and(|request: &Request| {
            let content_type = request.headers.get("content-type").and_then(|v| v.to_str().ok()).unwrap_or("");
            let body = String::from_utf8_lossy(&request.body);
            content_type.starts_with("multipart/form-data; boundary=")
                && body.contains("filename=\"a.pdf\"")
                && body.contains("%PDF")
                && body.contains("name=\"kind\"\r\n\r\nreceipt")
        })
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "id": "f1", "kind": "receipt", "name": "a.pdf", "content_type": "application/pdf", "size_bytes": 4,
            "sha256": "x", "received_on": null, "core_document_id": null, "bank_transaction_ids": [],
            "uploaded_by": null, "created_at": "2026-09-27T10:00:00Z", "origin": null})))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/companies/c1/files/f1/content/"))
        .and(header("accept", "*/*"))
        .and(query_param_is_missing("format"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header(
                    "content-disposition",
                    "attachment; filename*=UTF-8''arve%20%C3%B6.pdf",
                )
                .set_body_raw(b"%PDF-1.7".to_vec(), "application/pdf"),
        )
        .mount(&server)
        .await;
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(directory.path().join("a.pdf"), b"%PDF").unwrap();
    let client = builder(&server).build().unwrap();
    let upload = Upload::from_path(directory.path().join("a.pdf"))
        .await
        .unwrap();
    let document = client
        .files()
        .upload("c1", upload)
        .kind("receipt")
        .send()
        .await
        .unwrap();
    assert_eq!(document.id, "f1");
    let download = client.files().download("c1", "f1").send().await.unwrap();
    assert_eq!(download.file_name(), Some("arve ö.pdf"));
    assert_eq!(download.content_type(), Some("application/pdf"));
    assert_eq!(download.content_length(), Some(8));
    let chunks: Vec<_> = download.try_collect().await.unwrap();
    assert_eq!(chunks.concat(), b"%PDF-1.7");
}

#[tokio::test]
async fn every_page_over_http() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/organizations/"))
        .and(query_param("cursor", "c2"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"items": [organization("o2")], "next_cursor": null})),
        )
        .with_priority(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/organizations/"))
        .and(query_param_is_missing("cursor"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"items": [organization("o1")], "next_cursor": "c2"})),
        )
        .mount(&server)
        .await;
    let items: Vec<_> = builder(&server)
        .build()
        .unwrap()
        .organizations()
        .list()
        .stream()
        .try_collect()
        .await
        .unwrap();
    assert_eq!(
        items
            .iter()
            .map(|item| item.id.as_str())
            .collect::<Vec<_>>(),
        vec!["o1", "o2"]
    );
}

#[tokio::test]
async fn a_slow_server_times_out() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(2)))
        .mount(&server)
        .await;
    let client = builder(&server)
        .timeout(Duration::from_millis(100))
        .max_retries(1)
        .build()
        .unwrap();
    let error = client.me().retrieve().send().await.unwrap_err();
    assert!(matches!(error, Error::Timeout(_)), "{error:?}");
    assert_eq!(server.received_requests().await.unwrap().len(), 2);
}

#[tokio::test]
async fn a_refused_connection() {
    // Port 1 (tcpmux) has no listener. A port taken from a bound and dropped
    // listener is free again at once, and a mock server of a parallel test
    // can take it before this request is sent.
    let port = 1;
    let sleeps = Arc::new(Mutex::new(Vec::new()));
    let recorded = sleeps.clone();
    let client = Client::builder()
        .env(|_| None)
        .api_key("osk_test")
        .base_url(format!("http://127.0.0.1:{port}"))
        .jitter(|| 0.0)
        .sleep(move |duration| {
            recorded.lock().unwrap().push(duration);
            Box::pin(async {})
        })
        .build()
        .unwrap();
    let error = client.me().retrieve().send().await.unwrap_err();
    assert!(matches!(error, Error::Connection(_)), "{error:?}");
    assert_eq!(
        *sleeps.lock().unwrap(),
        vec![Duration::from_millis(500), Duration::from_secs(1)]
    );
}
