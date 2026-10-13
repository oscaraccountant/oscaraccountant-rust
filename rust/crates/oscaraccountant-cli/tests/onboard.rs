//! `oscar onboard` against the scripted onboarding of `clients/mock-server`:
//! every step end to end with flags and no terminal, the stops and the errors, and the
//! agreement pager driven by a pseudo terminal.

mod support;

use serde_json::{Value, json};
use support::{Mock, Run, User, test_browser};

/// The answers of step 2, as `--answers` reads them.
const ANSWERS: &str = r#"{
    "activity_description": "Software for shops",
    "customer_countries": ["EE", "FI"],
    "supplier_countries": ["EE"],
    "expected_turnover": "under_50k",
    "cash_usage": "no",
    "uses_crypto_assets": false,
    "needs_licence": false,
    "residence": "all_estonia",
    "acts_for_others": false,
    "field_experience": "3_10"
}"#;

async fn mock() -> Mock {
    Mock::start(json!({"device_interval": 0})).await
}

/// A user logged in to `mock` with the device login (an OAuth token, as `oscar login` gives),
/// with fast polls.
async fn user(mock: &Mock) -> User {
    let mut user = User::new();
    user.login_with_device(mock).await.expect(0);
    user.set("OSCAR_POLL_INTERVAL_MS", "5")
        .set("OSCAR_POLL_TIMEOUT_SECS", "20");
    user
}

async fn onboarding(mock: &Mock, body: Value) -> Value {
    mock.control("POST", "/_mock/onboarding", Some(body)).await
}

fn answers_file(user: &User) -> String {
    let path = user.dir.path().join("answers.json");
    std::fs::write(&path, ANSWERS).unwrap();
    path.to_str().unwrap().to_owned()
}

#[tokio::test]
async fn onboards_a_company_end_to_end_without_a_terminal() {
    let mock = mock().await;
    let user = user(&mock).await;
    let answers = answers_file(&user);
    let run = user
        .run(&[
            "onboard",
            "--registry-code",
            "12345678",
            "--books-today",
            "no",
            "--books-no-reason",
            "A new company",
            "--no-website",
            "--confirm-owners",
            "--answers",
            &answers,
            "--answer",
            "startup_funds_source=Savings",
            "--consent",
            "--pep",
            "no",
            "--pep-related",
            "no",
            "--invite-links",
        ])
        .await;
    // Steps 1 to 3 run; the agreement needs a terminal, so the run stops there with its link.
    let run = run.expect(2);
    let err = &run.stderr;
    for text in [
        "Step 1 of 6 · Your company",
        "Mock Company OÜ",
        // English, never the Estonian of the register.
        "OÜ (private limited company)",
        "Status             Registered",
        "is created (company",
        "Step 2 of 6 · About your business",
        "The answers about your business are submitted",
        "Step 3 of 6 · Verify your identity",
        "Jaan Tamm · beneficial owner",
        "/kyc/link/tok_mock_1",
        "/_mock/didit/session",
        "Your identity is verified: Mari Maasikas, a board member",
        "Step 4 of 6 · Accept the client agreement",
        "accepted only after reading it",
        &format!("/c/{}/agreement", mock.company_id),
    ] {
        assert!(err.contains(text), "{text:?} in {err}");
    }
    assert!(!err.contains("Registrisse"), "{err}");
    let confirm = mock.requests_to("/api/onboarding/confirm/").await;
    assert_eq!(
        confirm[0]["body"]["value"],
        json!({"beneficial_owners_confirmed": true})
    );
    let path = format!(
        "/api/companies/{}/compliance/questionnaire/submit/",
        mock.company_id
    );
    let submitted = &mock.requests_to(&path).await[0]["body"]["value"];
    assert_eq!(submitted["startup_funds_source"], "Savings");
    assert_eq!(submitted["customer_countries"], json!(["EE", "FI"]));
    // Step 3 starts as the web page does: the caller had no requirement before the start.
    let path = format!(
        "/api/companies/{}/compliance/identity/start/",
        mock.company_id
    );
    let started = &mock.requests_to(&path).await[0]["body"]["value"];
    assert_eq!(
        started,
        &json!({"consent": true, "is_pep": false, "is_pep_related": false})
    );

    // The agreement is accepted in the browser; `oscar onboard` resumes at the plan.
    onboarding(&mock, json!({"step": "plan", "checkout_polls": 2})).await;
    let status = user
        .run(&["onboard", "status", "--json"])
        .await
        .expect(0)
        .json();
    assert_eq!(status["current_step"], "plan");
    let run = user.run(&["onboard", "--plan", "year"]).await.expect(0);
    for text in [
        "Step 5 of 6 · Choose your plan",
        "/_mock/checkout/cs_mock_1",
        "The yearly plan is active",
        "Step 6 of 6 · Set up",
        "e-MTA access",
        &format!("/c/{}/setup", mock.company_id),
        // Required items are open: never "done" (N2-7).
        "Left to do, required: People (due 2026-10-25), e-MTA access (due 2026-11-20).",
    ] {
        assert!(run.stderr.contains(text), "{text:?} in {}", run.stderr);
    }
    assert!(!run.stderr.contains("is done"), "{}", run.stderr);
    onboarding(&mock, json!({"setup_complete": true})).await;
    let run = user.run(&["onboard"]).await.expect(0);
    assert!(
        run.stderr
            .contains("The onboarding of Mock Company OÜ is done"),
        "{}",
        run.stderr
    );
    let path = format!("/api/companies/{}/billing/checkout/", mock.company_id);
    let checkout = &mock.requests_to(&path).await[0]["body"]["value"];
    assert_eq!(
        checkout,
        &json!({"plan": "year", "return_path": format!("/onboarding?company={}", mock.company_id)})
    );

    // A terminal: the steps for a person.
    let mut user = user;
    user.set("OSCAR_FORCE_TTY", "1");
    let status = user.run(&["onboard", "status"]).await.expect(0);
    assert!(
        status.stdout.contains("Mock Company OÜ"),
        "{}",
        status.stdout
    );
    assert!(
        status.stdout.contains("5. Choose your plan"),
        "{}",
        status.stdout
    );
    let pdf = user.dir.path().join("evidence.pdf");
    let run = user
        .run(&["onboard", "agreement", "--download", pdf.to_str().unwrap()])
        .await
        .expect(0);
    assert!(run.stderr.contains("Saved the evidence"), "{}", run.stderr);
    assert!(std::fs::read(&pdf).unwrap().starts_with(b"%PDF"));
    // An existing file is kept without --force (M5).
    std::fs::write(&pdf, b"kept").unwrap();
    let run = user
        .run(&["onboard", "agreement", "--download", pdf.to_str().unwrap()])
        .await
        .expect(2);
    assert!(
        run.stderr.contains("exists. Add --force to replace it"),
        "{}",
        run.stderr
    );
    assert_eq!(std::fs::read(&pdf).unwrap(), b"kept");
    // A dangling symbolic link is not followed without --force (M5).
    #[cfg(unix)]
    {
        let link = user.dir.path().join("link.pdf");
        let target = user.dir.path().join("elsewhere.pdf");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        user.run(&["onboard", "agreement", "--download", link.to_str().unwrap()])
            .await
            .expect(2);
        assert!(!target.exists());
    }
    user.run(&[
        "onboard",
        "agreement",
        "--download",
        pdf.to_str().unwrap(),
        "--force",
    ])
    .await
    .expect(0);
    assert!(std::fs::read(&pdf).unwrap().starts_with(b"%PDF"));
}

