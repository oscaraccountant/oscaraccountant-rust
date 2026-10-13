//! The hand-written calls of `oscar onboard`.
//!
//! The onboarding endpoints are internal: they are not in the public schema, so the generated
//! table has none of them. Each call is declared once in [`CALLS`]: its method, its path, the
//! request fields it sends, and the response fields the CLI reads. The test `the_manifest_is_fresh`
//! writes the same list to `onboarding-calls.json`, and a test of the server repository checks that
//! file
//! against the internal schema of the server, so a change of the server breaks CI, not the users.

use oscaraccountant::{Client, HttpMethod};
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};
use serde_json::Value;
#[cfg(test)]
use serde_json::json;

use crate::error::{CliError, Result};

/// One hand-written call: what the contract test checks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CallSpec {
    /// A short name, for the messages of the contract test.
    pub name: &'static str,
    /// The HTTP method.
    pub method: &'static str,
    /// The path as the schema writes it, with `{company_id}` and the like.
    pub path: &'static str,
    /// The query parameters the CLI sends.
    pub query: &'static [&'static str],
    /// The fields of the JSON body the CLI sends.
    pub request: &'static [&'static str],
    /// The fields of the answer the CLI reads, as dotted paths; `[]` is an item of an array.
    pub response: &'static [&'static str],
}

/// The state of the onboarding of the signed-in person.
pub const ONBOARDING_STATE: CallSpec = CallSpec {
    name: "onboarding state",
    method: "GET",
    path: "/api/onboarding/",
    query: &[],
    request: &[],
    response: &[
        "current_step",
        "steps[].key",
        "steps[].status",
        "company.id",
        "company.name",
        "company.registry_code",
        "company.organization.name",
        "blocked_reason",
        "draft.registry_code",
    ],
};

/// Save the company step in progress: the pick, the books answers, the website.
pub const ONBOARDING_DRAFT: CallSpec = CallSpec {
    name: "save the company step",
    method: "PATCH",
    path: "/api/onboarding/",
    query: &[],
    request: &[
        "registry_code",
        "books_today",
        "books_no_reason",
        "books_software",
        "books_software_other",
        "website",
        "no_website",
    ],
    response: &[
        "draft.registry_code",
        "draft.books_today",
        "draft.website",
        "draft.prefill.registry_code",
        "draft.prefill.values.legal_name",
        "draft.prefill.values.address",
        "draft.prefill.legal_form_text",
        "draft.prefill.registration_date",
        "draft.prefill.status",
        "draft.prefill.status_text",
        "draft.prefill.values.legal_form",
        "draft.prefill.board_members[].name",
        "draft.prefill.shareholders[].name",
        "draft.prefill.shareholders[].share_percent",
        "draft.prefill.proposed_owners[].name",
        "draft.prefill.proposed_owners[].share_percent",
        "draft.prefill.legal_shareholders_to_explain[].name",
        "draft.prefill.ownership_chain.resolved",
        "draft.prefill.books_today_default",
        "draft.prefill.guard",
        "draft.beneficial_owners[].name",
    ],
};

/// Confirm the company step: the organization and the company are made.
pub const ONBOARDING_CONFIRM: CallSpec = CallSpec {
    name: "confirm the company",
    method: "POST",
    path: "/api/onboarding/confirm/",
    query: &[],
    request: &["beneficial_owners_confirmed"],
    response: &["company.id", "company.name", "current_step"],
};

/// Search the Business Register.
pub const REGISTER_SEARCH: CallSpec = CallSpec {
    name: "search the register",
    method: "GET",
    path: "/api/business-register/companies/",
    query: &["q", "limit"],
    request: &[],
    response: &[
        "results[].registry_code",
        "results[].name",
        "results[].legal_form",
        "results[].status",
        "results[].status_text",
        "results[].address",
    ],
};

/// The onboarding of one company.
pub const COMPANY_ONBOARDING: CallSpec = CallSpec {
    name: "company onboarding",
    method: "GET",
    path: "/api/companies/{company_id}/onboarding/",
    query: &[],
    request: &[],
    response: &[
        "id",
        "registry_code",
        "current_step",
        "steps[].key",
        "steps[].status",
        "steps[].minutes",
        "blocked_reason",
    ],
};

