//! The CLI suite: the real `oscar` binary against `clients/mock-server` over real HTTP.
//! Every login, the store, every command in table and JSON form, and the exit
//! codes.

mod support;

#[allow(dead_code, missing_docs, clippy::all)]
#[rustfmt::skip]
#[path = "../src/generated/commands.rs"]
mod commands;

#[allow(dead_code, missing_docs, clippy::all)]
#[rustfmt::skip]
#[path = "../src/generated/views.rs"]
mod views;

use serde_json::{Value, json};
use support::{Mock, User, cases_dir, test_browser};

fn defaults() -> Value {
    json!({"device_interval": 0})
}

#[tokio::test]
async fn prints_its_version() {
    let user = User::new();
    let run = user.run(&["--version"]).await.expect(0);
    assert_eq!(
        run.stdout,
        format!("oscar-cli {}\n", env!("CARGO_PKG_VERSION"))
    );
    let version = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../VERSION"),
    )
    .unwrap();
    assert_eq!(run.stdout.trim(), format!("oscar-cli {}", version.trim()));
}

#[tokio::test]
async fn api_key_login_whoami_token_and_logout() {
    let mock = Mock::start(defaults()).await;
    let user = User::new();
    let run = user.login_with_key(&mock).await.expect(0);
    assert!(
        run.stderr
            .contains("✓ Logged in as mock@example.com · company Mock Company OÜ"),
        "{}",
        run.stderr
    );
    assert!(run.stderr.contains("(OSCAR_CREDENTIAL_STORE=file)."));
    let config = user.config();
    let profile = &config["profiles"]["default"];
    assert_eq!(config["default_profile"], "default");
    assert_eq!(profile["auth"], "api_key");
    assert_eq!(profile["base_url"], mock.url);
    // The companies of the key, and no organization any more.
    assert!(profile.get("organizations").is_none(), "{profile}");
    assert_eq!(profile["companies"][0]["id"], mock.company_id);
    assert!(profile["companies"][0].get("organization_id").is_none());
    assert_eq!(profile["company"], mock.company_id);
    assert!(
        !config.to_string().contains(&mock.api_key),
        "no secret in config.json"
    );
    assert_eq!(
        user.credentials()["profiles"]["default"],
        json!({"api_key": mock.api_key})
    );

    let me = user.run(&["whoami", "--json"]).await.expect(0).json();
    assert_eq!(me["credential"]["kind"], "api_key");
    let table = user.run(&["whoami"]).await.expect(0);
    assert_eq!(
        table.stdout.trim_start().chars().next(),
        Some('{'),
        "not a terminal: JSON"
    );

    let token = user
        .run(&["auth", "token", "--json", "--base-url", &mock.url])
        .await
        .expect(0)
        .json();
    assert_eq!(
        token,
        json!({"token": mock.api_key, "expires_at": null, "kind": "api_key", "base_url": mock.url, "profile": "default"})
    );
    let plain = user.run(&["auth", "token"]).await.expect(0);
    assert_eq!(plain.stdout, format!("{}\n", mock.api_key));
    // Another base URL: no profile answers, so no key leaves for another host.
    let other = user
        .run(&[
            "auth",
            "token",
            "--json",
            "--base-url",
            "http://127.0.0.1:9",
        ])
        .await
        .expect(3);
    assert!(other.stdout.is_empty());
    let other = user
        .run(&[
            "auth",
            "token",
            "--profile",
            "default",
            "--base-url",
            "http://localhost:9",
        ])
        .await
        .expect(3);
    assert!(other.stderr.contains("is for"), "{}", other.stderr);

    let status = user
        .run(&["auth", "status", "--json"])
        .await
        .expect(0)
        .json();
    assert_eq!(status["auth"], "api_key");
    assert_eq!(status["expires_at"], Value::Null);
    assert_eq!(
        status["companies"],
        json!([{"id": mock.company_id, "name": "Mock Company OÜ"}])
    );
    assert!(status.get("organizations").is_none(), "{status}");
    assert!(status.get("organization").is_none(), "{status}");
    assert!(
        status["store"]
            .as_str()
            .unwrap()
            .ends_with("credentials.json")
    );

    let run = user.run(&["logout"]).await.expect(0);
    assert!(run.stderr.contains("stays valid at the server"));
    assert_eq!(user.config()["profiles"], json!({}));
    assert_eq!(user.credentials()["profiles"], json!({}));
    user.run(&["whoami"]).await.expect(3);
    user.run(&["auth", "status"]).await.expect(3);
    user.run(&["logout"]).await.expect(3);
}

#[tokio::test]
async fn the_api_key_flag_and_variable_are_never_stored() {
    let mock = Mock::start(defaults()).await;
    let mut user = User::new();
    let me = user
        .run(&[
            "whoami",
            "--json",
            "--base-url",
            &mock.url,
            "--api-key",
            &mock.api_key,
        ])
        .await
        .expect(0)
        .json();
    assert_eq!(me["credential"]["kind"], "api_key");
    user.set("OSCAR_API_KEY", &mock.api_key)
        .set("OSCAR_BASE_URL", &mock.url);
    user.run(&["whoami", "--json"]).await.expect(0);
    assert_eq!(user.config(), Value::Null);
    assert_eq!(user.credentials(), Value::Null);
    // A wrong key: 401, exit code 3.
    let run = user
        .run(&["whoami", "--api-key", "osk_wrong"])
        .await
        .expect(3);
    assert!(run.stderr.contains("oscar login"), "{}", run.stderr);
}

#[tokio::test]
async fn api_key_login_refuses_a_bad_key() {
    let mock = Mock::start(defaults()).await;
    let user = User::new();
    user.run_with(
        &["login", "--api-key", "--base-url", &mock.url],
        Some("osk_wrong\n"),
    )
    .await
    .expect(3);
    user.run_with(&["login", "--api-key", "--base-url", &mock.url], Some("\n"))
        .await
        .expect(2);
    assert_eq!(user.config(), Value::Null);
}