/// `--registry-code` with `-c/--company` is a usage error; with `OSCAR_COMPANY` only, the
/// registry code wins and onboards a new company (M6).
#[tokio::test]
async fn a_registry_code_wins_over_oscar_company() {
    let mock = mock().await;
    let mut user = user(&mock).await;
    onboarding(&mock, json!({"step": "done"})).await;
    let company = mock.company_id.clone();
    let run = user
        .run(&["onboard", "-c", &company, "--registry-code", "12345678"])
        .await
        .expect(2);
    assert!(
        run.stderr.contains("--registry-code") && run.stderr.contains("-c/--company"),
        "{}",
        run.stderr
    );
    user.set("OSCAR_COMPANY", &company);
    let run = user
        .run(&["onboard", "--registry-code", "12345678"])
        .await
        .expect(2);
    assert!(run.stderr.contains("Step 1 of 6"), "{}", run.stderr);
    assert!(run.stderr.contains("--books-today"), "{}", run.stderr);
    assert!(!run.stderr.contains("Step 6 of 6"), "{}", run.stderr);
}

#[tokio::test]
async fn every_terminal_step_has_a_flag() {
    let mock = mock().await;
    let user = user(&mock).await;
    let run = user.run(&["onboard"]).await.expect(2);
    assert!(
        run.stderr.contains("Give --registry-code <code>"),
        "{}",
        run.stderr
    );
    let run = user
        .run(&["onboard", "--registry-code", "12345678"])
        .await
        .expect(2);
    assert!(run.stderr.contains("--books-today"), "{}", run.stderr);
    let run = user
        .run(&[
            "onboard",
            "--registry-code",
            "12345678",
            "--books-today",
            "yes",
            "--books-software",
            "xolo",
        ])
        .await
        .expect(2);
    assert!(run.stderr.contains("--confirm-owners"), "{}", run.stderr);
    // Step 2: every missing answer is a usage error that names its flag, before anything is sent.
    onboarding(&mock, json!({"step": "business"})).await;
    let run = user.run(&["onboard", "--no-input"]).await.expect(2);
    for text in [
        "An answer to each question below is needed",
        "--no-input is set",
        "--website <url> or --no-website",
        "--answer customer_countries=<codes, for example EE,FI>",
        // The allowed values, not only after a wrong one (N3-6).
        "--answer field_experience=under_1|1_3|3_10|over_10",
    ] {
        assert!(run.stderr.contains(text), "{text:?} in {}", run.stderr);
    }
    let path = format!(
        "/api/companies/{}/compliance/questionnaire/",
        mock.company_id
    );
    let writes: Vec<Value> = mock
        .requests()
        .await
        .into_iter()
        .filter(|r| r["method"] != "GET" && r["path"].as_str().unwrap_or("").starts_with(&path))
        .collect();
    assert!(writes.is_empty(), "nothing is sent: {writes:?}");
    // The answers are all there but the website: step 2 asks it again.
    let answers = answers_file(&user);
    let run = user
        .run(&["onboard", "--answers", &answers])
        .await
        .expect(2);
    assert!(
        run.stderr
            .contains("The company website is needed. Give --website <url> or --no-website"),
        "{}",
        run.stderr
    );
    let run = user
        .run(&["onboard", "--answers", &answers, "--no-website"])
        .await
        .expect(2);
    assert!(run.stderr.contains("submitted"), "{}", run.stderr);
    let submitted = &mock.requests_to(&format!("{path}submit/")).await[0]["body"]["value"];
    assert_eq!(submitted["no_website"], true);
    onboarding(&mock, json!({"step": "identity"})).await;
    let run = user.run(&["onboard"]).await.expect(2);
    // Every missing flag of step 3 in one run (N3-5).
    for text in [
        "An answer to each question below is needed",
        "--consent ",
        "Didit Verification Privacy Notice",
        "--pep yes|no ",
        "--pep-related yes|no ",
    ] {
        assert!(run.stderr.contains(text), "{text:?} in {}", run.stderr);
    }
    let run = user.run(&["onboard", "--consent"]).await.expect(2);
    assert!(run.stderr.contains("--pep yes|no"), "{}", run.stderr);
    assert!(!run.stderr.contains("--consent "), "{}", run.stderr);
    let run = user
        .run(&[
            "onboard",
            "--consent",
            "--pep",
            "yes",
            "--pep-related",
            "no",
        ])
        .await
        .expect(2);
    assert!(
        run.stderr.contains("--pep-details <text>"),
        "{}",
        run.stderr
    );
    let start = format!(
        "/api/companies/{}/compliance/identity/start/",
        mock.company_id
    );
    assert!(mock.requests_to(&start).await.is_empty(), "nothing is sent");
    onboarding(&mock, json!({"step": "plan"})).await;
    let run = user.run(&["onboard", "--no-input"]).await.expect(2);
    assert!(run.stderr.contains("--plan month|year"), "{}", run.stderr);
    assert!(run.stderr.contains("--no-input is set"), "{}", run.stderr);
}