/// The questionnaire of step 2.
pub const QUESTIONNAIRE: CallSpec = CallSpec {
    name: "questionnaire",
    method: "GET",
    path: "/api/companies/{company_id}/compliance/questionnaire/",
    query: &[],
    request: &[],
    response: &[
        "activity_description",
        "customer_countries",
        "supplier_countries",
        "expected_turnover",
        "cash_usage",
        "uses_crypto_assets",
        "needs_licence",
        "licence_description",
        "residence",
        "residence_countries",
        "e_resident",
        "estonia_reason",
        "startup_funds_source",
        "acts_for_others",
        "acts_for_others_reason",
        "field_experience",
        "website",
        "no_website",
        "register_website",
        "asked.estonia_reason",
        "asked.startup_funds_source",
        "register_activity",
        "register_activity_names.code",
        "register_activity_names.en",
        "done",
        "submitted_at",
    ],
};

/// The fields of the questionnaire the CLI sends (step 2).
pub const QUESTIONNAIRE_FIELDS: &[&str] = &[
    "activity_description",
    "customer_countries",
    "supplier_countries",
    "expected_turnover",
    "cash_usage",
    "uses_crypto_assets",
    "needs_licence",
    "licence_description",
    "residence",
    "residence_countries",
    "e_resident",
    "estonia_reason",
    "startup_funds_source",
    "acts_for_others",
    "acts_for_others_reason",
    "field_experience",
];

/// The fields the CLI sends in step 2: the answers and the website, which step 2 asks again.
pub const QUESTIONNAIRE_REQUEST: &[&str] = &[
    "website",
    "no_website",
    "activity_description",
    "customer_countries",
    "supplier_countries",
    "expected_turnover",
    "cash_usage",
    "uses_crypto_assets",
    "needs_licence",
    "licence_description",
    "residence",
    "residence_countries",
    "e_resident",
    "estonia_reason",
    "startup_funds_source",
    "acts_for_others",
    "acts_for_others_reason",
    "field_experience",
];

/// Save some answers as a draft.
pub const QUESTIONNAIRE_SAVE: CallSpec = CallSpec {
    name: "save the questionnaire",
    method: "PATCH",
    path: "/api/companies/{company_id}/compliance/questionnaire/",
    query: &[],
    request: QUESTIONNAIRE_REQUEST,
    response: &["asked.estonia_reason", "asked.startup_funds_source"],
};

/// Submit the questionnaire.
pub const QUESTIONNAIRE_SUBMIT: CallSpec = CallSpec {
    name: "submit the questionnaire",
    method: "POST",
    path: "/api/companies/{company_id}/compliance/questionnaire/submit/",
    query: &[],
    request: QUESTIONNAIRE_REQUEST,
    response: &["done", "support_reason"],
};

/// The identity requirements of the company.
pub const KYC_REQUIREMENTS: CallSpec = CallSpec {
    name: "identity requirements",
    method: "GET",
    path: "/api/companies/{company_id}/kyc/requirements/",
    query: &["limit"],
    request: &[],
    response: &[
        "items[].id",
        "items[].person.first_name",
        "items[].person.last_name",
        "items[].role",
        "items[].role_ended",
        "items[].status",
        "items[].verification_status",
        "items[].can_start",
        "items[].pep_answered",
    ],
};

/// Where the caller stands in step 3: the call of the web page (`compliance/identity/`).
pub const IDENTITY_STEP: CallSpec = CallSpec {
    name: "identity step",
    method: "GET",
    path: "/api/companies/{company_id}/compliance/identity/",
    query: &[],
    request: &[],
    response: &[
        "state",
        "pep_answered",
        "requirement_id",
        "board_member",
        "done",
    ],
};

