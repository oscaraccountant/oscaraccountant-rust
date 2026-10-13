//! `oscar onboard`: the onboarding of a company in the terminal.
//! The steps and their order are the server's:
//! the CLI reads the state of the company and starts at the first step that is not done.
//!
//! | Step | Where |
//! |---|---|
//! | 0. Account | The browser: `oscar login`, or the web sign-up for a new person |
//! | 1. Company | The terminal: the Business Register search, the prefill, the confirmation |
//! | 2. About your business | The terminal: the questionnaire, a review, the submission |
//! | 3. Identity | Didit on the phone or in the browser; the CLI waits |
//! | 4. Agreement | The terminal: the pager, then `I accept` |
//! | 5. Plan | The terminal, then Checkout in the browser; the CLI waits |
//! | 6. Set up | The checklist, with the links of the web app |

pub mod api;
pub mod markdown;
pub mod pager;
pub mod questions;

use std::time::{Duration, Instant};

use oscaraccountant::Client;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use self::api::{Api, web_link};
use self::pager::{Document, Outcome, Pager};
use self::questions::{Kind, QUESTIONS};
use crate::browser;
use crate::cli;
use crate::commands::login;
use crate::error::{CliError, EXIT_AUTH, Result};
use crate::prompt::{Prompter, TextOptions};
use crate::render::{self, theme};
use crate::session::{App, Selection, VERSION};

/// The steps of the onboarding: the key of the server and the name the web app shows.
pub const STEPS: &[(&str, &str)] = &[
    ("account", "Create your account"),
    ("company", "Your company"),
    ("business", "About your business"),
    ("identity", "Verify your identity"),
    ("agreement", "Accept the client agreement"),
    ("plan", "Choose your plan"),
    ("setup", "Set up"),
];

/// The test switch of the time between two reads of a state that a person changes elsewhere.
pub const POLL_INTERVAL: &str = "OSCAR_POLL_INTERVAL_MS";

/// The test switch of the longest wait for such a state.
pub const POLL_TIMEOUT: &str = "OSCAR_POLL_TIMEOUT_SECS";

/// The answers of the flags of `oscar onboard`.
#[derive(Debug, Clone, Default)]
pub struct Options {
    /// `-c/--company`.
    pub company: Option<String>,
    /// `--registry-code`.
    pub registry_code: Option<String>,
    /// `--books-today yes|no`.
    pub books_today: Option<String>,
    /// `--books-no-reason`.
    pub books_no_reason: Option<String>,
    /// `--books-software`.
    pub books_software: Option<String>,
    /// `--books-software-other`.
    pub books_software_other: Option<String>,
    /// `--website`.
    pub website: Option<String>,
    /// `--no-website`.
    pub no_website: bool,
    /// `--confirm-owners`.
    pub confirm_owners: bool,
    /// `--answers <file.json>` and `--answer key=value`, merged.
    pub answers: Map<String, Value>,
    /// `--consent`.
    pub consent: bool,
    /// `--pep yes|no`.
    pub pep: Option<bool>,
    /// `--pep-related yes|no`.
    pub pep_related: Option<bool>,
    /// `--pep-details`.
    pub pep_details: Option<String>,
    /// `--invite-links`.
    pub invite_links: bool,
    /// `--language en|et`.
    pub language: Option<String>,
    /// `--plan month|year`.
    pub plan: Option<String>,
    /// `--no-wait`.
    pub no_wait: bool,
}

/// The books software of the company step, with its label.
pub const BOOKS_SOFTWARE: &[(&str, &str)] = &[
    ("merit_aktiva", "Merit Aktiva"),
    ("xolo", "Xolo"),
    ("directo", "Directo"),
    ("other", "Other software"),
    ("my_accountant", "My accountant"),
];

/// A version `x.y.z` as numbers; a pre-release or build suffix is ignored.
pub fn parse_version(text: &str) -> Option<(u64, u64, u64)> {
    let core = text.trim().trim_start_matches('v');
    let core = core.split(['-', '+']).next()?;
    let mut parts = core.split('.').map(|part| part.parse::<u64>().ok());
    let version = (
        parts.next()??,
        parts.next().unwrap_or(Some(0))?,
        parts.next().unwrap_or(Some(0))?,
    );
    Some(version)
}

/// Refuse an older CLI than the server's `cli_min_version`.
pub fn check_version(me: &Value, current: &str) -> Result<()> {
    let Some(minimum) = me.get("cli_min_version").and_then(Value::as_str) else {
        return Ok(());
    };
    match (parse_version(minimum), parse_version(current)) {
        (Some(minimum_parts), Some(current_parts)) if current_parts < minimum_parts => {
            Err(CliError::failed(format!(
                "This oscar is version {current}; the onboarding needs {minimum} or later. Update it first: npx @oscaraccountant/cli@latest onboard"
            )))
        }
        _ => Ok(()),
    }
}

/// How to onboard another company than the one the CLI found.
pub const NEW_COMPANY_HINT: &str =
    "To onboard a new company: `oscar onboard --registry-code <code>`.";

/// The line of a company that came through no onboarding (made before the flow, or by the firm).
pub fn no_onboarding(name: &str, company: &str) -> String {
    format!("{name} ({company}) needs no onboarding: it is set up already.")
}

/// The name of a step.
pub fn step_name(key: &str) -> &str {
    STEPS
        .iter()
        .find(|(name, _)| *name == key)
        .map_or(key, |(_, label)| label)
}

/// The number of a step, 0 to 6.
fn step_number(key: &str) -> usize {
    STEPS.iter().position(|(name, _)| *name == key).unwrap_or(0)
}

/// The lines of `oscar onboard status`: one per step, with its status.
pub fn status_lines(state: &Value, theme: &theme::Theme) -> Vec<String> {
    let mut lines = Vec::new();
    let steps = state["steps"].as_array().cloned().unwrap_or_default();
    for step in &steps {
        let key = step["key"].as_str().unwrap_or_default();
        let status = step["status"].as_str().unwrap_or_default();
        let (symbol, role) = match status {
            "done" => (theme.symbol(theme::Sym::Check), theme::Role::Ok),
            "current" => (theme.symbol(theme::Sym::Arrow), theme::Role::Accent),
            _ => (theme.symbol(theme::Sym::Dot), theme::Role::Muted),
        };
        // The minutes are the estimate of a step to do: next to `done` they would read as "ago".
        let minutes = step["minutes"]
            .as_u64()
            .filter(|_| status != "done")
            .map(|minutes| format!("· about {minutes} min"))
            .unwrap_or_default();
        let optional = if step["optional"].as_bool() == Some(true) {
            ", any time"
        } else {
            ""
        };
        lines.push(format!(
            "{} {}. {:<30} {}",
            theme.paint(role, symbol),
            step_number(key),
            step_name(key),
            theme.paint(
                theme::Role::Dim,
                format!("{status} {minutes}{optional}").trim()
            ),
        ));
    }
    if let Some(reason) = state["blocked_reason"].as_str().filter(|r| !r.is_empty()) {
        lines.push(format!(
            "{} The onboarding needs our support team: {reason}.",
            theme.paint(theme::Role::Bad, theme.symbol(theme::Sym::Bang))
        ));
    }
    lines
}

/// An amount in minor units, as `49.00 EUR`.
pub fn money(minor: i64, currency: &str) -> String {
    let sign = if minor < 0 { "-" } else { "" };
    let minor = minor.unsigned_abs();
    format!(
        "{sign}{}.{:02} {}",
        minor / 100,
        minor % 100,
        currency.to_uppercase()
    )
}

/// The SHA-256 of a text, in hex.
pub fn sha256_hex(text: &str) -> String {
    Sha256::digest(text.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// A QR code of `url` for a terminal, in half blocks; `None` when the URL is too long.
pub fn qr_code(url: &str) -> Option<String> {
    use qrcode::render::unicode::Dense1x2;
    let code = qrcode::QrCode::new(url.as_bytes()).ok()?;
    Some(
        code.render::<Dense1x2>()
            .dark_color(Dense1x2::Light)
            .light_color(Dense1x2::Dark)
            .quiet_zone(true)
            .build(),
    )
}

/// The name of a person of a requirement.
fn person_name(requirement: &Value) -> String {
    let first = requirement["person"]["first_name"]
        .as_str()
        .unwrap_or_default();
    let last = requirement["person"]["last_name"]
        .as_str()
        .unwrap_or_default();
    let name = format!("{first} {last}");
    let name = name.trim();
    if name.is_empty() {
        "A person".to_owned()
    } else {
        name.to_owned()
    }
}

/// A role as a person reads it.
fn role_name(role: &str) -> String {
    role.replace('_', " ")
}

/// A choice of a prompt: the value and its label.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Choice {
    value: String,
    label: String,
}

impl std::fmt::Display for Choice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.label)
    }
}

fn choices(values: &[(&str, &str)]) -> Vec<Choice> {
    values
        .iter()
        .map(|(value, label)| Choice {
            value: (*value).to_owned(),
            label: (*label).to_owned(),
        })
        .collect()
}

/// A company of a search of the Business Register.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Hit {
    code: String,
    line: String,
}

impl std::fmt::Display for Hit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.line)
    }
}

/// The register status of a company in English (the register writes it in Estonian).
pub fn status_name(code: &str) -> Option<&'static str> {
    match code {
        "R" => Some("Registered"),
        "L" => Some("In liquidation"),
        "N" => Some("Bankrupt"),
        "K" => Some("Deleted"),
        _ => None,
    }
}

/// A legal form of the server (`ou`, `as`...) as a person reads it in English.
pub fn legal_form_name(form: &str) -> Option<&'static str> {
    match form {
        "ou" => Some("OÜ (private limited company)"),
        "as" => Some("AS (public limited company)"),
        "fie" => Some("FIE (sole proprietor)"),
        "mtu" => Some("MTÜ (non-profit association)"),
        "sa" => Some("SA (foundation)"),
        "tu" => Some("TÜ (general partnership)"),
        "uu" => Some("UÜ (limited partnership)"),
        _ => None,
    }
}

/// The longest line of a search hit: a picker row must not wrap.
const HIT_WIDTH: usize = 76;

/// `text` cut to `width` characters, with an ellipsis.
fn shorten(text: &str, width: usize) -> String {
    if text.chars().count() <= width {
        return text.to_owned();
    }
    let mut cut: String = text.chars().take(width.saturating_sub(1)).collect();
    cut.push('…');
    cut
}