#[tokio::test]
async fn stops_and_explains() {
    let mock = mock().await;
    let user = user(&mock).await;
    // Pre-launch: the hint of the invite link.
    onboarding(&mock, json!({"pre_launch": true})).await;
    let run = user
        .run(&["onboard", "--registry-code", "12345678"])
        .await
        .expect(1);
    assert!(run.stderr.contains("pre_launch"), "{}", run.stderr);
    assert!(
        run.stderr.contains("If you have an invite link"),
        "{}",
        run.stderr
    );
    // A company that needs support, and owners the terminal cannot name.
    onboarding(&mock, json!({"pre_launch": false})).await;
    let run = user
        .run(&["onboard", "--registry-code", "87654321"])
        .await
        .expect(1);
    assert!(
        run.stderr.contains("joint_representation"),
        "{}",
        run.stderr
    );
    let run = user
        .run(&["onboard", "--registry-code", "11223344"])
        .await
        .expect(1);
    assert!(run.stderr.contains("beneficial owners"), "{}", run.stderr);
    // A blocked onboarding.
    onboarding(
        &mock,
        json!({"step": "identity", "blocked_reason": "no_estonian_eid"}),
    )
    .await;
    let run = user.run(&["onboard"]).await.expect(1);
    assert!(
        run.stderr
            .contains("needs our support team (no_estonian_eid)"),
        "{}",
        run.stderr
    );
    // The identity waits for staff, and --no-wait stops after the link.
    onboarding(
        &mock,
        json!({"step": "identity", "blocked_reason": "", "kyc_result": "needs_review"}),
    )
    .await;
    let flags = ["onboard", "--consent", "--pep", "no", "--pep-related", "no"];
    let run = user.run(&flags).await.expect(0);
    assert!(
        run.stderr.contains("We are checking your verification"),
        "{}",
        run.stderr
    );
    onboarding(&mock, json!({"step": "identity", "kyc_result": "approved"})).await;
    let run = user
        .run(&[&flags[..], &["--no-wait"]].concat())
        .await
        .expect(0);
    assert!(
        run.stderr
            .contains("Run `oscar onboard` again when the identity check is done"),
        "{}",
        run.stderr
    );
    // Embedded Checkout: the page of the web app, then the CLI waits for the plan step.
    onboarding(&mock, json!({"step": "plan", "embedded_checkout": true})).await;
    let run = user
        .run(&["onboard", "--plan", "month", "--no-wait"])
        .await
        .expect(0);
    assert!(
        run.stderr
            .contains(&format!("/onboarding?company={}", mock.company_id)),
        "{}",
        run.stderr
    );
    // The agreement PDF before the acceptance.
    onboarding(&mock, json!({"step": "agreement"})).await;
    let run = user
        .run(&["onboard", "agreement", "--download"])
        .await
        .expect(1);
    assert!(
        run.stderr.contains("Accept the agreement first"),
        "{}",
        run.stderr
    );
}

#[tokio::test]
async fn an_old_cli_and_no_login() {
    let mock = mock().await;
    let user = User::new();
    let run = user
        .run(&["onboard", "--base-url", &mock.url])
        .await
        .expect(3);
    assert!(
        run.stderr.contains("No company in Oscar yet?"),
        "{}",
        run.stderr
    );
    assert!(
        run.stderr.contains(&format!("{}/onboarding", mock.url)),
        "{}",
        run.stderr
    );
    let user = self::user(&mock).await;
    mock.control(
        "POST",
        "/_mock/responses",
        Some(json!({"responses": [{
            "match": {"path": "/api/me/"},
            "times": 5,
            "response": {"status": 200, "body": {"kind": "json", "value": {"cli_min_version": "99.0.0", "companies": []}}},
        }]})),
    )
    .await;
    let run = user.run(&["onboard", "status"]).await.expect(1);
    assert!(
        run.stderr.contains("needs 99.0.0 or later"),
        "{}",
        run.stderr
    );
}

#[tokio::test]
async fn a_new_organization_needs_a_new_login() {
    let mock = mock().await;
    let mut user = user(&mock).await;
    let browser_dir = tempfile::tempdir().unwrap();
    user.set("BROWSER", &test_browser(browser_dir.path()));
    onboarding(&mock, json!({"new_company_id": "cmpnewone01"})).await;
    let run: Run = user
        .run(&[
            "onboard",
            "--registry-code",
            "12345678",
            "--books-today",
            "yes",
            "--books-software",
            "merit_aktiva",
            "--website",
            "https://mock.example",
            "--confirm-owners",
        ])
        .await
        .expect(3);
    assert!(
        run.stderr.contains("is in a new organization"),
        "{}",
        run.stderr
    );
    assert!(run.stderr.contains("does not reach"), "{}", run.stderr);
    // The consent page cannot preselect it, so the CLI names the organization to choose.
    assert!(
        run.stderr.contains(
            "Important: on the consent page, choose the organization \"Mock Company OÜ\""
        ),
        "{}",
        run.stderr
    );
    assert!(
        run.stderr.contains("oscar onboard -c cmpnewone01"),
        "{}",
        run.stderr
    );
}