#[tokio::test]
async fn loopback_login() {
    let mock = Mock::start(defaults()).await;
    let mut user = User::new();
    let browser = test_browser(user.dir.path());
    user.set("BROWSER", &browser);
    let run = user
        .run(&["login", "--base-url", &mock.url])
        .await
        .expect(0);
    assert!(run.stderr.contains("✓ Logged in as"), "{}", run.stderr);
    assert_eq!(user.config()["profiles"]["default"]["auth"], "oauth");
    // The CLI checked that the web app has the page, and the browser landed on it.
    let pages = mock.wait_for_requests("/cli/done", 2).await;
    assert_eq!(pages.len(), 2, "{pages:#?}");
    assert!(pages.iter().all(|page| page["status"] == 200));
    assert!(
        pages[1]["headers"]["user-agent"]
            .as_str()
            .is_some_and(|agent| agent.starts_with("Python-urllib")),
        "the browser followed the redirect: {pages:#?}"
    );

    let authorize = &mock.requests_to("/oauth/authorize").await[0];
    let query: std::collections::HashMap<String, String> = authorize["query"]
        .as_array()
        .unwrap()
        .iter()
        .map(|pair| {
            (
                pair[0].as_str().unwrap().to_owned(),
                pair[1].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    assert_eq!(query["client_id"], "oscar-cli");
    assert_eq!(query["prompt"], "consent");
    assert_eq!(query["code_challenge_method"], "S256");
    assert_eq!(query["resource"], format!("{}/api", mock.url));
    assert_eq!(query["scope"], "read write");
    assert_eq!(query["response_type"], "code");
    assert_eq!(query["state"].len(), 43);
    let redirect = &query["redirect_uri"];
    assert!(redirect.starts_with("http://127.0.0.1:") && redirect.ends_with("/callback"));

    let token = &mock.requests_to("/oauth/token").await[0];
    let form: Vec<(String, String)> = token["body"]["value"]
        .as_array()
        .unwrap()
        .iter()
        .map(|pair| {
            (
                pair[0].as_str().unwrap().to_owned(),
                pair[1].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    assert!(form.contains(&("grant_type".into(), "authorization_code".into())));
    assert!(form.contains(&("redirect_uri".into(), redirect.clone())));
    assert!(form.contains(&("client_id".into(), "oscar-cli".into())));

    let me = user.run(&["whoami", "--json"]).await.expect(0).json();
    assert_eq!(me["credential"]["kind"], "oauth");
    let token = user
        .run(&["auth", "token", "--json", "--base-url", &mock.url])
        .await
        .expect(0)
        .json();
    assert_eq!(token["kind"], "oauth");
    assert!(token["expires_at"].as_str().unwrap().ends_with('Z'));
    let requests = mock.requests().await;
    assert!(
        requests.iter().all(|r| r["errors"] == json!([])),
        "{requests:#?}"
    );

    // A read-only login.
    user.run(&["login", "--scope", "read"]).await.expect(0);
    let authorize = mock.requests_to("/oauth/authorize").await;
    let last = authorize.last().unwrap()["query"].to_string();
    assert!(last.contains(r#"["scope","read"]"#), "{last}");
}

#[tokio::test]
async fn loopback_login_errors() {
    let mock = Mock::start(json!({"authorize_decision": "deny"})).await;
    let mut user = User::new();
    let browser = test_browser(user.dir.path());
    user.set("BROWSER", &browser);
    let run = user
        .run(&["login", "--base-url", &mock.url])
        .await
        .expect(3);
    assert!(run.stderr.contains("access_denied"), "{}", run.stderr);
    let error = mock.wait_for_requests("/cli/error", 1).await;
    assert_eq!(error.len(), 1, "the browser landed on the error page");
    assert_eq!(error[0]["query"], json!([["reason", "access_denied"]]));
    // The browser line names the URL too.
    assert!(
        run.stderr.contains("If it does not open, open this URL"),
        "{}",
        run.stderr
    );
    // An account with no company yet: the reason of the denial, and the way out.
    mock.control(
        "POST",
        "/_mock/config",
        Some(json!({"authorize_decision": "no_organization"})),
    )
    .await;
    let run = user
        .run(&["login", "--base-url", &mock.url])
        .await
        .expect(3);
    assert!(
        run.stderr.contains("Your Oscar account has no company yet"),
        "{}",
        run.stderr
    );
    assert!(
        run.stderr.contains(&format!("{}/onboarding", mock.url)),
        "{}",
        run.stderr
    );
    let error = mock.wait_for_requests("/cli/error", 2).await;
    assert_eq!(error.len(), 2, "{error:?}");
    assert_eq!(error[1]["query"], json!([["reason", "no_organization"]]));
    // A usage error, a wrong base URL and a closed port end the login before any wait.
    user.run(&["login", "--scope", "admin", "--base-url", &mock.url])
        .await
        .expect(2);
    user.run(&["login", "--base-url", "ftp://x.test"])
        .await
        .expect(2);
    user.run(&["login", "--base-url", "http://127.0.0.1:9"])
        .await
        .expect(4);
}

#[tokio::test]
async fn device_login_refresh_and_logout() {
    let mock = Mock::start(json!({"device_interval": 0, "access_token_ttl": 30})).await;
    let user = User::new();
    let run = user.login_with_device(&mock).await.expect(0);
    assert!(
        run.stderr
            .contains("Check that the browser shows the same code"),
        "{}",
        run.stderr
    );
    assert!(run.stderr.contains("/oauth-device"));
    let first = user.credentials()["profiles"]["default"].clone();
    assert!(first["refresh_token"].is_string());

    // The token lives 30 seconds, less than the margin of 60: each call refreshes first, and
    // the rotated refresh token is stored at once (a reused one would end the login).
    user.run(&["whoami", "--json"]).await.expect(0);
    let second = user.credentials()["profiles"]["default"].clone();
    assert_ne!(first["refresh_token"], second["refresh_token"]);
    user.run(&["invoices", "list", "--json"]).await.expect(0);
    let third = user.credentials()["profiles"]["default"].clone();
    assert_ne!(second["refresh_token"], third["refresh_token"]);
    let grants: Vec<String> = mock
        .requests_to("/oauth/token")
        .await
        .iter()
        .map(|request| request["body"]["value"][0][1].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(
        grants,
        [
            "urn:ietf:params:oauth:grant-type:device_code",
            "refresh_token",
            "refresh_token"
        ]
    );
    // `auth token` refreshes too, and answers the fresh token.
    let token = user
        .run(&["auth", "token", "--json"])
        .await
        .expect(0)
        .json();
    assert_eq!(
        token["token"],
        user.credentials()["profiles"]["default"]["access_token"]
    );

    user.run(&["logout"]).await.expect(0);
    let revoke = mock.requests_to("/oauth/revoke").await;
    assert_eq!(revoke.len(), 1);
    let tokens = mock.control("GET", "/_mock/tokens", None).await;
    assert!(
        tokens["refresh_tokens"]
            .as_array()
            .unwrap()
            .iter()
            .all(|token| token["status"] != "active"),
        "{tokens}"
    );
    assert_eq!(user.credentials()["profiles"], json!({}));
    assert_eq!(user.config()["profiles"], json!({}));
}

#[tokio::test]
async fn a_dead_refresh_token_means_log_in_again() {
    let mock = Mock::start(json!({"device_interval": 0})).await;
    let user = User::new();
    user.login_with_device(&mock).await.expect(0);
    let path = user.dir.path().join("credentials.json");
    let mut credentials = user.credentials();
    credentials["profiles"]["default"]["refresh_token"] = json!("dead");
    credentials["profiles"]["default"]["expires_at"] = json!("2000-01-01T00:00:00Z");
    std::fs::write(&path, credentials.to_string()).unwrap();
    let run = user.run(&["whoami"]).await.expect(3);
    assert!(run.stderr.contains("oscar login"), "{}", run.stderr);
    // A revoked access token: 401, exit code 3.
    user.login_with_device(&mock).await.expect(0);
    mock.control("POST", "/_mock/tokens/expire", Some(json!({})))
        .await;
    user.run(&["invoices", "list"]).await.expect(3);
}

#[tokio::test]
async fn device_login_slow_down_and_denial() {
    let mock = Mock::start(
        json!({"device_interval": 0, "device_auto_after_polls": 2, "device_slow_down_polls": 1}),
    )
    .await;
    let user = User::new();
    let started = std::time::Instant::now();
    user.run(&["login", "--device", "--base-url", &mock.url])
        .await
        .expect(0);
    assert!(started.elapsed().as_secs() >= 5, "slow_down adds 5 seconds");

    mock.control(
        "POST",
        "/_mock/config",
        Some(json!({"device_auto_after_polls": 1, "device_auto_decision": "deny"})),
    )
    .await;
    let run = user.run(&["login", "--device"]).await.expect(3);
    assert!(run.stderr.contains("denied"), "{}", run.stderr);
    assert!(
        run.stderr.contains("If your account has no company yet"),
        "{}",
        run.stderr
    );
    mock.control(
        "POST",
        "/_mock/config",
        Some(json!({"device_expires_in": 0, "device_auto_after_polls": null})),
    )
    .await;
    let run = user.run(&["login", "--device"]).await.expect(3);
    assert!(run.stderr.contains("expired"), "{}", run.stderr);
}

#[tokio::test]
async fn profiles_and_config() {
    let mock = Mock::start(defaults()).await;
    let user = User::new();
    user.login_with_key(&mock).await.expect(0);
    user.run_with(
        &[
            "login",
            "--api-key",
            "--profile",
            "second",
            "--base-url",
            &mock.url,
        ],
        Some(&mock.api_key),
    )
    .await
    .expect(0);
    let list = user
        .run(&["profiles", "list", "--json"])
        .await
        .expect(0)
        .json();
    assert_eq!(list.as_array().unwrap().len(), 2);
    assert_eq!(list[0]["name"], "default");
    assert_eq!(list[0]["default"], true);
    assert_eq!(list[0]["companies"], json!(["Mock Company OÜ"]));
    assert!(list[0].get("organization").is_none(), "{list}");
    user.run(&["profiles", "use", "second"]).await.expect(0);
    assert_eq!(user.config()["default_profile"], "second");
    user.run(&["profiles", "use", "nobody"]).await.expect(2);
    let table = user.run(&["profiles", "list"]).await.expect(0);
    assert!(table.stdout.contains("second"));

    user.run(&["config", "set", "company", "cmp0000009"])
        .await
        .expect(0);
    let got = user
        .run(&["config", "get", "company", "--json"])
        .await
        .expect(0)
        .json();
    assert_eq!(got, json!({"company": "cmp0000009"}));
    assert_eq!(user.config()["profiles"]["second"]["company"], "cmp0000009");
    // A company the key does not reach: 404, exit code 1.
    user.run(&["invoices", "list"]).await.expect(1);
    user.run(&["invoices", "list", "--company", &mock.company_id])
        .await
        .expect(0);
    user.run(&["config", "get", "colour"]).await.expect(2);
    user.run(&["config", "get", "company", "--profile", "nobody"])
        .await
        .expect(3);
}

#[tokio::test]
async fn a_login_to_another_server_needs_a_profile() {
    let a = Mock::start(defaults()).await;
    let b = Mock::start(defaults()).await;
    let user = User::new();
    user.login_with_key(&a).await.expect(0);
    let before = (user.config(), user.credentials());
    let run = user
        .run_with(
            &["login", "--api-key", "--base-url", &b.url],
            Some(&b.api_key),
        )
        .await
        .expect(2);
    assert!(run.stderr.contains("--profile <name>"), "{}", run.stderr);
    assert!(run.stderr.contains(&a.url), "{}", run.stderr);
    assert_eq!((user.config(), user.credentials()), before);
    user.run_with(
        &["login", "--api-key", "--profile", "b", "--base-url", &b.url],
        Some(&b.api_key),
    )
    .await
    .expect(0);
    let config = user.config();
    assert_eq!(config["profiles"]["b"]["base_url"], b.url);
    assert_eq!(config["profiles"]["default"]["base_url"], a.url);
    assert_eq!(
        user.credentials()["profiles"]["default"],
        before.1["profiles"]["default"]
    );
    // A login to the server of a profile, with no profile given, uses that profile.
    user.run_with(
        &["login", "--api-key", "--base-url", &b.url],
        Some(&b.api_key),
    )
    .await
    .expect(0);
    assert_eq!(user.config()["profiles"].as_object().unwrap().len(), 2);
}

#[tokio::test]
async fn a_login_keeps_a_change_made_while_it_waited() {
    let mock = Mock::start(defaults()).await;
    let user = User::new();
    user.run_with(
        &[
            "login",
            "--api-key",
            "--profile",
            "other",
            "--base-url",
            &mock.url,
        ],
        Some(&mock.api_key),
    )
    .await
    .expect(0);
    let args = [
        "login",
        "--device",
        "--profile",
        "new",
        "--base-url",
        &mock.url,
    ];
    let login = user.run(&args);
    let change = async {
        // Wait for the pending device grant: the login read config.json already.
        loop {
            let grants = mock.control("GET", "/_mock/device", None).await;
            if !grants["device_grants"].as_array().unwrap().is_empty() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        let path = user.dir.path().join("config.json");
        let mut config = user.config();
        config["profiles"]["other"]["company"] = json!("cmp_changed");
        std::fs::write(&path, config.to_string()).unwrap();
        mock.control("POST", "/_mock/device/approve", Some(json!({})))
            .await;
    };
    let (run, ()) = tokio::join!(login, change);
    run.expect(0);
    let config = user.config();
    assert_eq!(config["profiles"]["other"]["company"], "cmp_changed");
    assert_eq!(config["profiles"]["new"]["auth"], "oauth");
    assert_eq!(config["default_profile"], "other");
}

#[tokio::test]
async fn the_exit_codes() {
    let mock = Mock::start(defaults()).await;
    let user = User::new();
    user.run(&["whoami"]).await.expect(3);
    user.login_with_key(&mock).await.expect(0);
    user.run(&["invoices", "list", "--limit", "many"])
        .await
        .expect(2);
    user.run(&["invoices", "nothing"]).await.expect(2);
    user.run(&["invoices", "retrieve", "nope"]).await.expect(1);
    let run = user
        .run(&["invoices", "list", "--limit", "500"])
        .await
        .expect(1);
    assert!(run.stderr.contains("validation_error"), "{}", run.stderr);
    user.run(&[
        "whoami",
        "--base-url",
        "http://127.0.0.1:9",
        "--api-key",
        "osk_x",
    ])
    .await
    .expect(4);
    let run = user.run(&["api", "GET", "/api/nothing/"]).await.expect(1);
    assert!(
        run.stdout.contains("not_found"),
        "the error body is on stdout: {}",
        run.stdout
    );
    user.run(&["api", "FETCH", "/api/me/"]).await.expect(2);
}

#[tokio::test]
async fn api_all_pages_and_retries() {
    let mock = Mock::start(defaults()).await;
    let user = User::new();
    user.login_with_key(&mock).await.expect(0);
    let me = user.run(&["api", "get", "/api/me/"]).await.expect(0).json();
    assert_eq!(me["credential"]["kind"], "api_key");

    let one = user
        .run(&["invoices", "list", "--limit", "1", "--json"])
        .await
        .expect(0)
        .json();
    assert_eq!(one["items"].as_array().unwrap().len(), 1);
    assert!(one["next_cursor"].is_string());
    let all = user
        .run(&["invoices", "list", "--limit", "1", "--all", "--json"])
        .await
        .expect(0)
        .json();
    let full = user
        .run(&["invoices", "list", "--limit", "200", "--json"])
        .await
        .expect(0)
        .json();
    assert_eq!(all["items"], full["items"]);
    assert!(all["items"].as_array().unwrap().len() > 1);
    assert_eq!(all["next_cursor"], Value::Null);
    let pages = mock
        .requests_to(&format!("/api/companies/{}/invoices/", mock.company_id))
        .await;
    assert!(
        pages
            .iter()
            .any(|page| page["query"].to_string().contains("cursor"))
    );

    // A 503 then a success: the GET is retried, with the same answer.
    mock.control(
        "POST",
        "/_mock/responses",
        Some(json!({"responses": [{"match": {"path": "/api/me/"}, "response": {"status": 503, "headers": {"retry-after": "0"}, "body": {"kind": "json", "value": {"code": "busy"}}}}]})),
    )
    .await;
    user.run(&["whoami", "--json"]).await.expect(0);
    // A command keeps its Idempotency-Key on the retry.
    mock.control(
        "POST",
        "/_mock/responses",
        Some(json!({"responses": [{"match": {"operation_id": "commands_create_party"}, "fault": "close"}]})),
    )
    .await;
    user.run(&[
        "parties",
        "create",
        "--data",
        r#"{"name": "Retry OÜ", "kind": "supplier"}"#,
    ])
    .await
    .expect(0);
    let creates = mock
        .requests_to(&format!("/api/companies/{}/parties", mock.company_id))
        .await;
    assert_eq!(creates.len(), 2);
    assert_eq!(
        creates[0]["headers"]["idempotency-key"],
        creates[1]["headers"]["idempotency-key"]
    );
}

#[tokio::test]
async fn files_upload_and_download() {
    let mock = Mock::start(defaults()).await;
    let user = User::new();
    user.login_with_key(&mock).await.expect(0);
    let upload = user.dir.path().join("receipt.pdf");
    std::fs::write(&upload, b"%PDF-1.4\n% receipt\n").unwrap();
    let stored = user
        .run(&[
            "files",
            "upload",
            upload.to_str().unwrap(),
            "--kind",
            "receipt",
            "--received-on",
            "2026-09-20",
            "--json",
        ])
        .await
        .expect(0)
        .json();
    assert_eq!(stored["name"], "receipt.pdf");
    let id = stored["id"].as_str().unwrap().to_owned();
    user.run(&["files", "upload", "/no/such/file.pdf"])
        .await
        .expect(2);
    user.run(&[
        "files",
        "upload",
        upload.to_str().unwrap(),
        "--received-on",
        "20.09.2026",
    ])
    .await
    .expect(2);

    let saved = user.dir.path().join("copy.pdf");
    let run = user
        .run(&[
            "files",
            "download",
            &id,
            "-o",
            saved.to_str().unwrap(),
            "--json",
        ])
        .await
        .expect(0);
    assert_eq!(std::fs::read(&saved).unwrap(), b"%PDF-1.4\n% receipt\n");
    assert_eq!(run.json()["size"], 19);
    user.run(&["files", "download", &id, "-o", saved.to_str().unwrap()])
        .await
        .expect(2);
    user.run(&[
        "files",
        "download",
        &id,
        "-o",
        saved.to_str().unwrap(),
        "--force",
    ])
    .await
    .expect(0);
    let stdout = user
        .run(&["files", "download", &id, "-o", "-"])
        .await
        .expect(0);
    assert_eq!(stdout.stdout, "%PDF-1.4\n% receipt\n");
    // No -o: the name of the server, in the current directory.
    user.run(&["files", "download", "fil0000001"])
        .await
        .expect(0);
    assert!(user.dir.path().join("invoice.pdf").is_file());
    user.run(&["files", "download", "fil0000001"])
        .await
        .expect(2);
}

/// The arguments of one conformance case of `cases/operations.json`, as a command line.
fn command_line(call: &Value, workdir: &std::path::Path) -> Vec<String> {
    let name = call["operation"].as_str().unwrap();
    let (resource, method) = name.split_once('.').unwrap();
    let (resource, method) = (resource.replace('_', "-"), method.replace('_', "-"));
    let command = commands::COMMANDS
        .iter()
        .find(|command| command.resource == resource && command.method == method)
        .unwrap_or_else(|| panic!("{name} is in the table"));
    // The CLI name (`get`, `list`, none for the resource itself); `method` is the SDK name.
    let mut args = vec![resource.clone()];
    if !command.name.is_empty() {
        args.push(command.name.to_owned());
    }
    if method == "upload" {
        let file = &call["multipart"]["file"];
        let path = workdir.join(file["filename"].as_str().unwrap());
        let bytes = base64_decode(file["content_base64"].as_str().unwrap());
        std::fs::write(&path, bytes).unwrap();
        args.push(path.to_str().unwrap().to_owned());
        for (field, value) in call["multipart"]["fields"].as_object().unwrap() {
            args.push(format!("--{}", field.replace('_', "-")));
            args.push(value.as_str().unwrap().to_owned());
        }
    }
    for arg in command.args {
        let value = call["path"][arg.name].as_str().unwrap().to_owned();
        match arg.name {
            "company_id" => args.extend(["--company".to_owned(), value]),
            _ => args.push(value),
        }
    }
    if let Some(query) = call["query"].as_object() {
        for (name, value) in query {
            let flag = command.flags.iter().find(|flag| flag.name == name).unwrap();
            args.push(format!("--{}", flag.long));
            args.push(match value {
                Value::String(text) => text.clone(),
                other => other.to_string(),
            });
        }
    }
    if let Some(body) = call.get("body") {
        args.push("--data".to_owned());
        args.push(body.to_string());
    }
    if method == "download" {
        args.extend(["-o".to_owned(), "-".to_owned()]);
    }
    args
}

fn base64_decode(text: &str) -> Vec<u8> {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut bits = 0_u32;
    let mut count = 0;
    let mut out = Vec::new();
    for byte in text.bytes().filter(|byte| *byte != b'=') {
        let value = ALPHABET.iter().position(|a| *a == byte).unwrap() as u32;
        bits = (bits << 6) | value;
        count += 6;
        if count >= 8 {
            count -= 8;
            out.push((bits >> count) as u8);
        }
    }
    out
}

#[tokio::test]
async fn every_command_in_json_and_table_form() {
    let mock = Mock::start(defaults()).await;
    let mut user = User::new();
    user.login_with_key(&mock).await.expect(0);
    let cases: Value = serde_json::from_str(
        &std::fs::read_to_string(cases_dir().join("operations.json")).unwrap(),
    )
    .unwrap();
    let mut seen = std::collections::HashSet::new();
    for case in cases["cases"].as_array().unwrap() {
        let call = &case["call"];
        let Some(name) = call["operation"].as_str() else {
            continue;
        };
        if !seen.insert(name.to_owned()) {
            continue;
        }
        let args = command_line(call, user.dir.path());
        // A command with no content (`files delete`, 204) says "Done." on stderr, also with --json.
        let empty = case["expect"]["result"]["kind"] == "none";
        let mut json_args: Vec<&str> = args.iter().map(String::as_str).collect();
        json_args.push("--json");
        // A command may run once only (an upload, a command with a key): each run gets a new
        // Idempotency-Key, so a second run is a new call.
        let run = user.run(&json_args).await;
        assert_eq!(run.code, 0, "{name}: {args:?}: {}", run.stderr);
        if empty {
            assert!(run.stdout.is_empty(), "{name}: {}", run.stdout);
            assert_eq!(run.stderr.trim(), "Done.", "{name}");
        } else if !name.ends_with("download") {
            run.json();
        }
        user.set("OSCAR_FORCE_TTY", "1");
        let table: Vec<&str> = args.iter().map(String::as_str).collect();
        let run = user.run(&table).await;
        assert_eq!(run.code, 0, "{name} as a table: {}", run.stderr);
        if !name.ends_with("download") {
            assert!(
                serde_json::from_str::<Value>(&run.stdout).is_err(),
                "{name} prints a table"
            );
            // A write prints its success line, an empty list its empty state, on stderr.
            assert!(
                !run.stdout.trim().is_empty() || !run.stderr.trim().is_empty(),
                "{name} prints something"
            );
        }
        user.vars.retain(|(key, _)| key != "OSCAR_FORCE_TTY");
    }
    assert_eq!(
        seen.len(),
        commands::COMMANDS.len(),
        "every operation has a case"
    );
    let requests = mock.requests().await;
    let invalid: Vec<&Value> = requests
        .iter()
        .filter(|r| r["errors"] != json!([]))
        .collect();
    assert!(invalid.is_empty(), "{invalid:#?}");
}

#[tokio::test]
async fn completion_and_help() {
    let user = User::new();
    for shell in ["bash", "zsh", "fish", "powershell", "elvish"] {
        let run = user.run(&["completion", shell]).await.expect(0);
        assert!(run.stdout.contains("oscar"));
    }
    user.run(&["completion", "tcsh"]).await.expect(2);
    let help = user.run(&["invoices", "list", "--help"]).await.expect(0);
    assert!(help.stdout.contains("--all") && help.stdout.contains("--company"));
    assert!(help.stdout.contains("--columns") && help.stdout.contains("--jq"));
    let bash = user.run(&["completion", "bash"]).await.expect(0);
    assert!(bash.stdout.contains("trial-balance") && bash.stdout.contains("get"));
    let help = user.run(&["invoices", "--help"]).await.expect(0);
    assert!(help.stdout.contains("get") && !help.stdout.contains("retrieve"));
    user.run(&[]).await.expect(2);
}

#[test]
fn a_closed_pipe_is_not_a_panic() {
    // Regression: `oscar completion bash | head -3` panicked with "Broken pipe".
    let user = User::new();
    for args in [&["completion", "bash"][..], &["completion", "zsh"][..]] {
        let mut child = std::process::Command::new(User::binary())
            .args(args)
            .env("OSCAR_CONFIG_DIR", user.dir.path())
            .env("BROWSER", "true")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .expect("the oscar binary runs");
        // The reader goes away before the script is written: every write fails.
        drop(child.stdout.take());
        let output = child.wait_with_output().expect("oscar ran");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(!stderr.contains("panicked"), "{args:?}: {stderr}");
        assert_eq!(output.status.code(), Some(0), "{args:?}: {stderr}");
    }
}

#[tokio::test]
async fn loopback_login_with_no_page_of_the_web_app() {
    // An older server has no `/cli/done`: the CLI answers the browser with its own page.
    let mock = Mock::start(json!({"cli_pages": false})).await;
    let mut user = User::new();
    let browser = test_browser(user.dir.path());
    user.set("BROWSER", &browser);
    user.run(&["login", "--base-url", &mock.url])
        .await
        .expect(0);
    let pages = mock.requests_to("/cli/done").await;
    assert_eq!(pages.len(), 1, "only the check: {pages:#?}");
    assert_eq!(pages[0]["status"], 404);
}

/// `GET /api/me/` with these companies, for every later call.
async fn script_companies(mock: &Mock, companies: Value) {
    mock.control("DELETE", "/_mock/responses", None).await;
    let me = json!({
        "user": {"id": "u1", "email": "mock@example.com", "name": "Mock User", "is_staff": false, "ui_mode": "simple"},
        "credential": {"kind": "api_key", "scopes": ["read", "write"], "staff_key": false},
        "companies": companies,
        "cli_min_version": "0.1.0",
        "email_verified": true,
        "has_totp": false,
        "staff_mfa_required": false,
        "features": {"agent": false, "inbox": false, "employees": false},
    });
    mock.control(
        "POST",
        "/_mock/responses",
        Some(json!({"responses": [{"match": {"path": "/api/me/"}, "times": 1000, "response": {"status": 200, "body": {"kind": "json", "value": me}}}]})),
    )
    .await;
}

fn company(id: &str, name: &str, code: &str) -> Value {
    json!({"id": id, "name": name, "registry_code": code,
           "vat_number": "", "legal_form": "ou", "state": "ready"})
}

#[tokio::test]
async fn several_companies_and_the_company_flag() {
    let mock = Mock::start(defaults()).await;
    script_companies(
        &mock,
        json!([
            company(&mock.company_id, "Mock Company OÜ", "12345678"),
            company("cmp0000002", "Alpha OÜ", "22222222"),
            company("cmp0000003", "Alpha Holding OÜ", "33333333"),
        ]),
    )
    .await;
    let mut user = User::new();
    // No terminal: the picker never shows, no company is chosen, and the CLI says how to choose.
    let run = user.login_with_key(&mock).await.expect(0);
    assert!(
        run.stderr.contains("You have 3 companies"),
        "{}",
        run.stderr
    );
    assert!(run.stderr.contains("· no company"), "{}", run.stderr);
    let profile = &user.config()["profiles"]["default"];
    assert_eq!(profile["company"], Value::Null);
    assert_eq!(profile["companies"].as_array().unwrap().len(), 3);
    let run = user.run(&["invoices", "list"]).await.expect(2);
    assert!(run.stderr.contains("-c/--company"), "{}", run.stderr);

    // `-c` takes a name prefix, a registry code or an id, from the cache: no call of /api/me/.
    mock.control("DELETE", "/_mock/requests", None).await;
    user.run(&["invoices", "list", "-c", "mock", "--json"])
        .await
        .expect(0);
    user.run(&["invoices", "list", "-c", "12345678", "--json"])
        .await
        .expect(0);
    user.set("OSCAR_COMPANY", "MOCK COMP");
    user.run(&["invoices", "list", "--json"]).await.expect(0);
    user.vars.retain(|(name, _)| name != "OSCAR_COMPANY");
    assert!(mock.requests_to("/api/me/").await.is_empty());
    let path = format!("/api/companies/{}/invoices/", mock.company_id);
    assert_eq!(mock.requests_to(&path).await.len(), 3);
    let run = user
        .run(&["invoices", "list", "-c", "alpha h", "--json"])
        .await;
    assert_eq!(
        mock.requests_to("/api/companies/cmp0000003/invoices/")
            .await
            .len(),
        1,
        "{}",
        run.stderr
    );

    // Ambiguous: a usage error that lists the matches.
    let run = user
        .run(&["invoices", "list", "-c", "alpha"])
        .await
        .expect(2);
    assert!(
        run.stderr.contains("`alpha` matches 2 companies"),
        "{}",
        run.stderr
    );
    assert!(
        run.stderr
            .contains("Alpha Holding OÜ (registry code 33333333")
    );

    // A name the cache does not know: /api/me/ is read again and the cache stored.
    script_companies(
        &mock,
        json!([
            company(&mock.company_id, "Mock Company OÜ", "12345678"),
            company("cmp0000004", "Delta AS", "44444444"),
        ]),
    )
    .await;
    user.run(&["invoices", "list", "-c", "delta", "--json"])
        .await;
    assert_eq!(
        mock.requests_to("/api/companies/cmp0000004/invoices/")
            .await
            .len(),
        1
    );
    let cache = &user.config()["profiles"]["default"]["companies"];
    assert_eq!(cache.as_array().unwrap().len(), 2);
    let run = user
        .run(&["invoices", "list", "-c", "zeta"])
        .await
        .expect(2);
    assert!(
        run.stderr.contains("No company matches `zeta`"),
        "{}",
        run.stderr
    );

    // `oscar companies use`, and a login with -c.
    let run = user.run(&["companies", "use", "mock"]).await.expect(0);
    assert!(
        run.stderr
            .contains("✓ Default company: Mock Company OÜ (profile `default`)"),
        "{}",
        run.stderr
    );
    assert_eq!(
        user.config()["profiles"]["default"]["company"],
        mock.company_id
    );
    user.run(&["invoices", "list", "--json"]).await.expect(0);
    user.run(&["companies", "use", "zeta"]).await.expect(2);
    // The success line of a write names the company.
    user.set("OSCAR_FORCE_TTY", "1");
    let run = user
        .run(&[
            "parties",
            "create",
            "--data",
            r#"{"name": "Line OÜ", "kind": "supplier"}"#,
        ])
        .await
        .expect(0);
    user.vars.retain(|(name, _)| name != "OSCAR_FORCE_TTY");
    assert!(run.stderr.contains("✓ Created party"), "{}", run.stderr);
    assert!(
        run.stderr.contains(" · Mock Company OÜ · "),
        "{}",
        run.stderr
    );
    user.set("OSCAR_FORCE_TTY", "1");
    let me = user.run(&["whoami", "--no-input"]).await.expect(0);
    user.vars.retain(|(name, _)| name != "OSCAR_FORCE_TTY");
    assert!(
        me.stdout.contains("Mock Company OÜ (cmp0000001)"),
        "{}",
        me.stdout
    );
    let run = user
        .run_with(
            &["login", "--api-key", "-c", "delta", "--base-url", &mock.url],
            Some(&mock.api_key),
        )
        .await
        .expect(0);
    assert!(run.stderr.contains("company Delta AS"), "{}", run.stderr);
    assert_eq!(
        user.config()["profiles"]["default"]["company"],
        "cmp0000004"
    );
    // A login with a company the user cannot see stores nothing.
    user.run_with(
        &["login", "--api-key", "-c", "zeta", "--base-url", &mock.url],
        Some(&mock.api_key),
    )
    .await
    .expect(2);
    assert_eq!(
        user.config()["profiles"]["default"]["company"],
        "cmp0000004"
    );
}

#[tokio::test]
async fn no_company_offers_the_onboarding_and_no_input_never_asks() {
    let mock = Mock::start(defaults()).await;
    script_companies(&mock, json!([])).await;
    let mut user = User::new();
    let run = user.login_with_key(&mock).await.expect(0);
    assert!(run.stderr.contains("oscar onboard"), "{}", run.stderr);
    // The hint follows the line it explains.
    assert!(
        run.stderr.find("Logged in") < run.stderr.find("oscar onboard"),
        "{}",
        run.stderr
    );
    // A login with an API key and --no-input reads the key from standard input still.
    user.set("OSCAR_NO_INPUT", "1");
    user.run_with(
        &["login", "--api-key", "--no-input", "--base-url", &mock.url],
        Some(&mock.api_key),
    )
    .await
    .expect(0);
}

/// With no login at all, a company command says "Not logged in" (exit 3), not "No company"
/// (exit 2): the login is what is missing.
#[tokio::test]
async fn no_login_is_not_logged_in_before_no_company() {
    let mock = Mock::start(defaults()).await;
    let user = User::new();
    for args in [
        vec!["invoices", "list"],
        vec!["invoices", "list", "--profile", "nope"],
        vec!["invoices", "list", "--base-url", "http://127.0.0.1:1"],
    ] {
        let run = user.run(&args).await.expect(3);
        assert!(
            !run.stderr.contains("No company"),
            "{args:?}: {}",
            run.stderr
        );
    }
    let run = user.run(&["invoices", "list"]).await.expect(3);
    assert!(run.stderr.contains("Not logged in"), "{}", run.stderr);
    let run = user
        .run(&["invoices", "list", "--profile", "nope"])
        .await
        .expect(3);
    assert!(run.stderr.contains("No profile `nope`"), "{}", run.stderr);
    // A key with no company is the company that is missing.
    let run = user
        .run(&[
            "invoices",
            "list",
            "--base-url",
            &mock.url,
            "--api-key",
            "osk_x",
        ])
        .await
        .expect(2);
    assert!(run.stderr.contains("No company"), "{}", run.stderr);
}

#[tokio::test]
async fn a_row_is_found_by_what_the_list_shows_and_a_path_never_walks() {
    // Regression: `oscar invoices get EE-2026-007` and `oscar files get <name>` were 404.
    let mock = Mock::start(defaults()).await;
    let user = User::new();
    user.login_with_key(&mock).await.expect(0);
    let company = &mock.company_id;
    let invoices = user
        .run(&["invoices", "list", "--json"])
        .await
        .expect(0)
        .json();
    let mut item = invoices["items"][0].clone();
    item["number"] = json!("EE-2026-007");
    let list = format!("/api/companies/{company}/invoices/");
    mock.control(
        "POST",
        "/_mock/responses",
        Some(
            json!({"responses": [{"match": {"path": list}, "response": {"status": 200,
            "body": {"kind": "json", "value": {"items": [item], "next_cursor": null}}}}]}),
        ),
    )
    .await;
    let found = user
        .run(&["invoices", "get", "EE-2026-007", "--json"])
        .await
        .expect(0)
        .json();
    assert_eq!(found["id"], item["id"]);
    let lists = mock.requests_to(&list).await;
    assert!(
        lists
            .last()
            .unwrap()
            .to_string()
            .contains(r#"["number","EE-2026-007"]"#),
        "the list is filtered on the number: {lists:?}"
    );
    // In lower case: the exact filter of the server finds nothing, so the number is tried
    // in upper case.
    let empty = json!({"items": [], "next_cursor": null});
    mock.control(
        "POST",
        "/_mock/responses",
        Some(json!({"responses": [
            {"match": {"path": list}, "response": {"status": 200, "body": {"kind": "json", "value": empty}}},
            {"match": {"path": list}, "response": {"status": 200,
             "body": {"kind": "json", "value": {"items": [item], "next_cursor": null}}}},
        ]})),
    )
    .await;
    let found = user
        .run(&["invoices", "get", "ee-2026-007", "--json"])
        .await
        .expect(0)
        .json();
    assert_eq!(found["id"], item["id"]);
    let lists = mock.requests_to(&list).await;
    let last = lists.last().unwrap().to_string();
    assert!(last.contains(r#"["number","EE-2026-007"]"#), "{lists:?}");
    assert!(
        lists[lists.len() - 2]
            .to_string()
            .contains(r#"["number","ee-2026-007"]"#),
        "{lists:?}"
    );

    let file = user
        .run(&["files", "get", "invoice.pdf", "--json"])
        .await
        .expect(0)
        .json();
    assert_eq!(file["name"], "invoice.pdf");

    // Every invoice of the fixtures has the number `number`: the user is asked for more.
    let many = user.run(&["invoices", "get", "number"]).await.expect(2);
    assert!(many.stderr.contains("matches 3 items"), "{}", many.stderr);
    assert!(
        many.stderr.contains("01J90000000000000000000116  number"),
        "{}",
        many.stderr
    );

    // Regression: a value that would leave its segment is refused, and nothing is sent.
    let before = mock.requests().await.len();
    for bad in ["../../me", "a/b", ".."] {
        let run = user.run(&["invoices", "get", bad]).await.expect(2);
        assert!(
            run.stderr.contains("is not a valid INVOICE_ID"),
            "{}",
            run.stderr
        );
    }
    assert_eq!(mock.requests().await.len(), before);
}

#[tokio::test]
async fn an_error_page_is_summed_up_not_printed() {
    // Regression: an HTML 404 page of hundreds of lines after `error: 404 unknown:`.
    let mock = Mock::start(defaults()).await;
    let mut user = User::new();
    user.login_with_key(&mock).await.expect(0);
    let page = format!(
        "<!DOCTYPE html><html><head><title>Page not found at /api/x/</title></head><body>{}</body></html>",
        "<p>No route matches this path.</p>\n".repeat(300)
    );
    let script = json!({"responses": [{"match": {"path": "/api/me/"}, "times": 2, "response": {"status": 404,
        "headers": {"content-type": "text/html"}, "body": {"kind": "text", "value": page}}}]});
    mock.control("POST", "/_mock/responses", Some(script)).await;
    let run = user.run(&["whoami"]).await.expect(1);
    assert_eq!(
        run.stderr.lines().next().unwrap(),
        "error: 404 Not Found: the server answered an HTML page (\"Page not found at /api/x/\"), not JSON. OSCAR_DEBUG=1 prints the whole answer."
    );
    assert_eq!(run.stderr.lines().count(), 1, "{}", run.stderr);
    user.set("OSCAR_DEBUG", "1");
    let debug = user.run(&["whoami"]).await.expect(1);
    assert!(
        debug.stderr.contains("No route matches this path."),
        "{}",
        debug.stderr
    );
}

/// `GET /api/me/` of an OAuth login granted two companies on the consent page, for every later
/// call.
async fn script_two_companies(mock: &Mock, companies: Value) {
    mock.control("DELETE", "/_mock/responses", None).await;
    let me = json!({
        "user": {"id": "u1", "email": "mock@example.com", "name": "Mock User", "is_staff": false, "ui_mode": "simple"},
        "credential": {"kind": "oauth", "scopes": ["read", "write"], "staff_key": false},
        "companies": companies,
        "cli_min_version": "0.1.0",
        "email_verified": true,
        "has_totp": false,
        "staff_mfa_required": false,
        "features": {"agent": false, "inbox": false, "employees": false},
    });
    mock.control(
        "POST",
        "/_mock/responses",
        Some(json!({"responses": [{"match": {"path": "/api/me/"}, "times": 1000, "response": {"status": 200, "body": {"kind": "json", "value": me}}}]})),
    )
    .await;
}

/// A login of two companies: the profile stores both, the login line counts them, and both are
/// the choices of the default company. No command asks for an organization.
#[tokio::test]
async fn a_login_of_two_companies() {
    let mock = Mock::start(defaults()).await;
    script_two_companies(
        &mock,
        json!([
            company(&mock.company_id, "Mock Company OÜ", "12345678"),
            company("cmp0000002", "Second Company OÜ", "22222222"),
        ]),
    )
    .await;
    let user = User::new();
    let run = user.login_with_device(&mock).await.expect(0);
    assert!(
        run.stderr
            .contains("Logged in as mock@example.com · 2 companies · no company"),
        "{}",
        run.stderr
    );
    assert!(
        run.stderr.contains("You have 2 companies"),
        "{}",
        run.stderr
    );
    let profile = &user.config()["profiles"]["default"];
    assert_eq!(profile["auth"], "oauth");
    assert_eq!(profile["companies"].as_array().unwrap().len(), 2);
    assert!(profile.get("organizations").is_none(), "{profile}");
    assert!(profile.get("organization").is_none(), "{profile}");

    // The default company may be the second one.
    let run = user
        .run(&["login", "--device", "--base-url", &mock.url, "-c", "second"])
        .await
        .expect(0);
    assert!(
        run.stderr.contains("· company Second Company OÜ"),
        "{}",
        run.stderr
    );
    assert_eq!(
        user.config()["profiles"]["default"]["company"],
        "cmp0000002"
    );
    // Not a company of the grant.
    user.run(&["login", "--device", "--base-url", &mock.url, "-c", "third"])
        .await
        .expect(2);
    // A login again keeps the default company it still reaches.
    user.run(&["login", "--device", "--base-url", &mock.url])
        .await
        .expect(0);
    assert_eq!(
        user.config()["profiles"]["default"]["company"],
        "cmp0000002"
    );

    let list = user
        .run(&["profiles", "list", "--json"])
        .await
        .expect(0)
        .json();
    assert_eq!(
        list[0]["companies"],
        json!(["Mock Company OÜ", "Second Company OÜ"])
    );
    let status = user
        .run(&["auth", "status", "--json"])
        .await
        .expect(0)
        .json();
    assert_eq!(
        status["companies"],
        json!([
            {"id": mock.company_id, "name": "Mock Company OÜ"},
            {"id": "cmp0000002", "name": "Second Company OÜ"},
        ])
    );
    let whoami = user.run(&["whoami"]).await.expect(0).json();
    assert!(whoami.get("organizations").is_none(), "{whoami}");
    assert_eq!(whoami["companies"].as_array().unwrap().len(), 2);
    // The organization commands and the flag are gone.
    user.run(&["organizations", "list"]).await.expect(2);
    user.run(&["companies", "list", "--organization", "org0000001"])
        .await
        .expect(2);
}

/// The `config.json` of a CLI before 0.2.0, with the organizations of each profile and an
/// `organization_id` in each cached company: it loads, no command asks for an organization,
/// and the next write drops the old keys.
#[tokio::test]
async fn the_config_file_of_an_older_cli_still_loads() {
    let mock = Mock::start(defaults()).await;
    let user = User::new();
    user.login_with_key(&mock).await.expect(0);
    let mut config = user.config();
    let profile = config["profiles"]["default"].as_object_mut().unwrap();
    profile.insert(
        "organizations".into(),
        json!([{"id": "org0000001", "name": "Mock Organization OÜ"}]),
    );
    profile.insert(
        "organization".into(),
        json!({"id": "org0000001", "name": "Mock Organization OÜ"}),
    );
    profile["companies"][0]["organization_id"] = json!("org0000001");
    let path = user.dir.path().join("config.json");
    std::fs::write(&path, config.to_string()).unwrap();

    let list = user
        .run(&["profiles", "list", "--json"])
        .await
        .expect(0)
        .json();
    assert_eq!(list[0]["companies"], json!(["Mock Company OÜ"]));
    assert!(list[0].get("organizations").is_none(), "{list}");
    let status = user
        .run(&["auth", "status", "--json"])
        .await
        .expect(0)
        .json();
    assert_eq!(
        status["companies"],
        json!([{"id": mock.company_id, "name": "Mock Company OÜ"}])
    );
    assert!(status.get("organization").is_none(), "{status}");
    user.run(&["companies", "list", "--json"]).await.expect(0);
    assert_eq!(mock.requests_to("/api/companies/").await.len(), 1);
    user.run(&["invoices", "list", "--json"]).await.expect(0);

    // The next write of the file: no organization key, no `organization_id`.
    user.run(&["companies", "use", &mock.company_id])
        .await
        .expect(0);
    let profile = &user.config()["profiles"]["default"];
    assert!(profile.get("organizations").is_none(), "{profile}");
    assert!(profile.get("organization").is_none(), "{profile}");
    assert!(
        profile["companies"][0].get("organization_id").is_none(),
        "{profile}"
    );
    assert_eq!(profile["company"], mock.company_id);
}

/// A company as `GET /api/companies/` lists it.
fn listed_company(id: &str, name: &str, code: &str) -> Value {
    json!({"id": id, "name": name, "registry_code": code,
           "vat_number": "", "vat_status": "unregistered", "vat_status_from": null,
           "legal_form": "ou", "files_tsd_monthly": false, "state": "ready",
           "core_company_id": "01J90000000000000000000008", "base_currency": "EUR",
           "created_at": "2026-09-27T10:00:00Z", "updated_at": "2026-09-27T10:00:00Z"})
}

/// The pages of `GET /api/companies/`, for its next requests.
async fn script_company_pages(mock: &Mock, pages: &[(Value, Option<&str>)], times: u32) {
    let responses: Vec<Value> = pages
        .iter()
        .map(|(items, next)| {
            json!({"match": {"path": "/api/companies/"}, "times": times, "response": {"status": 200,
                   "body": {"kind": "json", "value": {"items": items, "next_cursor": next}}}})
        })
        .collect();
    mock.control(
        "POST",
        "/_mock/responses",
        Some(json!({ "responses": responses })),
    )
    .await;
}

/// The ids of the items of a list answer.
fn ids(list: &Value) -> Vec<&str> {
    list["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["id"].as_str().unwrap())
        .collect()
}

/// `oscar companies list` asks nothing: it calls `GET /api/companies/` once per page, with no
/// default company, no `--organization` and no read of `GET /api/me/` first.
#[tokio::test]
async fn companies_list_asks_nothing() {
    let mock = Mock::start(defaults()).await;
    script_two_companies(
        &mock,
        json!([
            company(&mock.company_id, "Mock Company OÜ", "12345678"),
            company("cmp0000002", "Second Company OÜ", "22222222"),
        ]),
    )
    .await;
    let mut user = User::new();
    user.login_with_device(&mock).await.expect(0);
    // Two companies and no terminal: no default company, and the list needs none.
    assert_eq!(user.config()["profiles"]["default"]["company"], Value::Null);
    let me = mock.requests_to("/api/me/").await.len();
    let second = listed_company("cmp0000002", "Second Company OÜ", "22222222");
    let third = listed_company("cmp0000003", "Third Company OÜ", "33333333");
    script_company_pages(&mock, &[(json!([second]), Some("page2"))], 1).await;
    let list = user
        .run(&["companies", "list", "--json"])
        .await
        .expect(0)
        .json();
    assert_eq!(ids(&list), ["cmp0000002"]);
    assert_eq!(list["next_cursor"], "page2");
    // The next page with its cursor.
    script_company_pages(&mock, &[(json!([third]), None)], 1).await;
    let next = user
        .run(&["companies", "list", "--cursor", "page2", "--json"])
        .await
        .expect(0)
        .json();
    assert_eq!(ids(&next), ["cmp0000003"]);
    let pages = mock.requests_to("/api/companies/").await;
    assert_eq!(pages.len(), 2, "{pages:#?}");
    assert_eq!(pages[0]["query"], json!([]));
    assert_eq!(pages[1]["query"], json!([["cursor", "page2"]]));
    assert_eq!(mock.requests_to("/api/me/").await.len(), me);
    assert!(mock.requests_to("/api/organizations/").await.is_empty());

    // The table.
    script_company_pages(&mock, &[(json!([second, third]), None)], 1000).await;
    user.set("OSCAR_FORCE_TTY", "1")
        .set("COLUMNS", "100")
        .set("FORCE_HYPERLINK", "0")
        .set("LC_ALL", "en_US.UTF-8");
    let table = user.run(&["companies", "list"]).await.expect(0);
    insta::assert_snapshot!("companies_list_of_the_login", table.stdout);
    user.vars.retain(|(key, _)| key != "OSCAR_FORCE_TTY");

    // A key of OSCAR_API_KEY and no profile: the same call.
    let mut keyed = User::new();
    keyed
        .set("OSCAR_API_KEY", &mock.api_key)
        .set("OSCAR_BASE_URL", &mock.url);
    mock.control("DELETE", "/_mock/responses", None).await;
    let list = keyed
        .run(&["companies", "list", "--json"])
        .await
        .expect(0)
        .json();
    assert_eq!(ids(&list), [mock.company_id.as_str()]);
    assert_eq!(keyed.config(), Value::Null);
}

/// A list with an `ordering` asks for the newest first, unless `--ordering` is given; a list
/// with no `ordering` sends none.
#[tokio::test]
async fn lists_ask_for_the_newest_first() {
    let mock = Mock::start(defaults()).await;
    let user = User::new();
    user.login_with_key(&mock).await.expect(0);
    let company = &mock.company_id;
    let last_query = |requests: Vec<Value>| requests.last().unwrap()["query"].clone();

    user.run(&["invoices", "list", "--json"]).await.expect(0);
    let invoices = format!("/api/companies/{company}/invoices/");
    let query = last_query(mock.requests_to(&invoices).await);
    assert_eq!(query, json!([["ordering", "-invoice_date"]]));

    user.run(&["invoices", "list", "--ordering", "invoice_date", "--json"])
        .await
        .expect(0);
    let query = last_query(mock.requests_to(&invoices).await);
    assert_eq!(query, json!([["ordering", "invoice_date"]]));

    user.run(&["documents", "list", "--json"]).await.expect(0);
    let documents = format!("/api/companies/{company}/documents/");
    let query = last_query(mock.requests_to(&documents).await);
    assert_eq!(query, json!([["ordering", "-created_at"]]));

    user.run(&["bank-transactions", "list", "--json"])
        .await
        .expect(0);
    let lines = format!("/api/companies/{company}/bank-transactions/");
    let query = last_query(mock.requests_to(&lines).await);
    assert_eq!(query, json!([["ordering", "-booking_date"]]));

    user.run(&["accounts", "list", "--json"]).await.expect(0);
    let accounts = format!("/api/companies/{company}/accounts/");
    assert_eq!(last_query(mock.requests_to(&accounts).await), json!([]));

    let help = user.run(&["invoices", "list", "--help"]).await.expect(0);
    assert!(
        help.stdout
            .contains("[default: -invoice_date, newest first]"),
        "{}",
        help.stdout
    );
}

/// `-c/--company` before the command, the command's `-c` over the root's, and `oscar companies
/// get <COMPANY>`.
#[tokio::test]
async fn the_company_before_the_command_and_as_the_argument_of_companies_get() {
    let mock = Mock::start(defaults()).await;
    script_companies(
        &mock,
        json!([
            company(&mock.company_id, "Mock Company OÜ", "12345678"),
            company("cmp0000002", "Alpha OÜ", "22222222"),
        ]),
    )
    .await;
    let mut user = User::new();
    // Two companies and no terminal: no default company.
    user.login_with_key(&mock).await.expect(0);
    assert_eq!(user.config()["profiles"]["default"]["company"], Value::Null);
    user.run(&["invoices", "list", "--json"]).await.expect(2);
    let mine = format!("/api/companies/{}/invoices/", mock.company_id);
    let alpha = "/api/companies/cmp0000002/invoices/";

    // `oscar -c X invoices list`.
    user.run(&["-c", "12345678", "invoices", "list", "--json"])
        .await
        .expect(0);
    assert_eq!(mock.requests_to(&mine).await.len(), 1);
    // The command's -c wins over the root's.
    user.run(&["-c", "alpha", "invoices", "list", "-c", "mock", "--json"])
        .await
        .expect(0);
    assert_eq!(mock.requests_to(&mine).await.len(), 2);
    assert!(mock.requests_to(alpha).await.is_empty());
    // The flag wins over OSCAR_COMPANY.
    user.set("OSCAR_COMPANY", "alpha");
    user.run(&["--company", "mock", "invoices", "list", "--json"])
        .await
        .expect(0);
    assert_eq!(mock.requests_to(&mine).await.len(), 3);
    assert!(mock.requests_to(alpha).await.is_empty());
    user.vars.retain(|(key, _)| key != "OSCAR_COMPANY");

    // `oscar companies get <COMPANY>`, by registry code, and with the root's -c.
    let path = format!("/api/companies/{}/", mock.company_id);
    let got = user
        .run(&["companies", "get", "12345678", "--json"])
        .await
        .expect(0)
        .json();
    assert_eq!(got["id"], mock.company_id.as_str());
    assert_eq!(mock.requests_to(&path).await.len(), 1);
    user.run(&["-c", "mock", "companies", "get", "--json"])
        .await
        .expect(0);
    assert_eq!(mock.requests_to(&path).await.len(), 2);
    // The same company twice is fine; two companies are a usage error, with no call.
    user.run(&["companies", "get", "12345678", "-c", "12345678", "--json"])
        .await
        .expect(0);
    let run = user
        .run(&["companies", "get", "12345678", "-c", "alpha"])
        .await
        .expect(2);
    insta::assert_snapshot!("companies_get_company_twice", run.stderr);
    user.run(&["-c", "alpha", "companies", "get", "12345678"])
        .await
        .expect(2);
    assert_eq!(mock.requests_to(&path).await.len(), 3);
    assert!(
        mock.requests_to("/api/companies/cmp0000002/")
            .await
            .is_empty()
    );
}
