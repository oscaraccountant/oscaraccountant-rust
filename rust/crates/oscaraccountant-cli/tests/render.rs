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
    ("employees_list", &["employees", "list"]),
    (
        "employee_submissions_list",
        &["employee-submissions", "list"],
    ),
    ("pay_runs_list", &["pay-runs", "list"]),
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
        "invoices_bank_accounts",
        &["invoices", "bank-accounts", "01J90000000000000000000116"],
    ),
    (
        "parties_get",
        &["parties", "get", "01J90000000000000000000179"],
    ),
    ("files_get", &["files", "get", "fil0000001"]),
    ("employees_get", &["employees", "get", "emp0000001"]),
    (
        "employee_submissions_get",
        &["employee-submissions", "get", "sub0000001"],
    ),
    ("pay_runs_get", &["pay-runs", "get", "pyr0000001"]),
    ("companies_get", &["companies", "get"]),
    ("me", &["me"]),
    ("inbox", &["inbox"]),
    ("tax_account_balance", &["tax-account", "balance"]),
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
        if !matches!(args[0], "companies" | "me") || (args[0] == "companies" && args[1] == "get") {
            args.extend(["--company", COMPANY]);
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
    // Regression: `oscar invoices issue KTHAGV0H` gave a 404 although `get` took it.
    // Regression: the success line named the short id, not the number, and no amount.
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

/// A bank transaction of the payments to explain, as a server with receipt states answers it.
fn payment(
    id: &str,
    date: &str,
    name: &str,
    total_minor: i64,
    state: &str,
    detail: Option<&str>,
) -> serde_json::Value {
    let candidates = if state == "invoice_not_matched" {
        serde_json::json!([{"invoice_id": "01J90000000000000000000116", "number": "INV-0042"}])
    } else {
        serde_json::json!([])
    };
    serde_json::json!({
        "id": id, "bank_account_id": "01J90000000000000000000001",
        "bank_account_label": "Main account", "currency": "EUR", "booking_date": date,
        "direction": "out", "amount_minor": total_minor, "fee_minor": 0,
        "total_minor": total_minor, "counterparty_name": name, "description": "Card payment",
        "status": "unmatched", "has_document": detail.is_some(), "file_ids": [],
        "receipt_state": state, "receipt_detail": detail, "match_proposal_id": null,
        "matched_invoice_candidates": candidates,
    })
}

/// The coverage of the payments to explain: one account with a gap month, one with no bank
/// lines, one bank feed account the books do not have.
fn coverage() -> serde_json::Value {
    serde_json::json!({
        "accounts": [
            {"bank_account_id": "01J90000000000000000000001", "name": "Main account",
             "iban": "EE001", "currency": "EUR", "transaction_count": 120,
             "first_booking_date": "2026-09-01", "last_booking_date": "2026-11-18",
             "booked_months": ["2026-09", "2026-11"], "gap_months": ["2026-10"]}
        ],
        "uncovered_accounts": [
            {"bank_account_id": "01J90000000000000000000002", "name": "Savings account",
             "iban": "EE003", "currency": "EUR", "reason": "no_bank_lines"},
            {"bank_account_id": null, "name": "Broker account", "iban": "EE004",
             "currency": "EUR", "reason": "feed_not_linked"}
        ],
        "checked_from": "2026-09-01",
        "checked_to": "2026-11-18"
    })
}

/// Script the two pages of the payments to explain, `times` times each.
async fn script_payments_to_explain(mock: &Mock, times: usize) {
    let first = serde_json::json!({
        "items": [
            payment("01J90000000000000000000801", "2026-09-04", "Example SMS Inc", -1896,
                    "invoice_not_matched", None),
            payment("01J90000000000000000000802", "2026-09-02", "Example Cloud Ltd", -4321,
                    "receipt_not_booked", Some("review")),
        ],
        "next_cursor": "c2",
        "coverage": coverage(),
    });
    let second = serde_json::json!({
        "items": [payment("01J90000000000000000000803", "2026-09-01", "Example Hosting OÜ",
                          -990, "missing", None)],
        "next_cursor": null,
        "coverage": coverage(),
    });
    let mut responses = Vec::new();
    for _ in 0..times {
        for page in [&first, &second] {
            responses.push(serde_json::json!({
                "match": {"operation_id": "books_bank_transactions_list"},
                "response": {"status": 200, "body": {"kind": "json", "value": page}},
            }));
        }
    }
    mock.control(
        "POST",
        "/_mock/responses",
        Some(serde_json::json!({ "responses": responses })),
    )
    .await;
}

#[tokio::test]
async fn the_payments_to_explain_with_their_receipt_and_coverage() {
    let (mock, mut user) = logged_in().await;
    let args = [
        "bank-transactions",
        "list",
        "--to-explain",
        "true",
        "--all",
        "--company",
        COMPANY,
    ];
    // A table at 80 and 120 columns: the receipt of each payment, then the coverage.
    script_payments_to_explain(&mock, 2).await;
    let mut text = String::new();
    for columns in ["80", "120"] {
        text.push_str(&format!("=== {columns} columns\n"));
        text.push_str(&shown(&mut user, &args, columns, false).await);
    }
    insta::assert_snapshot!("bank_transactions_to_explain", text, &args.join(" "));

    // `--json` with `--all`: every item, and the coverage of the first page.
    script_payments_to_explain(&mock, 1).await;
    let mut json_args = args.to_vec();
    json_args.push("--json");
    let page = user.run(&json_args).await.expect(0).json();
    assert_eq!(page["items"].as_array().map(Vec::len), Some(3), "{page}");
    assert_eq!(page["next_cursor"], serde_json::Value::Null);
    assert_eq!(page["coverage"], coverage());

    // NDJSON: one payment per line, no coverage line.
    script_payments_to_explain(&mock, 1).await;
    let mut ndjson_args = args.to_vec();
    ndjson_args.extend(["-o", "ndjson"]);
    let run = user.run(&ndjson_args).await.expect(0);
    let lines: Vec<&str> = run.stdout.lines().collect();
    assert_eq!(lines.len(), 3, "{}", run.stdout);
    assert!(
        lines.iter().all(|line| line.contains("\"receipt_state\"")),
        "{}",
        run.stdout
    );
}