/// Step 3 with no requirement of the caller yet (P1-6): the consent, the PEP answer and the
/// start, as the web page does; then each state of the server has its own message (P2-21).
#[tokio::test]
async fn identity_states_say_what_to_do() {
    let mock = mock().await;
    let user = user(&mock).await;
    let start = format!(
        "/api/companies/{}/compliance/identity/start/",
        mock.company_id
    );
    let flags = ["onboard", "--consent", "--pep", "no", "--pep-related", "no"];
    for (state, text) in [
        ("register_unavailable", "in the Business Register now"),
        ("not_board_member", "/signup"),
        ("declined", "could not verify your identity online"),
    ] {
        onboarding(&mock, json!({"step": "identity"})).await;
        onboarding(&mock, json!({"identity_state": state})).await;
        let run = user.run(&flags).await.expect(0);
        assert!(
            run.stderr.contains(text),
            "{state}: {text:?} in {}",
            run.stderr
        );
        assert!(!run.stderr.contains("Waiting"), "{}", run.stderr);
    }
    assert!(mock.requests_to(&start).await.is_empty(), "no start");
    // A cancelled Smart-ID starts again at once.
    onboarding(&mock, json!({"step": "identity"})).await;
    onboarding(&mock, json!({"identity_state": "retry"})).await;
    let run = user
        .run(&[&flags[..], &["--no-wait"]].concat())
        .await
        .expect(0);
    assert!(run.stderr.contains("cancelled or failed"), "{}", run.stderr);
    assert!(
        run.stderr.contains("/_mock/didit/session"),
        "{}",
        run.stderr
    );
    assert_eq!(mock.requests_to(&start).await.len(), 1);
    // A check that runs, resumed with no terminal (P2-21): how to get its link, then the link.
    onboarding(&mock, json!({"step": "identity"})).await;
    onboarding(
        &mock,
        json!({"identity_state": "in_progress", "pep_answered": true}),
    )
    .await;
    let run = user.run(&["onboard", "--no-wait"]).await.expect(0);
    assert!(
        run.stderr.contains("Your identity check is running"),
        "{}",
        run.stderr
    );
    assert!(
        run.stderr.contains("`oscar onboard --consent`"),
        "{}",
        run.stderr
    );
    assert!(
        !run.stderr.contains("/_mock/didit/session"),
        "{}",
        run.stderr
    );
    let before = mock.requests_to(&start).await.len();
    let run = user
        .run(&["onboard", "--consent", "--no-wait"])
        .await
        .expect(0);
    assert!(
        run.stderr.contains("/_mock/didit/session"),
        "{}",
        run.stderr
    );
    let again = &mock.requests_to(&start).await[before]["body"]["value"];
    assert_eq!(again, &json!({"consent": true}), "the PEP answer is kept");
    // Verified, with the PEP question still open: the CLI asks it, and the step is done.
    onboarding(&mock, json!({"step": "identity"})).await;
    onboarding(
        &mock,
        json!({"identity_state": "passed", "pep_answered": false}),
    )
    .await;
    let run = user.run(&flags).await.expect(2);
    assert!(
        run.stderr.contains("One question is left"),
        "{}",
        run.stderr
    );
    assert!(
        run.stderr
            .contains("Your identity is verified: Mari Maasikas"),
        "{}",
        run.stderr
    );
    assert!(
        run.stderr
            .contains("Step 4 of 6 · Accept the client agreement"),
        "{}",
        run.stderr
    );
    let answered = &mock.requests_to(&start).await.last().unwrap()["body"]["value"].clone();
    assert_eq!(answered["is_pep"], false);
    // With no flag and no terminal, the PEP question names its flag.
    onboarding(&mock, json!({"step": "identity"})).await;
    let run = user.run(&["onboard", "--consent"]).await.expect(2);
    assert!(run.stderr.contains("--pep yes|no"), "{}", run.stderr);
}

/// A finished onboarding resumes at the setup checklist (P1-7); a company that came through no
/// onboarding says so (P2-22); the agreement names the step that is missing (P3-20).
#[tokio::test]
async fn resumes_where_the_company_stands() {
    let mock = mock().await;
    let mut user = user(&mock).await;
    onboarding(&mock, json!({"step": "done"})).await;
    let run = user.run(&["onboard"]).await.expect(0);
    assert!(
        run.stderr.contains("Step 6 of 6 · Set up"),
        "{}",
        run.stderr
    );
    assert!(!run.stderr.contains("Step 1 of 6"), "{}", run.stderr);
    assert!(
        run.stderr.contains("oscar onboard --registry-code <code>"),
        "{}",
        run.stderr
    );
    // --registry-code onboards a new company, not the default one: the server allows it once
    // every onboarding is done, so the pick is saved and the next answer is asked.
    let run = user
        .run(&["onboard", "--registry-code", "12345678"])
        .await
        .expect(2);
    assert!(run.stderr.contains("Step 1 of 6"), "{}", run.stderr);
    assert!(run.stderr.contains("--books-today"), "{}", run.stderr);
    onboarding(&mock, json!({"step": "done"})).await;

    onboarding(&mock, json!({"has_onboarding": false})).await;
    let run = user.run(&["onboard", "--no-input"]).await.expect(2);
    assert!(
        run.stderr
            .contains("needs no onboarding: it is set up already. This onboards a new company."),
        "{}",
        run.stderr
    );
    assert!(run.stderr.contains("--registry-code"), "{}", run.stderr);
    let status = user
        .run(&["onboard", "status", "--json"])
        .await
        .expect(0)
        .json();
    assert_eq!(status["onboarding"], Value::Null);
    let company = mock.company_id.clone();
    let run = user
        .run(&["onboard", "agreement", "-c", &company])
        .await
        .expect(2);
    assert!(run.stderr.contains("needs no onboarding"), "{}", run.stderr);
    // An explicit set-up company: the same clear line, not a 404 (P2-22).
    let run = user.run(&["onboard", "-c", &company]).await.expect(2);
    assert!(
        run.stderr
            .contains("needs no onboarding: it is set up already"),
        "{}",
        run.stderr
    );
    assert!(!run.stderr.contains("404"), "{}", run.stderr);
    assert!(run.stderr.contains("--registry-code"), "{}", run.stderr);

    onboarding(&mock, json!({"has_onboarding": true, "step": "identity"})).await;
    let run = user.run(&["onboard", "agreement"]).await.expect(2);
    assert!(
        run.stderr
            .contains("Step 3 (Verify your identity) is not done yet"),
        "{}",
        run.stderr
    );
    user.set("OSCAR_FORCE_TTY", "1");
    let status = user.run(&["onboard", "status"]).await.expect(0);
    assert!(status.stdout.contains("about 3 min"), "{}", status.stdout);
    onboarding(&mock, json!({"has_onboarding": false, "step": "done"})).await;
    let run = user.run(&["onboard", "status"]).await.expect(0);
    assert!(run.stdout.contains("needs no onboarding"), "{}", run.stdout);
}

