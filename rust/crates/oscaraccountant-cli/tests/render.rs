//! The snapshots of the output for a person: every
//! list and detail command against the fixtures of the mock server, at 80 and 120 columns,
//! with and without colour. `cargo insta review` shows a change.
//!
//! Each run forces a terminal (`OSCAR_FORCE_TTY`), the width (`COLUMNS`) and the time
//! (`OSCAR_NOW`), so the snapshots do not depend on the machine.

mod support;

use support::{Mock, User};

/// The time of every run: relative dates and overdue days are stable.
const NOW: &str = "2026-10-11T12:00:00Z";

/// The company and the ids of the fixtures.
const COMPANY: &str = "cmp0000001";

/// Every command that prints an answer of the API, with its arguments.
const COMMANDS: &[(&str, &[&str])] = &[
    ("accounts_list", &["accounts", "list"]),
    ("bank_accounts_list", &["bank-accounts", "list"]),
    ("bank_transactions_list", &["bank-transactions", "list"]),
    ("dimensions_list", &["dimensions", "list"]),
    ("documents_list", &["documents", "list"]),
    ("invoices_list", &["invoices", "list"]),
    ("parties_list", &["parties", "list"]),
    ("periods_list", &["periods", "list"]),
    ("vat_codes_list", &["vat-codes", "list"]),
    ("files_list", &["files", "list"]),
    ("inbox_list_emails", &["inbox", "list-emails"]),
    ("organizations_list", &["organizations", "list"]),
    ("companies_list", &["companies", "list"]),
    (
        "bank_transactions_get",
        &["bank-transactions", "get", "01J90000000000000000000029"],
    ),
    (
        "documents_get",
        &["documents", "get", "01J90000000000000000000096"],
    ),
    (
        "invoices_get",
        &["invoices", "get", "01J90000000000000000000116"],
    ),
    (
        "parties_get",
        &["parties", "get", "01J90000000000000000000179"],
    ),
    ("files_get", &["files", "get", "fil0000001"]),
    ("companies_get", &["companies", "get"]),
    (
        "organizations_get",
        &["organizations", "get", "--organization", "org0000001"],
    ),
    ("me", &["me"]),
    ("inbox", &["inbox"]),
    (
        "general_ledger",
        &[
            "general-ledger",
            "--account-code",
            "1000",
            "--date-from",
            "2026-01-01",
            "--date-to",
            "2026-09-30",
        ],
    ),
    ("open_items", &["open-items", "--as-of", "2026-09-30"]),
    ("trial_balance", &["trial-balance", "--period", "2026-09"]),
    (
        "vat_preview",
        &[
            "vat-preview",
            "--period-from",
            "2026-09-01",
            "--period-to",
            "2026-09-30",
        ],
    ),
    (
        "balance_sheet",
        &["reports", "balance-sheet", "--date", "2026-09-30"],
    ),
    (
        "profit_and_loss",
        &[
            "reports",
            "profit-and-loss",
            "--date-from",
            "2026-01-01",
            "--date-to",
            "2026-09-30",
        ],
    ),
];

async fn logged_in() -> (Mock, User) {
    let mock = Mock::start(serde_json::json!({"device_interval": 0})).await;
    let mut user = User::new();
    user.login_with_key(&mock).await.expect(0);
    user.set("OSCAR_FORCE_TTY", "1")
        .set("OSCAR_NOW", NOW)
        .set("FORCE_HYPERLINK", "0")
        .set("LC_ALL", "en_US.UTF-8");
    (mock, user)
}

/// One run at this width, with or without colour: its standard output, then its standard error.
async fn shown(user: &mut User, args: &[&str], columns: &str, color: bool) -> String {
    let saved = user.vars.clone();
    user.set("COLUMNS", columns);
    if color {
        user.set("NO_COLOR", "").set("CLICOLOR_FORCE", "1");
    }
    let run = user.run(args).await;
    user.vars = saved;
    assert_eq!(run.code, 0, "{args:?}: {}", run.stderr);
    let mut text = run.stdout;
    if !run.stderr.is_empty() {
        text.push_str("--- stderr\n");
        text.push_str(&run.stderr);
    }
    text
}

#[tokio::test]
async fn every_list_and_detail_command() {
    let (_mock, mut user) = logged_in().await;
    for (name, args) in COMMANDS {
        let mut args: Vec<&str> = args.to_vec();
        if !matches!(args[0], "organizations" | "companies" | "me")
            || (args[0] == "companies" && args[1] == "get")
        {
            args.extend(["--company", COMPANY]);
        }
        if args[0] == "companies" && args[1] == "list" {
            args.extend(["--organization", "org0000001"]);
        }
        for color in [false, true] {
            let mut text = String::new();
            for columns in ["80", "120"] {
                text.push_str(&format!("=== {columns} columns\n"));
                text.push_str(&shown(&mut user, &args, columns, color).await);
            }
            let snapshot = if color {
                format!("{name}_color")
            } else {
                (*name).to_owned()
            };
            insta::assert_snapshot!(snapshot, text, &args.join(" "));
        }
    }
}