/// The companies of a search answer.
fn hits(answer: &Value) -> Vec<Hit> {
    answer["results"]
        .as_array()
        .map(|results| {
            results
                .iter()
                .filter_map(|result| {
                    let code = match &result["registry_code"] {
                        Value::Number(number) => number.to_string(),
                        Value::String(text) => text.clone(),
                        _ => return None,
                    };
                    let mut parts = vec![
                        result["name"].as_str().unwrap_or_default().to_owned(),
                        code.clone(),
                    ];
                    // Only a status that is not the usual one says something in a picker row.
                    if let Some(status) = result["status"].as_str().and_then(status_name)
                        && status != "Registered"
                    {
                        parts.push(status.to_owned());
                    }
                    let head = parts.join(" · ");
                    let mut line = head.clone();
                    if let Some(address) = result["address"].as_str().filter(|t| !t.is_empty()) {
                        let room = HIT_WIDTH.saturating_sub(head.chars().count() + 3);
                        if room >= 8 {
                            line = format!("{head} · {}", shorten(address, room));
                        }
                    }
                    Some(Hit { code, line })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// A registry code: 8 digits.
pub fn is_registry_code(text: &str) -> bool {
    let text = text.trim();
    text.len() == 8 && text.chars().all(|ch| ch.is_ascii_digit())
}

/// The onboarding the person started and has not finished, from `GET /api/onboarding/`.
#[derive(Debug, Clone)]
pub struct OpenOnboarding {
    /// The id of its company.
    pub id: String,
    /// The name of its company.
    pub name: String,
    /// The registry code of its company, when the server gives it.
    pub registry_code: Option<String>,
    /// The name of the organization of its company, when the server gives it.
    pub organization: Option<String>,
    /// False: the company is in another organization than the login's, so its routes answer 404
    /// to this login (a token of the CLI is bound to one organization).
    pub reachable: bool,
    /// The name of the organization of the login, when it has one.
    pub login_organization: Option<String>,
    /// The state of `GET /api/onboarding/`: the steps and the current one.
    pub state: Value,
}

impl OpenOnboarding {
    /// Read from the state of the person and `GET /api/me/` of the login; `None` when no
    /// company onboarding is open.
    pub fn read(state: Value, me: &Value) -> Option<Self> {
        let company = &state["company"];
        let id = company["id"].as_str()?.to_owned();
        let reachable = me["companies"]
            .as_array()
            .is_some_and(|items| items.iter().any(|item| item["id"].as_str() == Some(&id)));
        let text = |value: &Value| value.as_str().filter(|t| !t.is_empty()).map(str::to_owned);
        Some(Self {
            name: text(&company["name"]).unwrap_or_else(|| "your company".to_owned()),
            registry_code: text(&company["registry_code"]),
            organization: text(&company["organization"]["name"]),
            reachable,
            login_organization: organization_of(me),
            state,
            id,
        })
    }

    /// The organization to choose on the consent page: its own name, else the company's.
    pub fn organization_name(&self) -> &str {
        self.organization.as_deref().unwrap_or(&self.name)
    }

    /// `the onboarding of X is open in organization Y`, and the organization of this login.
    pub fn elsewhere_line(&self) -> String {
        let login = self
            .login_organization
            .as_deref()
            .map(|name| format!(", and this login is for \"{name}\""))
            .unwrap_or_default();
        format!(
            "The onboarding of {} ({}) is open in the organization \"{}\"{login}.",
            self.name,
            self.id,
            self.organization_name()
        )
    }

    /// What to run to reach it by hand.
    pub fn login_hint(&self) -> String {
        format!(
            "Run `oscar login`, choose the organization \"{}\" on the consent page, then `oscar onboard`.",
            self.organization_name()
        )
    }

    /// The step it stands at, as `step 3 (Verify your identity)`.
    pub fn step_text(&self) -> String {
        let current = self.state["current_step"].as_str().unwrap_or_default();
        if STEPS.iter().any(|(key, _)| *key == current) {
            format!("step {} ({})", step_number(current), step_name(current))
        } else {
            "an open step".to_owned()
        }
    }

    /// `--registry-code <code>` names another company than this one.
    pub fn is_other_code(&self, code: &str) -> bool {
        self.registry_code
            .as_deref()
            .is_none_or(|own| own != code.trim())
    }

    /// The line after "set up already": a new company starts only once this one is done, as the
    /// server allows one open onboarding at a time (409 `onboarding_company_exists`).
    pub fn new_company_hint(&self) -> String {
        format!(
            "The onboarding of {} is still open: finish it first with `oscar onboard`. Another company can start after that.",
            self.name
        )
    }
}

/// One run of the onboarding.
struct Flow<'a> {
    app: &'a mut App,
    prompter: Prompter,
    options: Options,
    client: Client,
    base_url: String,
    company_name: String,
    language: Option<String>,
    poll: Duration,
    limit: Duration,
}

/// `--registry-code` starts a new company and `-c/--company` names an existing one: never both.
fn refuse_company_and_code(options: &Options) -> Result<()> {
    if options.registry_code.is_some() && options.company.is_some() {
        return Err(CliError::usage(
            "--registry-code onboards a new company and -c/--company names an existing one: give one of them.",
        ));
    }
    Ok(())
}

/// `oscar onboard`: run every step that is not done. `--registry-code` wins over `OSCAR_COMPANY`
/// and the default company of the profile: it onboards a new company.
pub async fn onboard(app: &mut App, options: Options) -> Result<()> {
    refuse_company_and_code(&options)?;
    let mut flow = Flow::start(app, options).await?;
    let selection = flow.selection().await?;
    let default = selection.profile.as_ref().and_then(|p| p.company.clone());
    let wanted = if flow.options.registry_code.is_some() {
        None
    } else {
        flow.wanted_company(selection).await?
    };
    let company = match wanted {
        Some(company) => {
            flow.has_onboarding(&company).await?;
            company
        }
        None => match flow.open_onboarding().await? {
            Some(open) => {
                flow.one_open_onboarding(&open)?;
                flow.reach_open(&open).await?;
                open.id
            }
            None => match flow.resumable(default).await? {
                Some((company, state)) if !flow.new_after_done(&company, &state).await? => company,
                _ => flow.company_step().await?,
            },
        },
    };
    flow.steps(&company).await
}

/// `oscar onboard status`: the steps and where the onboarding stands.
pub async fn status(app: &mut App, options: Options) -> Result<()> {
    let mut flow = Flow::start(app, options).await?;
    let selection = flow.selection().await?;
    let default = selection.profile.as_ref().and_then(|p| p.company.clone());
    let company = match flow.wanted_company(selection).await? {
        Some(company) => Some(company),
        None => match flow.open_onboarding().await? {
            Some(open) if !open.reachable => {
                // The routes of the company answer 404 to this login: show the person's state,
                // never "set up already" (N3-P1-1).
                return flow.status_elsewhere(&open);
            }
            Some(open) => Some(open.id),
            None => default,
        },
    };
    let api = Api::new(&flow.client);
    let state = match &company {
        Some(company) => match api.get(&api::COMPANY_ONBOARDING, company).await {
            Ok(state) => state,
            Err(error) if api::code(&error).as_deref() == Some("not_found") => {
                if flow.app.json() {
                    flow.app
                        .out
                        .json(&json!({"company": company, "onboarding": null}));
                    return Ok(());
                }
                flow.set_company_name(company).await;
                let line = no_onboarding(&flow.company_name, company);
                let hint = flow.new_company_hint().await;
                flow.app.out.line(&line);
                flow.app.out.line(&hint);
                return Ok(());
            }
            Err(error) => return Err(error),
        },
        None => api.call(&api::ONBOARDING_STATE, &[], &[], None).await?,
    };
    if flow.app.json() {
        flow.app.out.json(&state);
        return Ok(());
    }
    if let Some(company) = &company {
        flow.set_company_name(company).await;
        flow.app.out.line(&format!(
            "{} ({company})",
            flow.app.out.theme.bold(&flow.company_name)
        ));
    } else {
        flow.app
            .out
            .line("No company yet: `oscar onboard` starts with your company.");
    }
    let theme = flow.app.out.theme;
    for line in status_lines(&state, &theme) {
        flow.app.out.line(&line);
    }
    Ok(())
}

/// `oscar onboard agreement`: the pager and the acceptance, or with `download` the evidence PDF,
/// which replaces an existing file only with `force`.
pub async fn agreement(
    app: &mut App,
    options: Options,
    download: Option<String>,
    force: bool,
) -> Result<()> {
    let mut flow = Flow::start(app, options).await?;
    let selection = flow.selection().await?;
    let default = selection.profile.as_ref().and_then(|p| p.company.clone());
    let company = match flow.wanted_company(selection).await? {
        Some(company) => company,
        None => match flow.open_onboarding().await? {
            Some(open) => {
                flow.reach_open(&open).await?;
                open.id
            }
            None => default
                .ok_or_else(|| CliError::usage("No company. Give -c/--company <company>."))?,
        },
    };
    flow.set_company_name(&company).await;
    match download {
        Some(path) => flow.download(&company, &path, force).await,
        None => {
            flow.steps_before_agreement(&company).await?;
            flow.agreement(&company).await?;
            Ok(())
        }
    }
}

impl<'a> Flow<'a> {
    /// Log in when needed, read `/api/me/`, and check the version of the CLI.
    async fn start(app: &'a mut App, options: Options) -> Result<Self> {
        let prompter = Prompter::for_app(app);
        let poll = app
            .env
            .var(POLL_INTERVAL)
            .and_then(|value| value.parse::<u64>().ok())
            .map_or(Duration::from_secs(3), Duration::from_millis);
        let limit = app
            .env
            .var(POLL_TIMEOUT)
            .and_then(|value| value.parse::<u64>().ok())
            .map_or(Duration::from_secs(30 * 60), Duration::from_secs);
        let (client, me) = match connect(app).await {
            Ok(found) => found,
            Err(error) if error.code == EXIT_AUTH => {
                first_login(app, &prompter, &error).await?;
                connect(app).await?
            }
            Err(error) => return Err(error),
        };
        check_version(&me, VERSION)?;
        let base_url = client.base_url().to_owned();
        let language = options.language.clone();
        Ok(Self {
            app,
            prompter,
            options,
            client,
            base_url,
            company_name: String::new(),
            language,
            poll,
            limit,
        })
    }

    fn api(&self) -> Api<'_> {
        Api::new(&self.client)
    }

    async fn selection(&self) -> Result<Selection> {
        self.app.select(&self.app.config()?)
    }

    /// `-c/--company` or `OSCAR_COMPANY`, resolved.
    async fn wanted_company(&self, selection: Selection) -> Result<Option<String>> {
        let selection = self
            .app
            .with_company(selection, self.options.company.as_deref())
            .await?;
        Ok(selection.company)
    }

    /// The default company of the profile and its onboarding state, when its onboarding can go
    /// on (also when only the setup checklist is left). `None` for a new company:
    /// `--registry-code` is given, the profile has no company, or the company came through no
    /// onboarding (said once).
    async fn resumable(&mut self, default: Option<String>) -> Result<Option<(String, Value)>> {
        if self.options.registry_code.is_some() {
            return Ok(None);
        }
        let Some(company) = default else {
            return Ok(None);
        };
        match self.api().get(&api::COMPANY_ONBOARDING, &company).await {
            Ok(state) => Ok(Some((company, state))),
            Err(error) if api::code(&error).as_deref() == Some("not_found") => {
                self.set_company_name(&company).await;
                let line = no_onboarding(&self.company_name, &company);
                self.note(&format!("{line} This onboards a new company."));
                self.company_name.clear();
                Ok(None)
            }
            Err(error) => Err(error),
        }
    }

    /// The onboarding of the default company is done and no other one is open: on a terminal,
    /// ask whether to onboard a new company (as "New organization" of the web app) or to see
    /// the setup checklist of this one. True for a new company. With no terminal the checklist
    /// shows, and its last line names `--registry-code`.
    async fn new_after_done(&mut self, company: &str, state: &Value) -> Result<bool> {
        if state["current_step"].as_str() != Some("done") || !self.prompter.interactive() {
            return Ok(false);
        }
        self.set_company_name(company).await;
        let name = self.company_name.clone();
        self.note(&format!(
            "{} The onboarding of {name} is done.",
            theme::symbol(theme::Sym::Check)
        ));
        let options = choices(&[
            ("new", "Onboard a new company"),
            ("setup", &format!("See the setup checklist of {name}")),
        ]);
        let choice = self
            .prompter
            .select("What do you want to do?", options, "--registry-code <code>")
            .await?;
        if choice.value != "new" {
            return Ok(false);
        }
        self.company_name.clear();
        Ok(true)
    }

    /// A company named with `-c/--company` must have an onboarding: a company that is set up
    /// already (made before the flow, or by the firm) is a usage error that says so (P2-22).
    async fn has_onboarding(&mut self, company: &str) -> Result<()> {
        match self.api().get(&api::COMPANY_ONBOARDING, company).await {
            Ok(_) => Ok(()),
            Err(error) if api::code(&error).as_deref() == Some("not_found") => {
                self.set_company_name(company).await;
                let mut error = CliError::usage(no_onboarding(&self.company_name, company));
                error.details.push(self.new_company_hint().await);
                Err(error)
            }
            Err(error) => Err(error),
        }
    }

    /// `oscar onboard agreement` before step 4: name the step that is missing first.
    async fn steps_before_agreement(&self, company: &str) -> Result<()> {
        let state = match self.api().get(&api::COMPANY_ONBOARDING, company).await {
            Ok(state) => state,
            Err(error) if api::code(&error).as_deref() == Some("not_found") => {
                let mut error = CliError::usage(no_onboarding(&self.company_name, company));
                error.details.push(self.new_company_hint().await);
                return Err(error);
            }
            Err(error) => return Err(error),
        };
        let current = state["current_step"].as_str().unwrap_or_default();
        if STEPS.iter().any(|(key, _)| *key == current) && step_number(current) < 4 {
            return Err(CliError::usage(format!(
                "Step {} ({}) is not done yet, and the agreement comes after it. Run `oscar onboard` to go on.",
                step_number(current),
                step_name(current)
            )));
        }
        Ok(())
    }

    /// The onboarding the person started and has not finished, and whether this login reaches
    /// its company.
    async fn open_onboarding(&self) -> Result<Option<OpenOnboarding>> {
        let state = self
            .api()
            .call(&api::ONBOARDING_STATE, &[], &[], None)
            .await?;
        if state["company"]["id"].as_str().is_none() {
            return Ok(None);
        }
        let me = self.api().call(&api::ME, &[], &[], None).await?;
        Ok(OpenOnboarding::read(state, &me))
    }

    /// The hint after "set up already": a new company, or the open onboarding to finish first.
    async fn new_company_hint(&self) -> String {
        match self.open_onboarding().await {
            Ok(Some(open)) => open.new_company_hint(),
            _ => NEW_COMPANY_HINT.to_owned(),
        }
    }

    /// `--registry-code` while an onboarding is open: the server keeps one open onboarding at a
    /// time (409 `onboarding_company_exists`), as the web app, so the code of another company
    /// is refused with the one to finish, never ignored (N3-P2-1). The code of the open one goes
    /// on with it.
    fn one_open_onboarding(&mut self, open: &OpenOnboarding) -> Result<()> {
        let Some(code) = self.options.registry_code.as_deref() else {
            return Ok(());
        };
        if !open.is_other_code(code) {
            self.note(&format!(
                "{} ({}) is onboarding already: this goes on at {}.",
                open.name,
                code.trim(),
                open.step_text()
            ));
            return Ok(());
        }
        let code_text = open
            .registry_code
            .as_deref()
            .map(|own| format!(", registry code {own}"))
            .unwrap_or_default();
        let mut error = CliError::usage(format!(
            "The onboarding of {}{code_text} is still open, at {}. Finish it before you onboard another company.",
            open.name,
            open.step_text()
        ));
        if open.reachable {
            error
                .details
                .push("Go on with it: `oscar onboard` (without --registry-code).".to_owned());
        } else {
            error.details.push(open.elsewhere_line());
            error.details.push(open.login_hint());
        }
        Err(error)
    }

    /// The open onboarding of a company this login cannot reach (another organization): say
    /// so, and offer the browser login to that organization with the consent-page hint, as
    /// after step 1 (N3-P1-1). With no terminal, the error says what to run.
    async fn reach_open(&mut self, open: &OpenOnboarding) -> Result<()> {
        if open.reachable {
            return Ok(());
        }
        self.company_name.clone_from(&open.name);
        if !self.prompter.interactive() {
            let mut error = CliError::auth(open.elsewhere_line());
            error.details.push(open.login_hint());
            return Err(error);
        }
        self.note(&format!(
            "{} {}",
            theme::symbol(theme::Sym::Bang),
            open.elsewhere_line()
        ));
        let organization = open.organization_name().to_owned();
        let yes = self
            .prompter
            .confirm(
                &format!("Log in to \"{organization}\" now, in the browser?"),
                true,
                "`oscar login`",
            )
            .await?;
        if !yes {
            return Err(CliError::usage(open.login_hint()));
        }
        self.login_again(&open.id, &organization, open.login_organization.as_deref())
            .await
    }

    /// `oscar onboard status` for an onboarding open in another organization: the steps of
    /// the person's state, and how to reach it.
    fn status_elsewhere(&mut self, open: &OpenOnboarding) -> Result<()> {
        if self.app.json() {
            self.app.out.json(&open.state);
            return Ok(());
        }
        let theme = self.app.out.theme;
        self.app
            .out
            .line(&format!("{} ({})", theme.bold(&open.name), open.id));
        for line in status_lines(&open.state, &theme) {
            self.app.out.line(&line);
        }
        self.app.out.line(&format!(
            "{} {}",
            theme.paint(theme::Role::Warn, theme.symbol(theme::Sym::Bang)),
            open.elsewhere_line()
        ));
        self.app.out.line(&open.login_hint());
        Ok(())
    }

    async fn set_company_name(&mut self, company: &str) {
        let me = self.api().call(&api::ME, &[], &[], None).await.ok();
        let name = me.as_ref().and_then(|me| {
            me["companies"]
                .as_array()?
                .iter()
                .find(|item| item["id"].as_str() == Some(company))?["name"]
                .as_str()
                .map(str::to_owned)
        });
        if let Some(name) = name {
            self.company_name = name;
        } else if self.company_name.is_empty() {
            self.company_name = "your company".to_owned();
        }
    }

    fn link(&self, path: &str) -> String {
        web_link(&self.base_url, path)
    }

    fn note(&mut self, text: &str) {
        self.app.out.note(text);
    }

    fn header(&mut self, key: &str) {
        let line = format!("Step {} of 6 · {}", step_number(key), step_name(key));
        let line = theme::current().paint_bold(theme::Role::Accent, &line);
        self.app.out.note("");
        self.app.out.note(&line);
        self.app.out.flush();
    }

    fn success(&mut self, text: &str) {
        render::success_line(&mut self.app.out, text);
        self.app.out.flush();
    }

    /// Open `url` in the browser when there is a display, and print it in every case.
    fn open(&mut self, what: &str, url: &str) {
        let opened = !login::no_display(&self.app.env) && browser::open(url, &self.app.env);
        let how = if opened {
            "Opened in the browser"
        } else {
            "Open"
        };
        self.note(&format!("{how}: {what}"));
        self.note(&format!("    {}", theme::link(url, url)));
        self.app.out.flush();
    }

    /// Wait until `done` says so, with a spinner; `None` after the time limit.
    async fn wait<F, Fut>(&mut self, message: &str, mut done: F) -> Result<Option<Value>>
    where
        F: FnMut(Client) -> Fut,
        Fut: std::future::Future<Output = Result<Option<Value>>>,
    {
        self.app.out.flush();
        let spinner = self.prompter.spinner(message);
        let started = Instant::now();
        let limit = self.limit;
        let poll = self.poll;
        let client = self.client.clone();
        let waiting = async move {
            loop {
                if let Some(value) = done(client.clone()).await? {
                    return Ok(Some(value));
                }
                if started.elapsed() >= limit {
                    return Ok(None);
                }
                tokio::time::sleep(poll).await;
            }
        };
        let result = tokio::select! {
            result = waiting => result,
            // Ctrl-C ends the wait, never the work: say how to go on (N3-8).
            _ = tokio::signal::ctrl_c() => {
                spinner.finish();
                return Err(CliError::interrupted(
                    "Stopped waiting. Nothing is lost: run `oscar onboard` to go on.",
                ));
            }
        };
        spinner.finish();
        result
    }

    /// Run each step from the one the server says, until the onboarding is done or a step waits
    /// for something outside the terminal.
    async fn steps(&mut self, company: &str) -> Result<()> {
        self.set_company_name(company).await;
        let mut last = String::new();
        loop {
            let state = self.api().get(&api::COMPANY_ONBOARDING, company).await?;
            if let Some(reason) = state["blocked_reason"].as_str().filter(|r| !r.is_empty()) {
                let mut error = CliError::failed(format!(
                    "The onboarding of {} needs our support team ({reason}).",
                    self.company_name
                ));
                error.details.push(format!(
                    "Talk to us from the onboarding page: {}",
                    self.link("/onboarding")
                ));
                return Err(error);
            }
            let current = state["current_step"]
                .as_str()
                .unwrap_or_default()
                .to_owned();
            if current == last && current != "done" {
                // The step ran and the server did not move on: stop rather than ask again.
                self.note(&format!(
                    "The step \"{}\" is not done yet. Run `oscar onboard` again to go on.",
                    step_name(&current)
                ));
                return Ok(());
            }
            let advanced = match current.as_str() {
                "business" => {
                    self.business(company).await?;
                    true
                }
                "identity" => self.identity(company).await?,
                "agreement" => self.agreement(company).await?,
                "plan" => self.plan(company).await?,
                "done" => {
                    self.setup(company).await?;
                    return Ok(());
                }
                other => {
                    return Err(CliError::failed(format!(
                        "The server names a step this oscar does not know: `{other}`. Update oscar, or go on in the browser: {}",
                        self.link("/onboarding")
                    )));
                }
            };
            if !advanced {
                return Ok(());
            }
            last = current;
        }
    }

    // ------------------------------------------------------------------
    // Step 1: the company
    // ------------------------------------------------------------------

    /// Search the register and give the registry code of the company the person picks. Text
    /// typed with no row chosen is a registry code, the one company it names, or a list to
    /// choose from; the search asks again when it names none.
    async fn search_company(&mut self) -> Result<String> {
        loop {
            let client = self.client.clone();
            let source = move |text: String| {
                let client = client.clone();
                async move {
                    Api::new(&client)
                        .call(
                            &api::REGISTER_SEARCH,
                            &[],
                            &[("q", &text), ("limit", "8")],
                            None,
                        )
                        .await
                        .map(|answer| hits(&answer))
                        .unwrap_or_default()
                }
            };
            let pick = self
                .prompter
                .search(
                    "Your company",
                    Some("Type a name, a registry code or a VAT number; pick one with the arrows"),
                    2,
                    source,
                    "--registry-code <code>",
                )
                .await?;
            let text = match pick {
                crate::prompt::Pick::Item(hit) => return Ok(hit.code),
                crate::prompt::Pick::Typed(text) if is_registry_code(&text) => {
                    return Ok(text.trim().to_owned());
                }
                crate::prompt::Pick::Typed(text) => text.trim().to_owned(),
            };
            if text.chars().count() < 2 {
                self.note("Type at least two letters of the name, or the registry code.");
                continue;
            }
            let answer = self
                .api()
                .call(
                    &api::REGISTER_SEARCH,
                    &[],
                    &[("q", &text), ("limit", "8")],
                    None,
                )
                .await?;
            let mut found = hits(&answer);
            match found.len() {
                0 => self.note(&format!(
                    "No company of the Business Register matches `{text}`. Try another name or the registry code (a company registered today is there tomorrow)."
                )),
                1 => return Ok(found.remove(0).code),
                _ => {
                    found.push(Hit {
                        code: String::new(),
                        line: "None of these: search again".to_owned(),
                    });
                    let chosen = self
                        .prompter
                        .select(
                            &format!("Which company matches `{text}`?"),
                            found,
                            "--registry-code <code>",
                        )
                        .await?;
                    if !chosen.code.is_empty() {
                        return Ok(chosen.code);
                    }
                }
            }
        }
    }

    /// Show the company the register gives.
    fn show_prefill(&mut self, draft: &Value) {
        let prefill = &draft["prefill"];
        let text = |value: &Value| value.as_str().unwrap_or_default().to_owned();
        let names = |list: &Value| -> String {
            list.as_array()
                .map(|items| {
                    items
                        .iter()
                        .map(|item| match item["share_percent"].as_str() {
                            Some(share) if !share.is_empty() => {
                                format!("{} ({share} %)", text(&item["name"]))
                            }
                            _ => text(&item["name"]),
                        })
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .unwrap_or_default()
        };
        // English where the register writes Estonian: the legal form and the status by their code.
        let legal_form = prefill["values"]["legal_form"]
            .as_str()
            .and_then(legal_form_name)
            .map_or_else(|| text(&prefill["legal_form_text"]), str::to_owned);
        let status = prefill["status"]
            .as_str()
            .and_then(status_name)
            .map_or_else(|| text(&prefill["status_text"]), str::to_owned);
        let rows = [
            ("Name", text(&prefill["values"]["legal_name"])),
            ("Legal form", legal_form),
            ("Registry code", text(&prefill["registry_code"])),
            ("Registered", text(&prefill["registration_date"])),
            ("Status", status),
            ("Address", text(&prefill["values"]["address"])),
            ("Board", names(&prefill["board_members"])),
            ("Shareholders", names(&prefill["shareholders"])),
            ("Beneficial owners", names(&draft["beneficial_owners"])),
        ];
        self.note("");
        for (label, value) in rows {
            if !value.is_empty() {
                let label = theme::paint(theme::Role::Dim, &format!("{label:<18}"));
                self.note(&format!("  {label} {value}"));
            }
        }
        self.note("");
    }

    /// Step 1: pick, check and confirm the company. Gives the id of the new company.
    async fn company_step(&mut self) -> Result<String> {
        self.header("company");
        let code = match self.options.registry_code.clone() {
            Some(code) => code,
            None => self.search_company().await?,
        };
        let state = self
            .api()
            .call(
                &api::ONBOARDING_DRAFT,
                &[],
                &[],
                Some(&json!({"registry_code": code.trim()})),
            )
            .await?;
        let draft = state["draft"].clone();
        let prefill = &draft["prefill"];
        if let Some(guard) = prefill["guard"].as_str().filter(|g| !g.is_empty()) {
            let mut error = CliError::failed(format!(
                "{} cannot be set up here ({guard}).",
                prefill["values"]["legal_name"]
                    .as_str()
                    .unwrap_or("The company")
            ));
            error.details.push(format!(
                "Talk to us from the onboarding page: {}",
                self.link("/onboarding")
            ));
            return Err(error);
        }
        self.show_prefill(&draft);
        let owners = draft["beneficial_owners"].as_array().map_or(0, Vec::len);
        let to_explain = prefill["legal_shareholders_to_explain"]
            .as_array()
            .is_some_and(|items| !items.is_empty());
        let resolved = prefill["ownership_chain"]["resolved"]
            .as_bool()
            .unwrap_or(false);
        if owners == 0 || (to_explain && !resolved) {
            let mut error = CliError::failed(
                "The beneficial owners of this company need answers the terminal does not ask yet.",
            );
            error.details.push(format!(
                "Your pick is saved: name the owners and confirm in the browser, then run `oscar login` and `oscar onboard`: {}",
                self.link("/onboarding")
            ));
            return Err(error);
        }
        let changes = self.books_and_website(&draft).await?;
        if !changes.as_object().is_some_and(Map::is_empty) {
            self.api()
                .call(&api::ONBOARDING_DRAFT, &[], &[], Some(&changes))
                .await?;
        }
        let name = prefill["values"]["legal_name"]
            .as_str()
            .unwrap_or("the company")
            .to_owned();
        let confirmed = self.options.confirm_owners
            || self
                .prompter
                .confirm(
                    &format!("I confirm in writing that these are the beneficial owners of {name}"),
                    false,
                    "--confirm-owners",
                )
                .await?;
        if !confirmed {
            return Err(CliError::usage(
                "The beneficial owners are not confirmed, so the company was not created. Your answers are saved.",
            ));
        }
        let state = self
            .api()
            .call(
                &api::ONBOARDING_CONFIRM,
                &[],
                &[],
                Some(&json!({"beneficial_owners_confirmed": true})),
            )
            .await?;
        let company = state["company"]["id"]
            .as_str()
            .ok_or_else(|| CliError::failed("The server made the company but gave no id."))?
            .to_owned();
        self.company_name = state["company"]["name"]
            .as_str()
            .unwrap_or(&name)
            .to_owned();
        let created = format!("{} is created (company {company})", self.company_name);
        self.success(&created);
        self.reach(&company).await?;
        Ok(company)
    }

    /// The books answers and the website of step 1, from the flags or the prompts.
    async fn books_and_website(&mut self, draft: &Value) -> Result<Value> {
        let mut changes = Map::new();
        let saved = |key: &str| {
            draft[key]
                .as_str()
                .filter(|t| !t.is_empty())
                .map(str::to_owned)
        };
        let books_today = match self.options.books_today.clone() {
            Some(answer) => answer,
            None => {
                let default = saved("books_today")
                    .or_else(|| {
                        draft["prefill"]["books_today_default"]
                            .as_str()
                            .map(str::to_owned)
                    })
                    .unwrap_or_default();
                let mut options = choices(&[
                    ("yes", "Yes, in software or with an accountant"),
                    ("no", "No, not yet"),
                ]);
                if default == "no" {
                    options.reverse();
                }
                self.prompter
                    .select(
                        "Does the company keep books today?",
                        options,
                        "--books-today yes|no",
                    )
                    .await?
                    .value
            }
        };
        changes.insert("books_today".into(), json!(books_today));
        if books_today == "no" {
            let reason = match self.options.books_no_reason.clone() {
                Some(reason) => reason,
                None => {
                    self.prompter
                        .text(
                            "Why does the company keep no books yet?",
                            TextOptions {
                                initial: saved("books_no_reason"),
                                ..TextOptions::default()
                            },
                            "--books-no-reason <text>",
                        )
                        .await?
                }
            };
            changes.insert("books_no_reason".into(), json!(reason));
        } else {
            let software = match self.options.books_software.clone() {
                Some(software) => software,
                None => {
                    self.prompter
                        .select(
                            "Where are the books kept?",
                            choices(BOOKS_SOFTWARE),
                            "--books-software <name>",
                        )
                        .await?
                        .value
                }
            };
            if software == "other" {
                let other = match self.options.books_software_other.clone() {
                    Some(other) => other,
                    None => {
                        self.prompter
                            .text(
                                "Which software?",
                                TextOptions::default(),
                                "--books-software-other <name>",
                            )
                            .await?
                    }
                };
                changes.insert("books_software_other".into(), json!(other));
            }
            changes.insert("books_software".into(), json!(software));
        }
        if self.options.no_website {
            changes.insert("no_website".into(), json!(true));
        } else if let Some(website) = self.options.website.clone() {
            changes.insert("website".into(), json!(website));
        } else if self.prompter.interactive() {
            let website = self
                .prompter
                .text(
                    "The company website",
                    TextOptions {
                        initial: saved("website"),
                        help: Some("Leave it empty when the company has no website".into()),
                        ..TextOptions::default()
                    },
                    "--website <url>",
                )
                .await?;
            if website.trim().is_empty() {
                changes.insert("no_website".into(), json!(true));
            } else {
                changes.insert("website".into(), json!(website.trim()));
            }
        } else if saved("website").is_none() {
            return Err(self
                .prompter
                .missing("--website <url> or --no-website", "The company website"));
        }
        Ok(Value::Object(changes))
    }

    /// After the company step: the login must reach the new company. A token of the CLI is bound
    /// to one organization, and the company is in a new one, so the CLI logs in again.
    async fn reach(&mut self, company: &str) -> Result<()> {
        let me = self.api().call(&api::ME, &[], &[], None).await?;
        let reached = me["companies"].as_array().is_some_and(|items| {
            items
                .iter()
                .any(|item| item["id"].as_str() == Some(company))
        });
        if reached {
            return Ok(());
        }
        // The consent page cannot preselect an organization (the authorize request has no hint
        // for it), so the CLI names the one to choose: it carries the name of the company.
        let organization = organization_of(&me);
        self.note(&format!(
            "{} is in a new organization of its own. Log in again to reach it.",
            self.company_name
        ));
        let name = self.company_name.clone();
        self.login_again(company, &name, organization.as_deref())
            .await
    }

    /// Log in again, in the browser (or with a device code with no display), to the
    /// organization `organization`, which holds `company`; `other` is the organization of the
    /// login now. Fails when the new login still does not reach the company.
    async fn login_again(
        &mut self,
        company: &str,
        organization: &str,
        other: Option<&str>,
    ) -> Result<()> {
        // The one thing the person must do right (P2-18): its own block, in the warning colour.
        for line in consent_hint(organization, other) {
            self.note(&line);
        }
        self.app.out.flush();
        let method = if login::no_display(&self.app.env) {
            "--device"
        } else {
            "--browser"
        };
        // The same server and profile: the login never asks for them again.
        let selection = self.app.select(&self.app.config()?)?;
        let globals = self.app.globals.clone();
        if self.app.explicit_profile().is_none() {
            self.app.globals.profile = selection.name;
        }
        let login = run_login(self.app, &[method]).await;
        self.app.globals = globals;
        login?;
        let (client, _) = self.app.client().await?;
        self.client = client;
        let me = self.api().call(&api::ME, &[], &[], None).await?;
        let reached = me["companies"].as_array().is_some_and(|items| {
            items
                .iter()
                .any(|item| item["id"].as_str() == Some(company))
        });
        if !reached {
            return Err(CliError::auth(format!(
                "This login does not reach {} ({company}). Run `oscar login`, choose the organization \"{organization}\", then `oscar onboard -c {company}`.",
                self.company_name
            )));
        }
        Ok(())
    }

    // ------------------------------------------------------------------
    // Step 2: about your business
    // ------------------------------------------------------------------

    /// Ask question `key`; `place` is its number and the count of the questions asked, shown
    /// before the question.
    async fn ask(
        &mut self,
        key: &str,
        current: Option<&Value>,
        place: (usize, usize),
    ) -> Result<Value> {
        let question = questions::question(key)
            .ok_or_else(|| CliError::failed(format!("No question `{key}`.")))?;
        let flag = format!("--answer {key}=<value> or --answers <file.json>");
        let label = format!("({}/{}) {}", place.0, place.1, question.label);
        let label = label.as_str();
        match question.kind {
            Kind::Text => {
                let initial = current.and_then(Value::as_str).map(str::to_owned);
                let text = self
                    .prompter
                    .text(
                        label,
                        TextOptions {
                            initial,
                            check: Some(std::sync::Arc::new(|text: &str| {
                                if text.trim().is_empty() {
                                    Err("An answer is needed.".into())
                                } else {
                                    Ok(())
                                }
                            })),
                            ..TextOptions::default()
                        },
                        &flag,
                    )
                    .await?;
                Ok(json!(text.trim()))
            }
            Kind::Countries => {
                let initial = current.and_then(Value::as_array).map(|codes| {
                    codes
                        .iter()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join(", ")
                });
                let text = self
                    .prompter
                    .text(
                        label,
                        TextOptions {
                            initial: initial.filter(|text| !text.is_empty()),
                            check: Some(std::sync::Arc::new(|text: &str| {
                                questions::countries(text).map(|_| ())
                            })),
                            ..TextOptions::default()
                        },
                        &flag,
                    )
                    .await?;
                Ok(json!(questions::countries(&text).map_err(CliError::usage)?))
            }
            Kind::YesNo => {
                // No default: Enter alone never answers a risk question (only an earlier answer
                // is offered again).
                let answer = self
                    .prompter
                    .yes_no(label, current.and_then(Value::as_bool), &flag)
                    .await?;
                Ok(Value::Bool(answer))
            }
            Kind::Choice(values) => {
                let mut options = choices(values);
                if let Some(index) = current
                    .and_then(Value::as_str)
                    .and_then(|value| options.iter().position(|choice| choice.value == value))
                {
                    options.rotate_left(index);
                }
                let choice = self.prompter.select(label, options, &flag).await?;
                Ok(json!(choice.value))
            }
        }
    }

    /// Save the answers so far as a draft, and give what the server asks now.
    async fn save_answers(
        &self,
        company: &str,
        answers: &Map<String, Value>,
        website: &Map<String, Value>,
    ) -> Result<Value> {
        let mut body: Map<String, Value> = answers
            .iter()
            .filter(|(_, value)| questions::answered(Some(value)))
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();
        body.extend(website.clone());
        let saved = self
            .api()
            .call(
                &api::QUESTIONNAIRE_SAVE,
                &[("company_id", company)],
                &[],
                Some(&Value::Object(body)),
            )
            .await?;
        Ok(saved["asked"].clone())
    }

    /// The website answer of step 2, from the flags, the saved answer, or a prompt. Step 2 asks
    /// it again (the web form does): an empty website with no "no website" is not an answer.
    async fn website_answer(&mut self, questionnaire: &Value) -> Result<Map<String, Value>> {
        let mut answer = Map::new();
        if self.options.no_website {
            answer.insert("no_website".into(), json!(true));
        } else if let Some(website) = self.options.website.clone() {
            answer.insert("website".into(), json!(website.trim()));
        } else if questionnaire["no_website"].as_bool() == Some(true)
            || questionnaire["website"]
                .as_str()
                .is_some_and(|w| !w.trim().is_empty())
        {
            // Answered already (on the company step, or a saved draft).
        } else if self.prompter.interactive() {
            let initial = questionnaire["register_website"]
                .as_str()
                .filter(|w| !w.is_empty())
                .map(str::to_owned);
            let website = self
                .prompter
                .text(
                    "The company website",
                    TextOptions {
                        initial,
                        help: Some("Leave it empty when the company has no website".into()),
                        ..TextOptions::default()
                    },
                    WEBSITE_FLAG,
                )
                .await?;
            if website.trim().is_empty() {
                answer.insert("no_website".into(), json!(true));
            } else {
                answer.insert("website".into(), json!(website.trim()));
            }
        } else {
            return Err(self.prompter.missing(WEBSITE_FLAG, "The company website"));
        }
        Ok(answer)
    }

    /// The usage error of the answers `--no-input` (or no terminal) leaves out, before anything is
    /// sent: one line per missing answer, with its flag.
    fn missing_answers(&self, missing: &[&str], website: bool) -> CliError {
        let mut error = self.prompter.missing(
            "--answer <key>=<value> for each, or --answers <file.json>",
            "An answer to each question below",
        );
        let mut rows: Vec<(String, &str)> = Vec::new();
        if website {
            rows.push((WEBSITE_FLAG.to_owned(), "The company website"));
        }
        for key in missing {
            let label = questions::question(key).map_or("", |q| q.label);
            rows.push((
                format!("--answer {key}={}", questions::value_hint(key)),
                label,
            ));
        }
        let width = rows.iter().map(|(flag, _)| flag.len()).max().unwrap_or(0);
        error.details = rows
            .into_iter()
            .map(|(flag, label)| format!("{flag:<width$}  {label}"))
            .collect();
        error
    }

    /// Step 2: the questionnaire, a review, the submission.
    async fn business(&mut self, company: &str) -> Result<()> {
        self.header("business");
        let questionnaire = self.api().get(&api::QUESTIONNAIRE, company).await?;
        let mut answers = Map::new();
        for key in api::QUESTIONNAIRE_FIELDS {
            let value = &questionnaire[*key];
            if questions::answered(Some(value)) {
                answers.insert((*key).to_owned(), value.clone());
            }
        }
        if !answers.contains_key("activity_description")
            && let Some(activity) = register_activity(&questionnaire)
        {
            answers.insert("activity_description".into(), json!(activity));
        }
        let given = self.options.answers.clone();
        answers.extend(given.clone());
        let mut asked = questionnaire["asked"].clone();
        if !self.prompter.interactive() {
            // Every missing answer is a usage error before anything is sent.
            let missing = questions::missing(&answers, &asked);
            let website = self.website_answer(&questionnaire).await;
            if !missing.is_empty() {
                return Err(self.missing_answers(&missing, website.is_err()));
            }
            let website = website?;
            asked = self.save_answers(company, &answers, &website).await?;
            // The server decides questions 11 and 12 from the saved answers.
            let missing = questions::missing(&answers, &asked);
            if !missing.is_empty() {
                return Err(self.missing_answers(&missing, false));
            }
            return self.submit(company, &answers, &asked, &website).await;
        }
        let website = self.website_answer(&questionnaire).await?;
        for question in QUESTIONS {
            if matches!(question.key, "estonia_reason" | "startup_funds_source") {
                // The server decides these from the answers saved so far.
                asked = self.save_answers(company, &answers, &website).await?;
            }
            if given.contains_key(question.key) || !questions::asked(question.key, &answers, &asked)
            {
                continue;
            }
            let place = questions::place(question.key, &answers, &asked);
            let value = self
                .ask(question.key, answers.get(question.key), place)
                .await?;
            answers.insert(question.key.to_owned(), value);
        }
        asked = self.save_answers(company, &answers, &website).await?;
        self.note("");
        self.note(&theme::current().bold("Your answers"));
        for line in review_lines(&answers, &asked) {
            self.note(&line);
        }
        // The whole review is on the screen before the prompt takes the terminal.
        self.app.out.flush();
        let submit = self
            .prompter
            .confirm("Submit these answers?", true, "--answers <file.json>")
            .await?;
        if !submit {
            return Err(CliError::usage(
                "Not submitted. Your answers are saved as a draft: run `oscar onboard` to change and submit them.",
            ));
        }
        self.submit(company, &answers, &asked, &website).await
    }

    /// Submit the answers of step 2.
    async fn submit(
        &mut self,
        company: &str,
        answers: &Map<String, Value>,
        asked: &Value,
        website: &Map<String, Value>,
    ) -> Result<()> {
        let mut body = questions::body(answers, asked);
        if let Value::Object(fields) = &mut body {
            fields.extend(website.clone());
        }
        let submitted = self
            .api()
            .call(
                &api::QUESTIONNAIRE_SUBMIT,
                &[("company_id", company)],
                &[],
                Some(&body),
            )
            .await?;
        if let Some(reason) = submitted["support_reason"]
            .as_str()
            .filter(|r| !r.is_empty())
        {
            let mut error = CliError::failed(format!(
                "Your answers are saved, and the onboarding needs our support team ({reason})."
            ));
            error.details.push(format!(
                "Talk to us from the onboarding page: {}",
                self.link("/onboarding")
            ));
            return Err(error);
        }
        self.success("The answers about your business are submitted");
        Ok(())
    }

    // ------------------------------------------------------------------
    // Step 3: identity
    // ------------------------------------------------------------------

    async fn requirements(&self, company: &str) -> Result<Vec<Value>> {
        let answer = self
            .api()
            .call(
                &api::KYC_REQUIREMENTS,
                &[("company_id", company)],
                &[("limit", "100")],
                None,
            )
            .await?;
        Ok(answer["items"]
            .as_array()
            .map(|items| {
                items
                    .iter()
                    .filter(|item| item["role_ended"].as_bool() != Some(true))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default())
    }

    async fn yes_no(&mut self, given: Option<bool>, message: &str, flag: &str) -> Result<bool> {
        if let Some(answer) = given {
            return Ok(answer);
        }
        self.prompter.yes_no(message, None, flag).await
    }

    /// Where the caller stands in step 3, as the web page reads it.
    async fn identity_step(&self, company: &str) -> Result<Value> {
        self.api().get(&api::IDENTITY_STEP, company).await
    }

    /// The persons of the company who verify their identity, the caller first when the server
    /// made the requirement already.
    fn show_requirements(&mut self, requirements: &[Value], mine: Option<&str>) {
        if requirements.is_empty() {
            return;
        }
        self.note("Who verifies their identity:");
        for item in requirements {
            let done = matches!(item["status"].as_str(), Some("satisfied" | "waived"));
            let symbol = if done {
                theme::paint(theme::Role::Ok, theme::symbol(theme::Sym::Check))
            } else {
                theme::paint(theme::Role::Muted, theme::symbol(theme::Sym::Dot))
            };
            let you = if mine.is_some() && item["id"].as_str() == mine {
                " (you)"
            } else {
                ""
            };
            let state = item["verification_status"]
                .as_str()
                .filter(|state| *state != "created")
                .unwrap_or("to do")
                .replace('_', " ");
            self.note(&format!(
                "  {symbol} {}{you} · {} · {}",
                person_name(item),
                role_name(item["role"].as_str().unwrap_or_default()),
                if done { "done" } else { &state }
            ));
        }
    }

    /// Step 3, as the web page runs it: the privacy notices and
    /// the PEP question, then Didit for the caller (the server makes the caller's requirement at
    /// the start); then the CLI waits for the result. Each state of the server has its own
    /// message. `false` when the CLI stops.
    async fn identity(&mut self, company: &str) -> Result<bool> {
        self.header("identity");
        let step = self.identity_step(company).await?;
        let requirements = self.requirements(company).await?;
        let mine = step["requirement_id"].as_str().map(str::to_owned);
        self.show_requirements(&requirements, mine.as_deref());
        let state = step["state"].as_str().unwrap_or("not_started").to_owned();
        let pep_answered = step["pep_answered"].as_bool() == Some(true);
        match state.as_str() {
            "passed" if pep_answered => {
                self.verified(&step);
                return Ok(true);
            }
            "passed" => {
                // Verified, with the PEP question still open: the web page asks it again.
                self.note("Your identity is verified. One question is left.");
                self.start_check(company, false).await?;
                let step = self.identity_step(company).await?;
                if step["done"].as_bool() == Some(true) {
                    self.verified(&step);
                    return Ok(true);
                }
                return Ok(self.stopped(&step));
            }
            "in_progress" => {
                self.note("Your identity check is running.");
                self.link_again(company).await?;
            }
            "in_review" | "declined" | "not_board_member" | "register_unavailable" => {
                return Ok(self.stopped(&step));
            }
            other => {
                if other == "retry" {
                    self.note("Smart-ID or Mobile-ID was cancelled or failed. Try again.");
                }
                self.start_check(company, pep_answered).await?;
            }
        }
        let (requirements, mine) = if requirements.is_empty() {
            // The start made the caller's requirement: list who verifies now (N3-8).
            let step = self.identity_step(company).await?;
            let mine = step["requirement_id"].as_str().map(str::to_owned);
            let requirements = self.requirements(company).await?;
            self.show_requirements(&requirements, mine.as_deref());
            (requirements, mine)
        } else {
            (requirements, mine)
        };
        self.invite_others(company, &requirements, mine.as_deref())
            .await?;
        if self.options.no_wait {
            self.note("Run `oscar onboard` again when the identity check is done.");
            return Ok(false);
        }
        let company_owned = company.to_owned();
        let result = self
            .wait("Waiting for the identity check (Didit)...", move |client| {
                let company = company_owned.clone();
                async move {
                    let api = Api::new(&client);
                    let step = api.get(&api::IDENTITY_STEP, &company).await?;
                    // `not_started` until the person opens the link; `in_progress` while it runs.
                    let waiting =
                        matches!(step["state"].as_str(), Some("not_started" | "in_progress"));
                    Ok((!waiting).then_some(step))
                }
            })
            .await?;
        match result {
            Some(step) if step["done"].as_bool() == Some(true) => {
                self.verified(&step);
                Ok(true)
            }
            Some(step) => Ok(self.stopped(&step)),
            None => {
                self.note("Still waiting for the identity check. Run `oscar onboard` again when it is done.");
                Ok(false)
            }
        }
    }

    /// A check that runs: its link again, for a person who closed the page (P2-21). The start
    /// call gives the open Didit session back; it needs the consent, given again with a yes on
    /// a terminal or with `--consent`.
    async fn link_again(&mut self, company: &str) -> Result<()> {
        let again = if self.options.consent {
            true
        } else if self.prompter.interactive() {
            self.prompter
                .confirm(
                    "Show the link of the identity check again?",
                    false,
                    "--consent",
                )
                .await?
        } else {
            self.note("To see the link of the check again: `oscar onboard --consent`.");
            false
        };
        if again {
            self.start_check(company, true).await?;
        }
        Ok(())
    }

    /// The success line of step 3.
    fn verified(&mut self, step: &Value) {
        match step["board_member"]
            .as_str()
            .filter(|name| !name.is_empty())
        {
            Some(name) => self.success(&format!(
                "Your identity is verified: {name}, a board member of the company"
            )),
            None => self.success("Your identity is verified"),
        }
    }

    /// A state of step 3 the terminal cannot go past: say what it means and what to do. Always
    /// `false`: the run stops.
    fn stopped(&mut self, step: &Value) -> bool {
        let support = self.link("/onboarding");
        let message = match step["state"].as_str().unwrap_or_default() {
            "in_review" => format!(
                "We are checking your verification, and we email you when it is done (within one business day). Run `oscar onboard` again later, or talk to us: {support}"
            ),
            "declined" => format!(
                "We could not verify your identity online. Talk to us from the onboarding page: {support}"
            ),
            "not_board_member" => format!(
                "Only a board member can set up the company, and the verified name matches no board member in the Business Register. Ask a board member to sign up at {}, or talk to us: {support}",
                self.link("/signup")
            ),
            "register_unavailable" => format!(
                "We cannot check the board of the company in the Business Register now. Run `oscar onboard` again later, or talk to us: {support}"
            ),
            "retry" => "Smart-ID or Mobile-ID was cancelled or failed. Run `oscar onboard` to try again.".to_owned(),
            "passed" => "Your identity is verified, and the question on politically exposed persons is not answered yet. Run `oscar onboard` to answer it.".to_owned(),
            other => format!(
                "The identity check is {}. Run `oscar onboard` again, or talk to us: {support}",
                other.replace('_', " ")
            ),
        };
        self.note(&message);
        false
    }

    /// Start the check of the caller as the web page does: the privacy notices, the PEP question
    /// (unless answered before), then the Didit link. `pep_answered`: the PEP answer is kept.
    async fn start_check(&mut self, company: &str, pep_answered: bool) -> Result<()> {
        let pep_given = self.options.pep.is_some() || self.options.pep_related.is_some();
        let ask_pep = !pep_answered || pep_given;
        if !self.prompter.interactive() {
            self.missing_identity_flags(ask_pep)?;
        }
        let privacy = self.link("/privacy");
        let consent = self.options.consent
            || self
                .prompter
                .confirm(
                    &format!("I have read the privacy notice ({privacy}) and the Didit Verification Privacy Notice ({DIDIT_PRIVACY_URL}), and I agree to the check"),
                    false,
                    "--consent",
                )
                .await?;
        if !consent {
            return Err(CliError::usage(
                "The identity check needs your consent to the privacy notices.",
            ));
        }
        let mut body = json!({"consent": true});
        if let Some(language) = &self.language {
            body["language"] = json!(language);
        }
        if ask_pep {
            let pep = self
                .yes_no(self.options.pep, PEP_QUESTION, "--pep yes|no")
                .await?;
            let related = self
                .yes_no(
                    self.options.pep_related,
                    PEP_RELATED_QUESTION,
                    "--pep-related yes|no",
                )
                .await?;
            body["is_pep"] = json!(pep);
            body["is_pep_related"] = json!(related);
            if pep || related {
                let details = match self.options.pep_details.clone() {
                    Some(details) => details,
                    None => {
                        self.prompter
                            .text(
                                PEP_DETAILS_QUESTION,
                                TextOptions::default(),
                                "--pep-details <text>",
                            )
                            .await?
                    }
                };
                body["pep_details"] = json!(details);
            }
        }
        let started = match self
            .api()
            .call(
                &api::IDENTITY_START,
                &[("company_id", company)],
                &[],
                Some(&body),
            )
            .await
        {
            Ok(started) => started,
            // Verified already: the start recorded the PEP answer, and no check runs.
            Err(error) if api::code(&error).as_deref() == Some("identity_verified") => {
                return Ok(());
            }
            Err(error) => return Err(error),
        };
        let url = started["url"].as_str().unwrap_or_default().to_owned();
        if url.is_empty() {
            self.note("Your earlier identity check is reused: no new check is needed.");
            return Ok(());
        }
        self.note("Verify your identity with Smart-ID or Mobile-ID, a photo of your identity document and a selfie, on your phone or in the browser.");
        if self.prompter.interactive()
            && let Some(code) = qr_code(&url)
        {
            self.note("Scan this code with your phone:");
            self.note(&code);
        }
        self.open("the identity check (Didit)", &url);
        Ok(())
    }

    /// With no terminal: every flag of step 3 that is missing, in one usage error (N3-5).
    fn missing_identity_flags(&self, ask_pep: bool) -> Result<()> {
        let options = &self.options;
        let mut rows: Vec<(&str, &str)> = Vec::new();
        if !options.consent {
            rows.push((
                "--consent",
                "Your consent to our privacy notice and the Didit Verification Privacy Notice",
            ));
        }
        if ask_pep {
            if options.pep.is_none() {
                rows.push(("--pep yes|no", PEP_QUESTION));
            }
            if options.pep_related.is_none() {
                rows.push(("--pep-related yes|no", PEP_RELATED_QUESTION));
            }
            let yes = options.pep == Some(true) || options.pep_related == Some(true);
            if yes && options.pep_details.is_none() {
                rows.push(("--pep-details <text>", PEP_DETAILS_QUESTION));
            }
        }
        if rows.is_empty() {
            return Ok(());
        }
        let mut error = self
            .prompter
            .missing("each flag below", "An answer to each question below");
        let width = rows.iter().map(|(flag, _)| flag.len()).max().unwrap_or(0);
        error.details = rows
            .into_iter()
            .map(|(flag, label)| format!("{flag:<width$}  {label}"))
            .collect();
        Err(error)
    }

    /// The other persons who must verify: a link to send to each, on request.
    async fn invite_others(
        &mut self,
        company: &str,
        requirements: &[Value],
        mine: Option<&str>,
    ) -> Result<()> {
        let others: Vec<&Value> = requirements
            .iter()
            .filter(|item| item["status"].as_str() == Some("open"))
            .filter(|item| mine.is_none() || item["id"].as_str() != mine)
            .collect();
        for item in others {
            let name = person_name(item);
            let wanted = self.options.invite_links
                || (self.prompter.interactive()
                    && self
                        .prompter
                        .confirm(
                            &format!("Make an invite link for {name}?"),
                            false,
                            "--invite-links",
                        )
                        .await?);
            if !wanted {
                continue;
            }
            let id = item["id"].as_str().unwrap_or_default().to_owned();
            let link = self
                .api()
                .call(
                    &api::KYC_LINK,
                    &[("company_id", company), ("requirement_id", &id)],
                    &[],
                    None,
                )
                .await?;
            let url = link["url"].as_str().unwrap_or_default();
            let until = link["expires_at"].as_str().unwrap_or_default();
            self.note(&format!(
                "  Send this link to {name} (valid until {until}):"
            ));
            self.note(&format!("    {url}"));
        }
        Ok(())
    }

    // ------------------------------------------------------------------
    // Step 4: the client agreement
    // ------------------------------------------------------------------

    /// Step 4: read the agreement in the pager and accept it. `false` when the person left.
    async fn agreement(&mut self, company: &str) -> Result<bool> {
        self.header("agreement");
        loop {
            let query: Vec<(&str, &str)> = self
                .language
                .as_deref()
                .map(|language| vec![("language", language)])
                .unwrap_or_default();
            let doc = self
                .api()
                .call(&api::AGREEMENT, &[("company_id", company)], &query, None)
                .await?;
            if let Some(acceptance) = doc["acceptance"].as_object() {
                let at = acceptance
                    .get("accepted_at")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let by = acceptance
                    .get("person_name")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                self.note(&format!(
                    "The client agreement was accepted by {by} at {at}."
                ));
                return Ok(true);
            }
            let web = self.link(&format!("/c/{company}/agreement"));
            if doc["can_accept"].as_bool() != Some(true) {
                let problem = doc["problem"].as_str().unwrap_or_default();
                let mut error = CliError::failed(format!(
                    "Only the board member who verified their identity in step 3 accepts the agreement{}.",
                    if problem.is_empty() {
                        String::new()
                    } else {
                        format!(" ({problem})")
                    }
                ));
                error.details.push(format!("The agreement: {web}"));
                return Err(error);
            }
            if !self.prompter.interactive() || !self.app.env.stdout_tty {
                let mut error = CliError::usage(
                    "The client agreement is accepted only after reading it: run `oscar onboard` on a terminal, or accept it in the browser.",
                );
                error.details.push(web);
                return Err(error);
            }
            let document = Document {
                version: doc["version"].as_str().unwrap_or_default().to_owned(),
                language: doc["language"].as_str().unwrap_or("en").to_owned(),
                text: doc["text"].as_str().unwrap_or_default().to_owned(),
                sha256: doc["text_sha256"].as_str().unwrap_or_default().to_owned(),
            };
            if sha256_hex(&document.text) != document.sha256 {
                return Err(CliError::failed(
                    "The text of the agreement does not match its SHA-256, so nothing is accepted. Try again later.",
                ));
            }
            self.app.out.flush();
            let name = self.company_name.clone();
            let color = theme::current().color;
            let shown = document.clone();
            let outcome = tokio::task::spawn_blocking(move || {
                let mut pager = Pager::new(shown, &name, color, 80, 24);
                pager::run(&mut pager)
            })
            .await
            .map_err(|error| CliError::failed(format!("The pager failed: {error}")))?
            .map_err(|error| CliError::failed(format!("The pager failed: {error}")))?;
            match outcome {
                Outcome::Quit => {
                    self.note("You left the agreement without accepting it. Run `oscar onboard` to read it again.");
                    return Ok(false);
                }
                Outcome::SwitchLanguage => {
                    let other = if document.language == "et" {
                        "en"
                    } else {
                        "et"
                    };
                    self.language = Some(other.to_owned());
                }
                Outcome::Accepted => {
                    let body = json!({
                        "version": document.version,
                        "language": document.language,
                        "text_sha256": document.sha256,
                    });
                    match self
                        .api()
                        .call(
                            &api::AGREEMENT_ACCEPT,
                            &[("company_id", company)],
                            &[],
                            Some(&body),
                        )
                        .await
                    {
                        Ok(_) => {
                            self.language = Some(document.language.clone());
                            let message = format!(
                                "The client agreement is accepted on behalf of {} (version {}, {})",
                                self.company_name, document.version, document.language
                            );
                            self.success(&message);
                            return Ok(true);
                        }
                        Err(error) if api::code(&error).as_deref() == Some("agreement_changed") => {
                            self.note(
                                "The agreement changed while you read it. Read the new version.",
                            );
                        }
                        Err(error) => return Err(error),
                    }
                }
            }
        }
    }

    /// Save the evidence PDF of the acceptance. An existing file is a usage error without `force`.
    async fn download(&mut self, company: &str, path: &str, force: bool) -> Result<()> {
        let pdf = match self.api().agreement_pdf(company).await {
            Ok(pdf) => pdf,
            Err(error) if api::code(&error).as_deref() == Some("no_agreement") => {
                let mut error = error;
                error
                    .details
                    .push("Accept the agreement first: `oscar onboard agreement`.".into());
                return Err(error);
            }
            Err(error) => return Err(error),
        };
        let path = if path.is_empty() {
            format!("client-agreement-{company}.pdf")
        } else {
            path.to_owned()
        };
        let file = std::path::Path::new(&path);
        // `create_new` checks and creates in one step, and never follows a symbolic link.
        let opened = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .create_new(!force)
            .open(file);
        let mut out = match opened {
            Ok(out) => out,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                return Err(CliError::usage(format!(
                    "{path} exists. Add --force to replace it, or choose another path with --download <path>."
                )));
            }
            Err(error) => return Err(CliError::io("write", file, &error)),
        };
        std::io::Write::write_all(&mut out, &pdf)
            .map_err(|error| CliError::io("write", file, &error))?;
        self.success(&format!("Saved the evidence of the acceptance to {path}"));
        Ok(())
    }

    // ------------------------------------------------------------------
    // Step 5: the plan
    // ------------------------------------------------------------------

    /// Step 5: choose the plan, pay in the browser, wait for the payment.
    async fn plan(&mut self, company: &str) -> Result<bool> {
        self.header("plan");
        let billing = self.api().get(&api::BILLING, company).await?;
        let mut options = Vec::new();
        for price in billing["prices"].as_array().cloned().unwrap_or_default() {
            let plan = price["plan"].as_str().unwrap_or_default().to_owned();
            let currency = price["currency"].as_str().unwrap_or("eur");
            let amount = money(price["amount_minor"].as_i64().unwrap_or(0), currency);
            let total = money(price["total_minor"].as_i64().unwrap_or(0), currency);
            let name = if plan == "year" { "Yearly" } else { "Monthly" };
            options.push(Choice {
                label: format!("{name}: {amount} excluding VAT ({total} with VAT)"),
                value: plan,
            });
        }
        if options.is_empty() {
            options = choices(&[("month", "Monthly"), ("year", "Yearly")]);
        }
        let plan = match self.options.plan.clone() {
            Some(plan) => plan,
            None => {
                self.prompter
                    .select("Choose your plan", options, "--plan month|year")
                    .await?
                    .value
            }
        };
        let return_path = format!("/onboarding?company={company}");
        let started = self
            .api()
            .call(
                &api::CHECKOUT,
                &[("company_id", company)],
                &[],
                Some(&json!({"plan": plan, "return_path": return_path})),
            )
            .await?;
        let checkout = started["checkout_id"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        let url = started["checkout_url"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        let company_owned = company.to_owned();
        let result = if url.is_empty() {
            // Embedded Checkout: Stripe runs inside the page of the web app.
            let page = self.link(&return_path);
            self.open("the payment of your plan", &page);
            if self.options.no_wait {
                self.note("Run `oscar onboard` again when the payment is done.");
                return Ok(false);
            }
            self.wait("Waiting for the payment...", move |client| {
                let company = company_owned.clone();
                async move {
                    let state = Api::new(&client)
                        .get(&api::COMPANY_ONBOARDING, &company)
                        .await?;
                    Ok((state["current_step"].as_str() != Some("plan")).then(|| json!("complete")))
                }
            })
            .await?
        } else {
            self.open("the payment of your plan (Stripe Checkout)", &url);
            if self.options.no_wait {
                self.note("Run `oscar onboard` again when the payment is done.");
                return Ok(false);
            }
            self.wait("Waiting for the payment...", move |client| {
                let company = company_owned.clone();
                let checkout = checkout.clone();
                async move {
                    let result = Api::new(&client)
                        .call(
                            &api::CHECKOUT_RESULT,
                            &[("company_id", &company), ("checkout_id", &checkout)],
                            &[],
                            None,
                        )
                        .await?;
                    let paid = result["status"].as_str() == Some("complete")
                        && result["subscription_active"].as_bool() == Some(true);
                    Ok(match result["status"].as_str() {
                        Some("expired") => Some(json!("expired")),
                        _ if paid => Some(json!("complete")),
                        _ => None,
                    })
                }
            })
            .await?
        };
        match result.as_ref().and_then(Value::as_str) {
            Some("complete") => {
                let name = if plan == "year" { "yearly" } else { "monthly" };
                self.success(&format!("The {name} plan is active"));
                Ok(true)
            }
            Some(_) => Err(CliError::failed(
                "The payment page expired. Run `oscar onboard` to start the payment again.",
            )),
            None => {
                self.note(
                    "Still waiting for the payment. Run `oscar onboard` again when it is done.",
                );
                Ok(false)
            }
        }
    }

    // ------------------------------------------------------------------
    // Step 6: set up
    // ------------------------------------------------------------------

    /// Step 6: the checklist, each item with its state, and the page of the web app.
    async fn setup(&mut self, company: &str) -> Result<()> {
        self.header("setup");
        let checklist = self.api().get(&api::SETUP, company).await?;
        for item in checklist["items"].as_array().cloned().unwrap_or_default() {
            let key = item["key"].as_str().unwrap_or_default();
            let status = item["status"].as_str().unwrap_or_default();
            let symbol = match status {
                "done" => theme::paint(theme::Role::Ok, theme::symbol(theme::Sym::Check)),
                "dismissed" => theme::paint(theme::Role::Muted, "-"),
                _ => theme::paint(theme::Role::Warn, theme::symbol(theme::Sym::Dot)),
            };
            let requirement = item["requirement"].as_str().unwrap_or_default();
            let deadline = item["deadline"]
                .as_str()
                .map(|date| format!(", due {date}"))
                .unwrap_or_default();
            self.note(&format!(
                "  {symbol} {:<20} {}",
                setup_name(key),
                theme::paint(
                    theme::Role::Dim,
                    &format!(
                        "{requirement}, {}{deadline}",
                        status.replace('_', " ").replace("todo", "to do")
                    )
                )
            ));
        }
        let page = self.link(&format!("/c/{company}/setup"));
        let open = required_open(&checklist);
        if open.is_empty() {
            self.note(&format!(
                "Do the other items now or later in the web app: {}",
                theme::link(&page, &page)
            ));
            let message = format!("The onboarding of {} is done", self.company_name);
            self.success(&message);
        } else {
            // Never "done" while a required item is open (N2-7): say what is left.
            self.note(&format!(
                "{} Steps 1 to 5 are done. Left to do, required: {}.",
                theme::symbol(theme::Sym::Arrow),
                open.join(", ")
            ));
            self.note(&format!(
                "Do them in the web app: {}",
                theme::link(&page, &page)
            ));
        }
        self.note(NEW_COMPANY_HINT);
        Ok(())
    }
}

/// The Didit Verification Privacy Notice (the web page links the same).
const DIDIT_PRIVACY_URL: &str = "https://didit.me/privacy-policy";

/// The PEP questions of step 3, as the web page asks them.
const PEP_QUESTION: &str = "Do you hold, or have you held, a prominent public function (PEP)?";
const PEP_RELATED_QUESTION: &str = "Is a family member or close associate of yours a PEP?";
const PEP_DETAILS_QUESTION: &str = "Which function, and who holds it?";

/// The flags of the website answer.
const WEBSITE_FLAG: &str = "--website <url> or --no-website";

/// The register activity as the first answer to question 1, in English when the register table
/// has it (the register writes it in Estonian).
fn register_activity(questionnaire: &Value) -> Option<String> {
    let names = &questionnaire["register_activity_names"];
    if let Some(english) = names["en"].as_str().filter(|t| !t.is_empty()) {
        return Some(match names["code"].as_str().filter(|c| !c.is_empty()) {
            Some(code) => format!("{english} (EMTAK {code})"),
            None => english.to_owned(),
        });
    }
    questionnaire["register_activity"]
        .as_str()
        .filter(|t| !t.is_empty())
        .map(str::to_owned)
}

/// The block before the login to a new organization: the consent page cannot preselect it
/// (the authorize request has no hint for it), so the CLI names the one to choose, apart from
/// the other lines and in the warning colour.
pub fn consent_hint(company: &str, other: Option<&str>) -> Vec<String> {
    let theme = theme::current();
    let mut lines = vec![
        String::new(),
        format!(
            "  {}",
            theme.paint_bold(
                theme::Role::Warn,
                &format!(
                    "{} Important: on the consent page, choose the organization \"{company}\".",
                    theme.symbol(theme::Sym::Bang)
                )
            )
        ),
    ];
    if let Some(other) = other {
        lines.push(format!(
            "    {}",
            theme.paint(
                theme::Role::Warn,
                &format!("Not \"{other}\": that login cannot reach the company.")
            )
        ));
    }
    lines.push(String::new());
    lines
}

/// The review of step 2: each question asked, then its answer on the next line, one note per
/// line, so a terminal never loses an answer (N2-3: "No" for a false answer).
pub fn review_lines(answers: &Map<String, Value>, asked: &Value) -> Vec<String> {
    let theme = theme::current();
    let mut lines = Vec::new();
    for question in QUESTIONS {
        if questions::asked(question.key, answers, asked) {
            let value = answers.get(question.key).cloned().unwrap_or(Value::Null);
            lines.push(format!(
                "  {}",
                theme.paint(theme::Role::Dim, question.label)
            ));
            lines.push(format!(
                "    {}",
                theme.bold(&questions::show(question.key, &value))
            ));
        }
    }
    lines
}

/// The required items of the setup checklist that are not done, each with its deadline.
pub fn required_open(checklist: &Value) -> Vec<String> {
    checklist["items"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter(|item| item["requirement"].as_str() == Some("required"))
                .filter(|item| !matches!(item["status"].as_str(), Some("done" | "dismissed")))
                .map(|item| {
                    let name = setup_name(item["key"].as_str().unwrap_or_default());
                    match item["deadline"].as_str() {
                        Some(date) => format!("{name} (due {date})"),
                        None => name.to_owned(),
                    }
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The name of an item of the setup checklist.
pub fn setup_name(key: &str) -> &str {
    match key {
        "people" => "People",
        "migration" => "Move your books",
        "starting_balances" => "Starting balances",
        "bank" => "Connect your bank",
        "inbox" => "Invoice inbox",
        "emta_access" => "e-MTA access",
        "sales_invoices" => "Sales invoices",
        "payroll" => "Payroll",
        other => other,
    }
}

/// A client with the credential of the run, and `GET /api/me/`.
async fn connect(app: &App) -> Result<(Client, Value)> {
    let (client, _) = app.client().await?;
    let me = Api::new(&client).call(&api::ME, &[], &[], None).await?;
    Ok((client, me))
}

/// The name of the organization of a login, from `GET /api/me/`, when it has one.
fn organization_of(me: &Value) -> Option<String> {
    match me["organizations"].as_array()?.as_slice() {
        [one] => one["name"].as_str().map(str::to_owned),
        _ => None,
    }
}

/// `oscar login` with these flags, from the onboarding.
async fn run_login(app: &mut App, flags: &[&str]) -> Result<()> {
    let args: Vec<&str> = ["oscar", "login"].iter().chain(flags).copied().collect();
    let matches = cli::command()
        .try_get_matches_from(args)
        .map_err(|error| CliError::failed(error.to_string()))?;
    let leaf = matches
        .subcommand_matches("login")
        .ok_or_else(|| CliError::failed("No login command."))?;
    login::run(app, leaf).await
}

/// Step 0 with no login: log in, or set up the account and the company in the browser first.
/// The CLI asks whether the person has a company in Oscar: a token is bound to one
/// organization, so an account with none (a sign-up that stopped before step 1) has nothing
/// to log in to, and does step 1 in the browser like a new person (N2-4). A login the consent
/// page denies for that reason takes the same path.
async fn first_login(app: &mut App, prompter: &Prompter, error: &CliError) -> Result<()> {
    let base_url = app.select(&app.config()?)?.base_url;
    let signup = web_link(&base_url, "/onboarding");
    if !prompter.interactive() {
        let mut error = error.clone();
        error.details.push(format!(
            "No company in Oscar yet? Create your account and your company at {signup}, then run `oscar login` and `oscar onboard`."
        ));
        return Err(error);
    }
    let options = choices(&[
        ("login", "Yes: log in"),
        (
            "new",
            "No, not yet (new to Oscar, or the sign-up is not finished): set it up in the browser",
        ),
    ]);
    let choice = prompter
        .select(
            "You are not logged in. Do you already have a company in Oscar?",
            options,
            "--api-key or `oscar login`",
        )
        .await?;
    let mut flags: &[&str] = &[];
    if choice.value == "login" {
        match run_login(app, &[]).await {
            Err(error) if error.message == login::NO_COMPANY => {
                app.out.note(&format!(
                    "{} Your Oscar account has no company yet. Set up the company in the browser first; then the terminal goes on.",
                    theme::symbol(theme::Sym::Bang)
                ));
                // That login was in the browser: the next one is too, with no question.
                flags = &["--browser"];
            }
            other => return other,
        }
    }
    company_in_browser(app, prompter, &signup).await?;
    run_login(app, flags).await
}

/// Step 1 in the browser, for a person with no organization: open the onboarding page, and
/// wait until the person says the company is created.
async fn company_in_browser(app: &mut App, prompter: &Prompter, signup: &str) -> Result<()> {
    let opened = !login::no_display(&app.env) && browser::open(signup, &app.env);
    app.out.note(&format!(
        "{} the onboarding: {signup}",
        if opened { "Opened" } else { "Open" }
    ));
    app.out
        .note("Create your account (if needed) and your company there, then come back.");
    app.out.flush();
    // No default answer, as the other yes or no questions (N3-h): Enter alone asks again.
    let done = prompter
        .yes_no("Is your company created?", None, "`oscar login`")
        .await?;
    if !done {
        return Err(CliError::usage(
            "Run `oscar onboard` again when your company is created.",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_registry_code_and_a_company_are_a_usage_error() {
        let both = Options {
            company: Some("acme".into()),
            registry_code: Some("12345678".into()),
            ..Options::default()
        };
        let error = refuse_company_and_code(&both).unwrap_err();
        assert_eq!(error.code, crate::error::EXIT_USAGE);
        assert!(
            error.message.contains("--registry-code"),
            "{}",
            error.message
        );
        assert!(error.message.contains("-c/--company"), "{}", error.message);
        for one in [
            Options {
                company: Some("acme".into()),
                ..Options::default()
            },
            Options {
                registry_code: Some("12345678".into()),
                ..Options::default()
            },
        ] {
            assert!(refuse_company_and_code(&one).is_ok());
        }
    }

    #[test]
    fn compares_versions() {
        assert_eq!(parse_version("1.2.3"), Some((1, 2, 3)));
        assert_eq!(parse_version("v0.2"), Some((0, 2, 0)));
        assert_eq!(parse_version("1.0.0-beta.1"), Some((1, 0, 0)));
        assert_eq!(parse_version("x"), None);
        assert!(check_version(&json!({"cli_min_version": "0.1.0"}), "0.1.0").is_ok());
        assert!(check_version(&json!({}), "0.1.0").is_ok());
        assert!(check_version(&json!({"cli_min_version": "junk"}), "0.1.0").is_ok());
        let error = check_version(&json!({"cli_min_version": "0.2.0"}), "0.1.9").unwrap_err();
        assert!(
            error.message.contains("needs 0.2.0 or later"),
            "{}",
            error.message
        );
        assert!(error.message.contains("npx @oscaraccountant/cli@latest"));
    }

    #[test]
    fn names_and_formats() {
        assert_eq!(step_name("agreement"), "Accept the client agreement");
        assert_eq!(step_name("zzz"), "zzz");
        assert_eq!(step_number("plan"), 5);
        assert_eq!(money(4900, "eur"), "49.00 EUR");
        assert_eq!(money(-5, "eur"), "-0.05 EUR");
        assert_eq!(
            sha256_hex("abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert!(
            qr_code("https://verify.didit.me/session/x")
                .unwrap()
                .contains('█')
        );
        assert!(qr_code(&"x".repeat(5000)).is_none());
        assert_eq!(setup_name("emta_access"), "e-MTA access");
        assert_eq!(setup_name("new_item"), "new_item");
        assert!(is_registry_code("12345678"));
        assert!(!is_registry_code("1234567a"));
        assert_eq!(role_name("board_member"), "board member");
        assert_eq!(
            person_name(&json!({"person": {"first_name": "Mari", "last_name": "Maasikas"}})),
            "Mari Maasikas"
        );
        assert_eq!(person_name(&json!({})), "A person");
    }

    #[test]
    fn reads_search_hits() {
        let answer = json!({"results": [
            {"registry_code": 12345678, "name": "Acme OÜ", "status": "R", "status_text": "Registrisse kantud", "address": "Tallinn"},
            {"registry_code": "87654321", "name": "Beta AS", "status": "L", "status_text": "Likvideerimisel", "address": null},
            {"registry_code": null, "name": "Broken"},
            {"registry_code": "11111111", "name": "Long OÜ", "address": "Harju maakond, Tallinn, Kesklinna linnaosa, Narva mnt 5, 10117 and a long tail"},
        ]});
        let found = hits(&answer);
        assert_eq!(found.len(), 3);
        assert_eq!(found[0].code, "12345678");
        // English, never the Estonian status text of the register.
        assert_eq!(found[0].to_string(), "Acme OÜ · 12345678 · Tallinn");
        assert_eq!(found[1].to_string(), "Beta AS · 87654321 · In liquidation");
        // A long address is cut: a picker row never wraps.
        assert_eq!(found[2].to_string().chars().count(), HIT_WIDTH);
        assert!(found[2].to_string().ends_with('…'));
        assert_eq!(shorten("abc", 5), "abc");
        assert_eq!(status_name("K"), Some("Deleted"));
        assert_eq!(status_name("?"), None);
        assert_eq!(legal_form_name("ou"), Some("OÜ (private limited company)"));
        assert_eq!(legal_form_name("other"), None);
        assert!(hits(&json!({})).is_empty());
    }

    #[test]
    fn the_review_shows_every_answer() {
        let _theme = theme::scoped(theme::Theme::PLAIN);
        let answers = json!({
            "uses_crypto_assets": false,
            "needs_licence": true,
            "licence_description": "Banking",
            "cash_usage": "no",
            "residence": "all_estonia",
        });
        let lines = review_lines(answers.as_object().unwrap(), &json!({}));
        let text = lines.join("\n");
        assert!(
            text.contains("  Does it hold, accept or pay with crypto-assets?\n    No\n"),
            "{text}"
        );
        assert!(
            text.contains("  Does it need a licence?\n    Yes\n"),
            "{text}"
        );
        assert!(text.contains("  Which licence?\n    Banking\n"), "{text}");
        // A question with no answer yet shows a dash, never an empty line.
        assert!(
            text.contains("  Expected turnover this year\n    -"),
            "{text}"
        );
        assert!(lines.iter().all(|line| !line.trim().is_empty()), "{text}");
    }

    #[test]
    fn an_open_onboarding_says_where_it_is() {
        let me =
            json!({"organizations": [{"id": "o1", "name": "Old OÜ"}], "companies": [{"id": "c1"}]});
        assert!(OpenOnboarding::read(json!({"company": null}), &me).is_none());
        let state = json!({
            "current_step": "identity",
            "company": {"id": "c2", "name": "OÜ Selfi", "registry_code": "17585039",
                        "organization": {"id": "o2", "name": "Selfi Org"}},
        });
        let open = OpenOnboarding::read(state, &me).unwrap();
        assert!(!open.reachable);
        assert_eq!(open.organization_name(), "Selfi Org");
        assert_eq!(
            open.elsewhere_line(),
            "The onboarding of OÜ Selfi (c2) is open in the organization \"Selfi Org\", and this login is for \"Old OÜ\"."
        );
        assert_eq!(open.step_text(), "step 3 (Verify your identity)");
        assert!(open.is_other_code("12345678"));
        assert!(!open.is_other_code(" 17585039 "));
        assert!(
            open.new_company_hint()
                .contains("finish it first with `oscar onboard`")
        );
        // An older server: no registry code and no organization; the company names it.
        let bare = json!({"current_step": "nowhere", "company": {"id": "c1", "name": ""}});
        let open = OpenOnboarding::read(bare, &json!({})).unwrap();
        assert!(!open.reachable);
        assert_eq!(open.name, "your company");
        assert_eq!(open.organization_name(), "your company");
        assert!(open.is_other_code("12345678"));
        assert_eq!(open.step_text(), "an open step");
        assert_eq!(
            open.elsewhere_line(),
            "The onboarding of your company (c1) is open in the organization \"your company\"."
        );
        let mine = json!({"company": {"id": "c1", "name": "A"}});
        assert!(OpenOnboarding::read(mine, &me).unwrap().reachable);
    }

    #[test]
    fn the_consent_hint_stands_out() {
        let _theme = theme::scoped(theme::Theme::PLAIN);
        let lines = consent_hint("OÜ Selfi", Some("Old OÜ"));
        assert_eq!(
            lines,
            [
                "",
                "  ! Important: on the consent page, choose the organization \"OÜ Selfi\".",
                "    Not \"Old OÜ\": that login cannot reach the company.",
                "",
            ]
        );
        assert_eq!(consent_hint("A", None).len(), 3);
    }

    #[test]
    fn required_setup_items_left() {
        let checklist = json!({"items": [
            {"key": "people", "requirement": "required", "status": "in_progress", "deadline": "2026-10-25"},
            {"key": "bank", "requirement": "recommended", "status": "todo", "deadline": null},
            {"key": "emta_access", "requirement": "required", "status": "done", "deadline": null},
            {"key": "payroll", "requirement": "required", "status": "todo", "deadline": null},
            {"key": "inbox", "requirement": "required", "status": "dismissed", "deadline": null},
        ]});
        assert_eq!(
            required_open(&checklist),
            ["People (due 2026-10-25)", "Payroll"]
        );
        assert!(required_open(&json!({})).is_empty());
    }

    #[test]
    fn status_lines_of_a_state() {
        let state = json!({
            "steps": [
                {"key": "account", "status": "done", "minutes": 1, "optional": false},
                {"key": "business", "status": "current", "minutes": 2, "optional": false},
                {"key": "setup", "status": "todo", "minutes": 10, "optional": true},
            ],
            "blocked_reason": "joint_representation",
        });
        let lines = status_lines(&state, &theme::Theme::PLAIN);
        assert_eq!(lines.len(), 4);
        assert!(lines[0].contains("0. Create your account"), "{}", lines[0]);
        assert!(lines[0].trim_end().ends_with("done"), "{}", lines[0]);
        assert!(lines[1].contains("current · about 2 min"), "{}", lines[1]);
        assert!(lines[1].contains("2. About your business"));
        assert!(lines[2].contains("any time"));
        assert!(lines[3].contains("joint_representation"));
        assert!(status_lines(&json!({}), &theme::Theme::PLAIN).is_empty());
    }
}
