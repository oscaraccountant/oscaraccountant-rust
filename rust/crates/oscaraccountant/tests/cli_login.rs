//! The CLI login through the client: a 401 asks the CLI for a new token once.

#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;

use oscaraccountant::{Client, Credential, Error, HttpMethod};
use serde_json::json;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// The tests that write and run a script hold this lock ("Text file busy").
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// A fake `oscar` that answers the token of the file `token`, for the base URL `base_url`.
fn fake_cli(base_url: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let script = format!(
        "#!/bin/sh\n\
         dir='{}'\n\
         echo \"$*\" >> \"$dir/calls\"\n\
         if [ \"$1\" = \"--version\" ]; then echo 'oscar-cli 0.1.0'; exit 0; fi\n\
         printf '{{\"token\": \"%s\", \"expires_at\": null, \"kind\": \"oauth\", \"base_url\": \"{base_url}\", \"profile\": \"default\"}}' \"$(cat \"$dir/token\")\"\n",
        dir.path().display()
    );
    std::fs::write(dir.path().join("oscar"), script).unwrap();
    std::fs::set_permissions(
        dir.path().join("oscar"),
        std::fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    std::fs::write(dir.path().join("token"), "tok-1").unwrap();
    dir
}

fn calls(dir: &tempfile::TempDir) -> usize {
    std::fs::read_to_string(dir.path().join("calls"))
        .unwrap_or_default()
        .lines()
        .count()
}

fn client(server: &MockServer, dir: &tempfile::TempDir) -> Client {
    let path = dir.path().display().to_string();
    Client::builder()
        .env(move |name| (name == "PATH").then(|| path.clone()))
        .credential(Credential::Cli)
        .base_url(server.uri())
        .build()
        .unwrap()
}

#[tokio::test]
async fn a_401_asks_the_cli_again_and_retries_a_get() {
    let _serial = SERIAL.lock().await;
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/me/"))
        .and(header("authorization", "Bearer tok-1"))
        .respond_with(
            ResponseTemplate::new(401)
                .set_body_json(json!({"code": "not_authenticated", "detail": "Expired."})),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/organizations/"))
        .and(header("authorization", "Bearer tok-2"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"items": [], "next_cursor": null})),
        )
        .mount(&server)
        .await;
    let dir = fake_cli(&server.uri());
    let client = client(&server, &dir);

    // The token of the CLI is refused, and the CLI still answers the same one: the 401 goes to the caller.
    let error = client.me().retrieve().send().await.unwrap_err();
    assert!(
        matches!(&error, Error::Authentication(auth) if auth.api.is_some()),
        "{error:?}"
    );
    assert_eq!(calls(&dir), 3, "--version, then the token twice");
    assert_eq!(server.received_requests().await.unwrap().len(), 2);

    // The refused token is not kept: the next call asks the CLI again, and its new token works.
    std::fs::write(dir.path().join("token"), "tok-2").unwrap();
    let page = client.organizations().list().send().await.unwrap();
    assert!(page.items.is_empty());
    assert_eq!(calls(&dir), 4);
}

#[tokio::test]
async fn a_401_on_a_post_without_a_key_is_not_sent_again() {
    let _serial = SERIAL.lock().await;
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(401)
                .set_body_json(json!({"code": "not_authenticated", "detail": "Expired."})),
        )
        .mount(&server)
        .await;
    let dir = fake_cli(&server.uri());
    let client = client(&server, &dir);
    let error = client
        .request(HttpMethod::Post, "/api/things/")
        .send()
        .await
        .unwrap_err();
    assert_eq!(error.status(), Some(401));
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
    // The cache was dropped: the next call asks the CLI again.
    client
        .request(HttpMethod::Post, "/api/things/")
        .send()
        .await
        .unwrap_err();
    assert_eq!(calls(&dir), 3, "--version once, then one token per call");
}