#[tokio::test]
async fn output_options() {
    let (_mock, mut user) = logged_in().await;
    let company = ["--company", COMPANY];
    let run = |extra: &'static [&'static str]| {
        let mut args = vec!["invoices", "list"];
        args.extend(company);
        args.extend(extra);
        args
    };
    let wide = shown(&mut user, &run(&["-o", "wide"]), "200", false).await;
    insta::assert_snapshot!("invoices_list_wide", wide);
    let columns = shown(
        &mut user,
        &run(&["--columns", "number,due_date"]),
        "80",
        false,
    )
    .await;
    insta::assert_snapshot!("invoices_list_columns", columns);
    let jq = shown(&mut user, &run(&["--jq", ".items[].number"]), "80", false).await;
    insta::assert_snapshot!("invoices_list_jq", jq);
    let template = shown(
        &mut user,
        &run(&[
            "--template",
            "{% for i in items %}{{ i.number }} {{ i.state }}\n{% endfor %}",
        ]),
        "80",
        false,
    )
    .await;
    insta::assert_snapshot!("invoices_list_template", template);
    let fields = shown(&mut user, &run(&["--json=number,state"]), "80", false).await;
    insta::assert_snapshot!("invoices_list_json_fields", fields);
    let ndjson = shown(&mut user, &run(&["-o", "ndjson", "--all"]), "80", false).await;
    assert!(ndjson.lines().all(|line| line.starts_with('{')), "{ndjson}");

    user.set("COLUMNS", "80");
    let error = user.run(&run(&["--columns", "nmber"])).await.expect(2);
    assert!(
        error.stderr.contains("Did you mean: number?"),
        "{}",
        error.stderr
    );
    let error = user.run(&run(&["--json=nmber"])).await.expect(2);
    assert!(error.stderr.contains("number"), "{}", error.stderr);
    let error = user.run(&run(&["--jq", ".["])).await.expect(2);
    assert!(error.stderr.contains("not a valid jq"), "{}", error.stderr);

    // The SDK name stays as a hidden alias; the id takes a unique prefix or the short form.
    let full = user
        .run(&[
            "invoices",
            "retrieve",
            "01J90000000000000000000116",
            "--company",
            COMPANY,
            "--json",
        ])
        .await
        .expect(0)
        .json();
    user.vars.retain(|(key, _)| key != "OSCAR_FORCE_TTY");
    let plain = &user;
    let list = plain
        .run(&["invoices", "list", "--company", COMPANY, "--json"])
        .await
        .expect(0)
        .json();
    let ids: Vec<&str> = list["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|item| item["id"].as_str())
        .collect();
    let id = full["id"].as_str().unwrap();
    let short = &id[18..];
    if ids.iter().filter(|other| other.ends_with(short)).count() == 1 {
        let by_short = plain
            .run(&["invoices", "get", short, "--company", COMPANY])
            .await
            .expect(0)
            .json();
        assert_eq!(by_short["id"], full["id"]);
    }
    let ambiguous = plain
        .run(&["invoices", "get", "01J9", "--company", COMPANY])
        .await;
    if ids.len() > 1 {
        assert_eq!(ambiguous.code, 2, "{}", ambiguous.stderr);
        assert!(ambiguous.stderr.contains("matches"), "{}", ambiguous.stderr);
    }
    let singleton = plain
        .run(&[
            "trial-balance",
            "retrieve",
            "--company",
            COMPANY,
            "--period",
            "2026-09",
        ])
        .await
        .expect(0)
        .json();
    assert!(singleton.is_object());
}

#[tokio::test]
async fn a_write_takes_the_short_id_and_names_its_object() {
    // Self-test P1-5: `oscar invoices issue KTHAGV0H` gave a 404 although `get` took it.
    // Self-test P2-16: the success line named the short id, not the number, and no amount.
    let (mock, user) = logged_in().await;
    let full = "01J90000000000000000000116";
    for (method, data) in [("issue", "{}"), ("cancel", r#"{"reason": "x"}"#)] {
        let run = user
            .run(&[
                "invoices",
                method,
                "00000116",
                "--company",
                COMPANY,
                "--data",
                data,
            ])
            .await;
        assert_eq!(run.code, 0, "{method}: {}", run.stderr);
        let path = format!("/api/companies/{COMPANY}/invoices/{full}/{method}");
        assert_eq!(
            mock.requests_to(&path).await.len(),
            1,
            "{method} went to the full id"
        );
        assert!(
            run.stderr.contains(" invoice number (0.01 EUR)"),
            "{}",
            run.stderr
        );
    }
    let run = user
        .run(&[
            "invoices",
            "issue",
            "01J9",
            "--company",
            COMPANY,
            "--data",
            "{}",
        ])
        .await;
    assert_eq!(run.code, 2, "an ambiguous prefix: {}", run.stderr);
}

#[tokio::test]
async fn a_write_prints_one_success_line() {
    let (_mock, mut user) = logged_in().await;
    let run = user
        .run(&[
            "invoices",
            "issue",
            "01J90000000000000000000116",
            "--company",
            COMPANY,
            "--data",
            "{}",
        ])
        .await;
    assert_eq!(run.code, 0, "{}", run.stderr);
    assert!(run.stdout.is_empty(), "{}", run.stdout);
    assert!(
        run.stderr.starts_with("✓ Issued invoice "),
        "{}",
        run.stderr
    );
    assert!(
        run.stderr.contains("/c/cmp0000001/invoices"),
        "{}",
        run.stderr
    );
    // `LC_ALL` wins over `LC_CTYPE` and `LANG`, which a CI runner may set to UTF-8.
    user.set("LC_ALL", "C");
    let run = user
        .run(&[
            "invoices",
            "cancel",
            "01J90000000000000000000116",
            "--company",
            COMPANY,
            "--data",
            r#"{"reason": "x"}"#,
        ])
        .await;
    assert!(
        run.stderr.starts_with("v Cancelled invoice"),
        "{}",
        run.stderr
    );
}