/// Step 3 as the web page starts it: the consent, the PEP answer, then the Didit session of the
/// caller. The server makes the caller's requirement when there is none yet.
pub const IDENTITY_START: CallSpec = CallSpec {
    name: "start the identity check",
    method: "POST",
    path: "/api/companies/{company_id}/compliance/identity/start/",
    query: &[],
    request: &[
        "language",
        "consent",
        "is_pep",
        "is_pep_related",
        "pep_details",
    ],
    response: &["url", "verification_id"],
};

/// A link to send to another person.
pub const KYC_LINK: CallSpec = CallSpec {
    name: "invite link",
    method: "POST",
    path: "/api/companies/{company_id}/kyc/requirements/{requirement_id}/link/",
    query: &[],
    request: &[],
    response: &["url", "expires_at"],
};

/// The client agreement in one language.
pub const AGREEMENT: CallSpec = CallSpec {
    name: "client agreement",
    method: "GET",
    path: "/api/companies/{company_id}/onboarding/agreement/",
    query: &["language"],
    request: &[],
    response: &[
        "version",
        "language",
        "text",
        "text_sha256",
        "acceptance.version",
        "acceptance.language",
        "acceptance.accepted_at",
        "acceptance.person_name",
        "can_accept",
        "problem",
    ],
};

/// Accept the client agreement.
pub const AGREEMENT_ACCEPT: CallSpec = CallSpec {
    name: "accept the client agreement",
    method: "POST",
    path: "/api/companies/{company_id}/onboarding/agreement/accept/",
    query: &[],
    request: &["version", "language", "text_sha256"],
    response: &[
        "acceptance.version",
        "acceptance.language",
        "acceptance.accepted_at",
        "acceptance.channel",
    ],
};

/// The evidence PDF of the acceptance.
pub const AGREEMENT_DOCUMENT: CallSpec = CallSpec {
    name: "agreement evidence PDF",
    method: "GET",
    path: "/api/companies/{company_id}/onboarding/agreement/document/",
    query: &[],
    request: &[],
    response: &[],
};

/// The billing of the company: the prices.
pub const BILLING: CallSpec = CallSpec {
    name: "billing",
    method: "GET",
    path: "/api/companies/{company_id}/billing/",
    query: &[],
    request: &[],
    response: &[
        "service_status",
        "subscription_active",
        "prices[].plan",
        "prices[].amount_minor",
        "prices[].total_minor",
        "prices[].currency",
    ],
};

/// Start Checkout of a plan.
pub const CHECKOUT: CallSpec = CallSpec {
    name: "start checkout",
    method: "POST",
    path: "/api/companies/{company_id}/billing/checkout/",
    query: &[],
    request: &["plan", "return_path"],
    response: &["checkout_id", "checkout_url"],
};

/// The result of a Checkout.
pub const CHECKOUT_RESULT: CallSpec = CallSpec {
    name: "checkout result",
    method: "GET",
    path: "/api/companies/{company_id}/billing/checkout/{checkout_id}/",
    query: &[],
    request: &[],
    response: &["status", "service_status", "subscription_active"],
};

/// The setup checklist.
pub const SETUP: CallSpec = CallSpec {
    name: "setup checklist",
    method: "GET",
    path: "/api/companies/{company_id}/setup/",
    query: &[],
    request: &[],
    response: &[
        "items[].key",
        "items[].requirement",
        "items[].status",
        "items[].deadline",
        "complete",
    ],
};

/// The caller: the oldest CLI that may run the onboarding.
pub const ME: CallSpec = CallSpec {
    name: "me",
    method: "GET",
    path: "/api/me/",
    query: &[],
    request: &[],
    response: &[
        "user.email",
        "organizations[].id",
        "organizations[].name",
        "companies[].id",
        "companies[].name",
        "cli_min_version",
    ],
};

/// Every hand-written call of the CLI: the manifest of the contract test.
#[cfg(test)]
pub const CALLS: &[CallSpec] = &[
    ME,
    ONBOARDING_STATE,
    ONBOARDING_DRAFT,
    ONBOARDING_CONFIRM,
    REGISTER_SEARCH,
    COMPANY_ONBOARDING,
    QUESTIONNAIRE,
    QUESTIONNAIRE_SAVE,
    QUESTIONNAIRE_SUBMIT,
    KYC_REQUIREMENTS,
    IDENTITY_STEP,
    IDENTITY_START,
    KYC_LINK,
    AGREEMENT,
    AGREEMENT_ACCEPT,
    AGREEMENT_DOCUMENT,
    BILLING,
    CHECKOUT,
    CHECKOUT_RESULT,
    SETUP,
];