/// Every company of the person is set up (P1, a second company): on a terminal `oscar onboard`
/// offers a new company, as "New organization" of the web app, and its step 1 starts; the
/// other choice shows the setup checklist as before.
#[cfg(unix)]
#[tokio::test]
async fn offers_a_new_company_once_every_company_is_set_up() {
    let mock = mock().await;
    let user = user(&mock).await;
    onboarding(&mock, json!({"step": "done", "setup_complete": true})).await;
    let dir = user.dir.path().to_owned();
    let screen = tokio::task::spawn_blocking(move || {
        let mut session = pty::Session::start(&dir, &["onboard"]);
        session.expect("is done");
        session.expect("Onboard a new company");
        session.keys("\r");
        session.expect("Your company");
        session.keys("12345678\r");
        session.expect("Does the company keep books today?");
        session.keys("\x03");
        session.finish()
    })
    .await
    .unwrap();
    assert_ne!(screen.code, 0, "{}", screen.output);
    assert!(screen.output.contains("Step 1 of 6"), "{}", screen.output);
    assert!(!screen.output.contains("Step 6 of 6"), "{}", screen.output);
    let picked: Vec<Value> = mock
        .requests_to("/api/onboarding/")
        .await
        .into_iter()
        .filter(|r| r["method"] == "PATCH")
        .map(|r| r["body"]["value"]["registry_code"].clone())
        .collect();
    assert_eq!(picked, [json!("12345678")]);

    // The other choice: the checklist of the company that is set up, and no new company.
    onboarding(&mock, json!({"step": "done", "setup_complete": true})).await;
    let dir = user.dir.path().to_owned();
    let screen = tokio::task::spawn_blocking(move || {
        let mut session = pty::Session::start(&dir, &["onboard"]);
        session.expect("See the setup checklist");
        session.keys("\x1b[B\r");
        session.finish()
    })
    .await
    .unwrap();
    assert_eq!(screen.code, 0, "{}", screen.output);
    assert!(screen.output.contains("Step 6 of 6"), "{}", screen.output);
    assert!(
        screen
            .output
            .contains("The onboarding of Mock Company OÜ is done"),
        "{}",
        screen.output
    );
}

/// The agreement pager in a pseudo terminal: read to the end, type `I accept`, and the CLI
/// accepts with the version, the language and the hash it showed.
#[cfg(unix)]
#[tokio::test]
async fn the_pager_accepts_the_agreement_in_a_terminal() {
    let mock = mock().await;
    let user = user(&mock).await;
    onboarding(&mock, json!({"step": "agreement", "agreement_changes": 1})).await;
    let company = mock.company_id.clone();
    let dir = user.dir.path().to_owned();
    let screen = tokio::task::spawn_blocking(move || {
        pty::run(&dir, &["onboard", "agreement", "-c", &company])
    })
    .await
    .unwrap();
    assert_eq!(screen.code, 0, "{}", screen.output);
    assert!(screen.output.contains("SHA-256"), "{}", screen.output);
    assert!(
        screen.output.contains("changed while you read it"),
        "{}",
        screen.output
    );
    assert!(
        screen.output.contains("The client agreement is accepted"),
        "{}",
        screen.output
    );
    let path = format!(
        "/api/companies/{}/onboarding/agreement/accept/",
        mock.company_id
    );
    let accepts = mock.requests_to(&path).await;
    assert_eq!(accepts.len(), 2, "the first answer was agreement_changed");
    let body = &accepts[1]["body"]["value"];
    assert_eq!(body["language"], "et");
    assert_eq!(body["version"], "2026-10-10-r");
    assert_eq!(body["text_sha256"].as_str().unwrap().len(), 64);
    let state = mock.control("GET", "/_mock/onboarding", None).await;
    assert_eq!(state["acceptance"]["channel"], "cli");
}

/// The company search in a pseudo terminal (P1-8): text typed with no row chosen never ends the
/// run. A text that names no company asks again; one that names several offers them.
#[cfg(unix)]
#[tokio::test]
async fn the_search_keeps_the_prompt_open() {
    let mock = mock().await;
    let user = user(&mock).await;
    let dir = user.dir.path().to_owned();
    let screen = tokio::task::spawn_blocking(move || {
        let mut session = pty::Session::start(&dir, &["onboard"]);
        session.expect("Your company");
        session.keys("zz\r");
        session.expect("No company of the Business Register matches `zz`");
        session.expect("Your company");
        // `in` names Joint Board AS and Holding Chain OÜ: Enter with no row chosen offers both.
        session.keys("in\r");
        session.expect("Which company matches `in`?");
        session.expect("Holding Chain");
        session.keys("\r");
        session.finish()
    })
    .await
    .unwrap();
    // The first one needs our support team: the run ends there, after the pick.
    assert_eq!(screen.code, 1, "{}", screen.output);
    assert!(
        screen.output.contains("joint_representation"),
        "{}",
        screen.output
    );
    let drafts = mock.requests_to("/api/onboarding/").await;
    let picked: Vec<&Value> = drafts
        .iter()
        .filter(|r| r["method"] == "PATCH")
        .map(|r| &r["body"]["value"]["registry_code"])
        .collect();
    assert_eq!(picked, [&json!("87654321")]);
}

/// The questions of step 2 in a pseudo terminal (P2-19): a yes or no risk question has no
/// default, so Enter alone asks again; each question shows its place; the website is asked
/// again; the activity of the register is offered in English.
#[cfg(unix)]
#[tokio::test]
async fn a_risk_question_has_no_default_answer() {
    let mock = mock().await;
    let user = user(&mock).await;
    onboarding(&mock, json!({"step": "business"})).await;
    let dir = user.dir.path().to_owned();
    let screen = tokio::task::spawn_blocking(move || {
        let mut session = pty::Session::start(&dir, &["onboard"]);
        session.expect("The company website");
        session.keys("\r");
        session.expect("(1/10)");
        session.expect("Computer programming (EMTAK 62011)");
        session.keys("\r");
        session.expect("(2/10)");
        session.keys("EE\r");
        session.expect("(3/10)");
        session.keys("EE\r");
        session.expect("(4/10)");
        session.keys("\r");
        session.expect("(5/10)");
        session.keys("\r");
        session.expect("(6/10)");
        // A question with no default says how to answer (N3-4).
        session.expect("(y/n)");
        session.keys("\r");
        session.expect("Answer y for yes or n for no.");
        session.keys("n\r");
        session.expect("(7/10)");
        session.keys("n\r");
        session.expect("(8/10)");
        session.keys("\r");
        session.expect("(9/10)");
        session.keys("n\r");
        session.expect("(10/10)");
        session.keys("\r");
        // The review shows a "no" answer (N2-3).
        session.expect("Your answers");
        session.expect("Does it hold, accept or pay with crypto-assets?");
        session.expect("No");
        session.expect("Does it need a licence?");
        session.expect("No");
        session.expect("Submit these answers?");
        session.keys("\r");
        session.expect("privacy notice");
        session.keys("\x1b");
        session.finish()
    })
    .await
    .unwrap();
    assert_eq!(screen.code, 2, "{}", screen.output);
    let path = format!(
        "/api/companies/{}/compliance/questionnaire/submit/",
        mock.company_id
    );
    let submitted = &mock.requests_to(&path).await[0]["body"]["value"];
    assert_eq!(submitted["uses_crypto_assets"], false);
    assert_eq!(submitted["no_website"], true);
    assert_eq!(
        submitted["activity_description"],
        "Computer programming (EMTAK 62011)"
    );
}

/// The scripted onboarding of `OÜ Selfi`, at step 3, in another organization than the login's:
/// its company routes answer 404 to this login (N3-P1-1).
async fn open_elsewhere(mock: &Mock) {
    onboarding(
        mock,
        json!({
            "new_company_id": "cmpselfi001",
            "company_name": "OÜ Selfi",
            "organization_id": "org0000002",
            "organization_name": "OÜ Selfi",
            "step": "identity",
        }),
    )
    .await;
}

const ELSEWHERE: &str = "The onboarding of OÜ Selfi (cmpselfi001) is open in the organization \"OÜ Selfi\", and this login is for \"Mock Organization OÜ\".";

/// An onboarding open in another organization: never a bare 404 and never "set up already",
/// but the organization to log in to (N3-P1-1).
#[tokio::test]
async fn an_onboarding_open_in_another_organization() {
    let mock = mock().await;
    let mut user = user(&mock).await;
    open_elsewhere(&mock).await;
    for args in [
        &["onboard", "--no-input"][..],
        &["onboard", "--registry-code", "12345678", "--no-input"],
        &["onboard", "agreement", "--no-input"],
    ] {
        let run = user.run(args).await.expect(3);
        assert!(run.stderr.contains(ELSEWHERE), "{args:?}: {}", run.stderr);
        assert!(
            run.stderr
                .contains("Run `oscar login`, choose the organization \"OÜ Selfi\""),
            "{}",
            run.stderr
        );
        assert!(!run.stderr.contains("404"), "{}", run.stderr);
    }
    // The registry code of another company: the open one must finish first (N3-P2-1).
    let run = user
        .run(&["onboard", "--registry-code", "87654321", "--no-input"])
        .await
        .expect(2);
    assert!(
        run.stderr.contains(
            "The onboarding of OÜ Selfi, registry code 12345678 is still open, at step 3 (Verify your identity). Finish it before you onboard another company."
        ),
        "{}",
        run.stderr
    );
    assert!(run.stderr.contains(ELSEWHERE), "{}", run.stderr);
    // The status: the person's steps, never "set up already".
    let status = user
        .run(&["onboard", "status", "--json"])
        .await
        .expect(0)
        .json();
    assert_eq!(status["current_step"], "identity");
    assert_eq!(status["company"]["organization"]["name"], "OÜ Selfi");
    user.set("OSCAR_FORCE_TTY", "1");
    let run = user.run(&["onboard", "status"]).await.expect(0);
    assert!(
        run.stdout.contains("OÜ Selfi (cmpselfi001)"),
        "{}",
        run.stdout
    );
    assert!(
        run.stdout.contains("Verify your identity"),
        "{}",
        run.stdout
    );
    assert!(run.stdout.contains(ELSEWHERE), "{}", run.stdout);
    assert!(!run.stdout.contains("set up already"), "{}", run.stdout);
}