/// The manifest of [`CALLS`], as the contract test of the server repository reads it.
#[cfg(test)]
pub fn manifest() -> Value {
    let calls: Vec<Value> = CALLS
        .iter()
        .map(|call| {
            json!({
                "name": call.name,
                "method": call.method,
                "path": call.path,
                "query": call.query,
                "request": call.request,
                "response": call.response,
            })
        })
        .collect();
    json!({
        "description": "The hand-written calls of `oscar onboard`. Written by the test `the_manifest_is_fresh` of the CLI crate; checked against the internal schema of the server.",
        "calls": calls,
    })
}

/// A path segment, percent-encoded.
const SEGMENT: &AsciiSet = &NON_ALPHANUMERIC.remove(b'-').remove(b'_').remove(b'.');

/// The path of `spec` with its parameters, each percent-encoded.
pub fn path(spec: &CallSpec, params: &[(&str, &str)]) -> String {
    let mut path = spec.path.to_owned();
    for (name, value) in params {
        let encoded = utf8_percent_encode(value, SEGMENT).to_string();
        path = path.replace(&format!("{{{name}}}"), &encoded);
    }
    debug_assert!(
        !path.contains('{'),
        "a parameter of {} is missing",
        spec.path
    );
    path
}

/// The method of `spec`.
fn method(spec: &CallSpec) -> HttpMethod {
    match spec.method {
        "POST" => HttpMethod::Post,
        "PATCH" => HttpMethod::Patch,
        "PUT" => HttpMethod::Put,
        "DELETE" => HttpMethod::Delete,
        _ => HttpMethod::Get,
    }
}

/// The calls of the onboarding, with the client of the run.
pub struct Api<'a> {
    client: &'a Client,
}

impl<'a> Api<'a> {
    /// The calls through `client`.
    pub fn new(client: &'a Client) -> Self {
        Self { client }
    }

    /// Send `spec` with these path parameters, query and body, and read the JSON answer.
    pub async fn call(
        &self,
        spec: &CallSpec,
        params: &[(&str, &str)],
        query: &[(&str, &str)],
        body: Option<&Value>,
    ) -> Result<Value> {
        debug_assert!(
            query.iter().all(|(name, _)| spec.query.contains(name)),
            "a query parameter of {} is not declared",
            spec.name
        );
        if let Some(Value::Object(fields)) = body {
            debug_assert!(
                fields
                    .keys()
                    .all(|key| spec.request.contains(&key.as_str())),
                "a request field of {} is not declared",
                spec.name
            );
        }
        let mut request = self.client.request(method(spec), path(spec, params));
        for (name, value) in query {
            request = request.query(*name, *value);
        }
        if let Some(body) = body {
            request = request.json(body)?;
        }
        let response = request.send().await.map_err(explain)?;
        if response.body.is_empty() {
            return Ok(Value::Null);
        }
        Ok(response.json()?)
    }

    /// `GET` of `spec` with the company.
    pub async fn get(&self, spec: &CallSpec, company: &str) -> Result<Value> {
        self.call(spec, &[("company_id", company)], &[], None).await
    }

    /// The evidence PDF of the acceptance.
    pub async fn agreement_pdf(&self, company: &str) -> Result<bytes::Bytes> {
        let response = self
            .client
            .request(
                method(&AGREEMENT_DOCUMENT),
                path(&AGREEMENT_DOCUMENT, &[("company_id", company)]),
            )
            .accept("application/pdf")
            .send()
            .await
            .map_err(explain)?;
        Ok(response.body)
    }
}

/// The base of the web app for a link: the base URL of the API.
pub fn web_link(base_url: &str, path: &str) -> String {
    format!("{}{path}", base_url.trim_end_matches('/'))
}

/// The hint of a `pre_launch` refusal.
pub const PRE_LAUNCH_HINT: &str = "Oscar is not open to new clients yet. If you have an invite link, a referral link or a partner code, open it in the browser first, then run `oscar onboard` again.";

/// The error of a call, with the hint of the onboarding for the codes a person can act on.
pub fn explain(error: oscaraccountant::Error) -> CliError {
    let code = error.code().map(str::to_owned);
    let mut cli = CliError::from(error);
    match code.as_deref() {
        Some("pre_launch") => cli.details.push(PRE_LAUNCH_HINT.to_owned()),
        Some("onboarding_needs_support") => cli
            .details
            .push("This needs our support team: write to us from the onboarding page.".to_owned()),
        Some("session_required") => cli.details.push(
            "This step needs a login of the `oscar` CLI: run `oscar login` (not an API key)."
                .to_owned(),
        ),
        _ => {}
    }
    cli
}

/// The code of an API error, if the error is one.
pub fn code(error: &CliError) -> Option<String> {
    let body: Value = serde_json::from_str(error.body.as_deref()?).ok()?;
    body.get("code")?.as_str().map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The manifest file next to `Cargo.toml`. `OSCAR_UPDATE_MANIFEST=1` writes it.
    #[test]
    fn the_manifest_is_fresh() {
        let file = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("onboarding-calls.json");
        let mut text = serde_json::to_string_pretty(&manifest()).unwrap();
        text.push('\n');
        if std::env::var_os("OSCAR_UPDATE_MANIFEST").is_some() {
            std::fs::write(&file, &text).unwrap();
        }
        let committed = std::fs::read_to_string(&file).unwrap_or_default();
        assert_eq!(
            committed.replace("\r\n", "\n"),
            text,
            "onboarding-calls.json is stale: run `OSCAR_UPDATE_MANIFEST=1 cargo test -p oscaraccountant-cli manifest`"
        );
    }

    #[test]
    fn every_call_is_declared_once() {
        let mut keys: Vec<(&str, &str)> = CALLS.iter().map(|c| (c.method, c.path)).collect();
        keys.sort_unstable();
        let before = keys.len();
        keys.dedup();
        assert_eq!(before, keys.len());
        for call in CALLS {
            assert!(call.path.starts_with("/api/") && call.path.ends_with('/'));
            assert!(["GET", "POST", "PATCH"].contains(&call.method));
        }
    }

    #[test]
    fn builds_the_path() {
        assert_eq!(
            path(
                &KYC_LINK,
                &[("company_id", "c1"), ("requirement_id", "r/1")]
            ),
            "/api/companies/c1/kyc/requirements/r%2F1/link/"
        );
        assert_eq!(method(&IDENTITY_START), HttpMethod::Post);
        assert_eq!(method(&ONBOARDING_DRAFT), HttpMethod::Patch);
        assert_eq!(method(&SETUP), HttpMethod::Get);
        assert_eq!(
            web_link("https://x.test/", "/onboarding"),
            "https://x.test/onboarding"
        );
    }

    fn api_error(status: u16, body: &str) -> oscaraccountant::Error {
        oscaraccountant::Error::from_api(oscaraccountant::ApiError::from_response(
            status,
            None,
            body.as_bytes(),
        ))
    }

    #[test]
    fn explains_the_codes() {
        let error = explain(api_error(
            403,
            r#"{"code": "pre_launch", "detail": "Not open."}"#,
        ));
        assert_eq!(error.details, [PRE_LAUNCH_HINT]);
        assert_eq!(code(&error).as_deref(), Some("pre_launch"));
        let error = explain(api_error(409, r#"{"code": "onboarding_needs_support"}"#));
        assert!(error.details[0].contains("support"));
        let error = explain(api_error(403, r#"{"code": "session_required"}"#));
        assert!(error.details[0].contains("oscar login"));
        let error = explain(api_error(404, r#"{"code": "not_found"}"#));
        assert!(error.details.is_empty());
        assert_eq!(code(&CliError::failed("x")), None);
    }
}