/// `--registry-code` of another company while one onboarding is open in this organization:
/// said, not ignored; the code of the open one goes on with it (N3-P2-1).
#[tokio::test]
async fn one_open_onboarding_at_a_time() {
    let mock = mock().await;
    let user = user(&mock).await;
    onboarding(&mock, json!({"step": "identity"})).await;
    let run = user
        .run(&["onboard", "--registry-code", "87654321", "--no-input"])
        .await
        .expect(2);
    assert!(
        run.stderr.contains(
            "The onboarding of Mock Company OÜ, registry code 12345678 is still open, at step 3 (Verify your identity)."
        ),
        "{}",
        run.stderr
    );
    assert!(
        run.stderr
            .contains("Go on with it: `oscar onboard` (without --registry-code)."),
        "{}",
        run.stderr
    );
    onboarding(&mock, json!({"step": "done"})).await;
    let run = user
        .run(&["onboard", "--registry-code", "12345678", "--no-input"])
        .await;
    // Done: no onboarding is open, so the code starts a new company (step 1).
    assert!(run.stderr.contains("Step 1 of 6"), "{}", run.stderr);
    onboarding(&mock, json!({"step": "plan"})).await;
    let run = user
        .run(&["onboard", "--registry-code", "12345678", "--no-input"])
        .await;
    assert!(
        run.stderr
            .contains("Mock Company OÜ (12345678) is onboarding already: this goes on at step 5 (Choose your plan)."),
        "{}",
        run.stderr
    );
}

/// In a terminal, the CLI offers the login to the organization of the open onboarding, with
/// the consent-page hint (N3-P1-1). The mock binds every login to its own organization, so the
/// new login still does not reach the company, and the CLI says what to run.
#[cfg(unix)]
#[tokio::test]
async fn offers_the_login_to_the_organization_of_the_open_onboarding() {
    let mock = mock().await;
    let user = user(&mock).await;
    open_elsewhere(&mock).await;
    let dir = user.dir.path().to_owned();
    let browser = test_browser(&dir);
    let screen = tokio::task::spawn_blocking(move || {
        let mut session = pty::Session::start_with(&dir, &["onboard"], &[("BROWSER", &browser)]);
        session.expect("is open in the organization");
        session.expect("Log in to");
        session.keys("\r");
        session.finish()
    })
    .await
    .unwrap();
    let output = &screen.output;
    assert_eq!(screen.code, 3, "{output}");
    assert!(
        output.contains("Important: on the consent page, choose the organization \"OÜ Selfi\"."),
        "{output}"
    );
    assert!(
        output.contains("Not \"Mock Organization OÜ\": that login cannot reach the company."),
        "{output}"
    );
    // The same server and profile: the login asks for neither.
    assert!(!output.contains("? Server"), "{output}");
    assert!(output.contains("Logged in as"), "{output}");
    assert!(
        output.contains("This login does not reach OÜ Selfi (cmpselfi001)."),
        "{output}"
    );
}

/// An account with no company yet (N2-4): the CLI asks first, and when the person says "log
/// in" anyway, the consent page's denial (`reason=no_organization`) leads to step 1 in the
/// browser, then a second login in the browser with no question of the method.
#[cfg(unix)]
#[tokio::test]
async fn an_account_with_no_company_goes_to_the_browser() {
    let mock = Mock::start(json!({"authorize_decision": "no_organization"})).await;
    let user = User::new();
    let browser = test_browser(user.dir.path());
    let dir = user.dir.path().to_owned();
    let url = mock.url.clone();
    let screen = tokio::task::spawn_blocking(move || {
        let mut session = pty::Session::start_with(
            &dir,
            &["onboard", "--base-url", &url],
            &[("BROWSER", &browser)],
        );
        session.expect("Do you already have a company in Oscar?");
        session.keys("\r");
        session.expect("How do you want to log in?");
        // `BROWSER` is set: the browser is the first row.
        session.keys("\r");
        session.expect("Your Oscar account has no company yet");
        session.expect("/onboarding");
        session.expect("Is your company created?");
        session.expect("(y/n)");
        session.keys("y\r");
        session.finish()
    })
    .await
    .unwrap();
    // The mock still has no organization for the account: the second login says so again.
    assert_eq!(screen.code, 3, "{}", screen.output);
    // Two logins in the browser; the method is asked before the first one only.
    let output = &screen.output;
    assert_eq!(output.matches("Opening the browser").count(), 2, "{output}");
    let first = output.find("Opening the browser").unwrap();
    assert!(
        !output[first..].contains("How do you want to log in?"),
        "{output}"
    );
    assert!(output.contains("Opened the onboarding:"), "{output}");
    let errors = mock.wait_for_requests("/cli/error", 2).await;
    assert_eq!(errors.len(), 2, "{errors:?}");
    assert!(
        errors
            .iter()
            .all(|r| r["query"] == json!([["reason", "no_organization"]]))
    );
}

/// Ctrl-C during the wait for Didit ends the wait with the way to go on (N3-8).
#[cfg(unix)]
#[tokio::test]
async fn ctrl_c_ends_the_wait_and_says_how_to_go_on() {
    let mock = mock().await;
    let user = user(&mock).await;
    onboarding(&mock, json!({"step": "identity", "kyc_polls": 100000})).await;
    let dir = user.dir.path().to_owned();
    let browser = test_browser(&dir);
    let screen = tokio::task::spawn_blocking(move || {
        let mut session = pty::Session::start_with(
            &dir,
            &["onboard", "--consent", "--pep", "no", "--pep-related", "no"],
            &[("OSCAR_POLL_INTERVAL_MS", "50"), ("BROWSER", &browser)],
        );
        session.expect("Make an invite link");
        session.keys("\r");
        session.expect("Waiting for the identity check");
        session.keys("\x03");
        session.finish()
    })
    .await
    .unwrap();
    assert_eq!(screen.code, 130, "{}", screen.output);
    assert!(
        screen
            .output
            .contains("Stopped waiting. Nothing is lost: run `oscar onboard` to go on."),
        "{}",
        screen.output
    );
}

#[cfg(unix)]
mod pty {
    //! One run of `oscar` in a pseudo terminal, with keys typed as the screen asks for them.

    use std::io::{Read, Write};
    use std::path::Path;
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};

    use super::User;

    pub struct Screen {
        pub code: u32,
        pub output: String,
    }

    /// A run of `oscar` in a pseudo terminal of 80 columns and 24 rows.
    pub struct Session {
        child: Box<dyn Child + Send + Sync>,
        _master: Box<dyn MasterPty + Send>,
        output: Arc<Mutex<Vec<u8>>>,
        keys: Box<dyn Write + Send>,
        at: usize,
    }

    impl Session {
        pub fn start(dir: &Path, args: &[&str]) -> Self {
            Self::start_with(dir, args, &[])
        }

        /// [`start`](Self::start), with more variables of the environment.
        pub fn start_with(dir: &Path, args: &[&str], vars: &[(&str, &str)]) -> Self {
            let pair = native_pty_system()
                .openpty(PtySize {
                    rows: 24,
                    cols: 80,
                    pixel_width: 0,
                    pixel_height: 0,
                })
                .expect("a pseudo terminal");
            let mut command = CommandBuilder::new(User::binary());
            command.args(args);
            command.cwd(dir);
            for (name, _) in std::env::vars_os() {
                let text = name.to_string_lossy();
                if text.starts_with("OSCAR_")
                    || ["BROWSER", "SSH_CONNECTION", "NO_COLOR", "WAYLAND_DISPLAY"]
                        .contains(&text.as_ref())
                {
                    command.env_remove(&name);
                }
            }
            command.env("OSCAR_CONFIG_DIR", dir);
            command.env("OSCAR_CREDENTIAL_STORE", "file");
            command.env("TERM", "xterm-256color");
            command.env("NO_COLOR", "1");
            // A display, the same on every machine, so the browser login is the first choice.
            command.env("DISPLAY", ":99");
            // Never the real browser: `true` opens nothing; a test may set its own.
            command.env("BROWSER", "true");
            for (name, value) in vars {
                command.env(name, value);
            }
            let child = pair
                .slave
                .spawn_command(command)
                .expect("oscar runs in the terminal");
            drop(pair.slave);
            let output = Arc::new(Mutex::new(Vec::new()));
            let mut reader = pair.master.try_clone_reader().expect("a reader");
            let sink = output.clone();
            std::thread::spawn(move || {
                let mut buffer = [0_u8; 4096];
                while let Ok(read) = reader.read(&mut buffer) {
                    if read == 0 {
                        break;
                    }
                    sink.lock().unwrap().extend_from_slice(&buffer[..read]);
                }
            });
            let keys = pair.master.take_writer().expect("a writer");
            Self {
                child,
                _master: pair.master,
                output,
                keys,
                at: 0,
            }
        }

        /// Wait for `text` after the last text waited for. The words of the screen are drawn
        /// with cursor moves between them, so a test waits for one word at a time.
        pub fn expect(&mut self, text: &str) {
            let started = Instant::now();
            loop {
                let seen = String::from_utf8_lossy(&self.output.lock().unwrap()).into_owned();
                if let Some(at) = seen.get(self.at..).and_then(|rest| rest.find(text)) {
                    self.at += at + text.len();
                    return;
                }
                assert!(
                    started.elapsed() < Duration::from_secs(60),
                    "no {text:?} on the screen: {seen}"
                );
                std::thread::sleep(Duration::from_millis(20));
            }
        }

        pub fn keys(&mut self, text: &str) {
            self.keys.write_all(text.as_bytes()).unwrap();
            self.keys.flush().unwrap();
            std::thread::sleep(Duration::from_millis(100));
        }

        /// Wait for the end of the run.
        pub fn finish(mut self) -> Screen {
            let started = Instant::now();
            let status = loop {
                if let Some(status) = self.child.try_wait().expect("the status") {
                    break status;
                }
                assert!(
                    started.elapsed() < Duration::from_secs(60),
                    "oscar did not end: {}",
                    String::from_utf8_lossy(&self.output.lock().unwrap())
                );
                std::thread::sleep(Duration::from_millis(20));
            };
            std::thread::sleep(Duration::from_millis(200));
            let output = String::from_utf8_lossy(&self.output.lock().unwrap()).into_owned();
            Screen {
                code: status.exit_code(),
                output,
            }
        }
    }

    /// The agreement: read it, switch the language, accept; the server changes the text once.
    pub fn run(dir: &Path, args: &[&str]) -> Screen {
        let mut session = Session::start(dir, args);
        // The pager opens in English; `a` before the end does not accept.
        session.expect("SHA-256");
        session.keys("a");
        session.expect("before");
        // Switch to Estonian, go to the end, accept: the server says the text changed.
        session.keys("l");
        session.expect("Kliendileping");
        session.keys("G");
        session.keys("a");
        session.expect("(Esc");
        session.keys("I accept\r");
        // The new version opens; read it to the end again and accept.
        session.expect("Kliendileping");
        session.keys("\x1b[F");
        session.keys("a");
        session.expect("(Esc");
        session.keys("I accept\r");
        session.finish()
    }
}
