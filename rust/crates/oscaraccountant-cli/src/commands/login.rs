//! `oscar login` (`clients/spec/README.md`, sections 4.2 to 4.4): in the browser,
//! with a device code, or with an API key, then the choice of
//! the default company.
//!
//! On a terminal with no flag, the login asks: the server (only when a second profile or server
//! is in play), the method, then the company when the login sees several. Every question has a
//! flag (`--base-url`/`--profile`, `--device`/`--api-key`, `-c/--company`); with no terminal or
//! with `--no-input`, the CLI takes the default of each and never waits.

use std::fmt;
use std::io::Read;
use std::time::SystemTime;

use clap::ArgMatches;
use oscaraccountant::{Client, DEFAULT_BASE_URL};
use serde_json::{Map, Value};

use super::string;
use crate::browser;
use crate::cli::flag;
use crate::company::{self, Found};
use crate::config::{AuthKind, Company, Config, Organization, Profile};
use crate::env::Env;
use crate::error::{CliError, Result};
use crate::loopback::{self, LOGIN_TIMEOUT, Landing, Loopback};
use crate::oauth::{self, AuthorizeRequest, Metadata, TokenResponse};
use crate::prompt::{Prompter, TextOptions};
use crate::render::{
    self,
    theme::{self, Role, Sym},
};
use crate::secrets::Secret;
use crate::session::{self, App};

/// The error of a login by a person whose account has no company yet (the sign-up stopped
/// before step 1): the consent page has nothing to connect. `oscar onboard` matches it.
pub const NO_COMPANY: &str =
    "Your Oscar account has no company yet, so there is nothing to log in to.";

/// The next step after [`NO_COMPANY`], with the onboarding page of `base_url`.
pub fn no_company_hint(base_url: &str) -> String {
    format!(
        "Set up your company in the browser first: {}/onboarding. Then run `oscar login` again, or `oscar onboard`, which goes on in the terminal.",
        base_url.trim_end_matches('/')
    )
}

/// How to log in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    /// The browser and the loopback redirect.
    Browser,
    /// A code to enter on another device.
    Device,
    /// An organization API key.
    ApiKey,
}

impl fmt::Display for Method {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Browser => "In the browser",
            Self::Device => "With a code on another device (a phone, or over SSH)",
            Self::ApiKey => "With an API key",
        })
    }
}

/// A server of the server prompt.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Server {
    /// The server of a profile.
    Profile { name: String, base_url: String },
    /// Another server: the CLI asks for its base URL and a profile name.
    Other,
}

impl fmt::Display for Server {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Profile { name, base_url } => write!(f, "{base_url}  (profile {name})"),
            Self::Other => f.write_str("Another server"),
        }
    }
}

/// Log in, choose the default company, and store both.
pub async fn run(app: &mut App, matches: &ArgMatches) -> Result<()> {
    let leaf = super::leaf(matches);
    let prompter = Prompter::for_app(app);
    let config = app.config()?;
    let mut wanted = app.base_url_override()?;
    let mut explicit = app.explicit_profile();
    if wanted.is_none()
        && explicit.is_none()
        && prompter.interactive()
        && let Some((name, url)) = choose_server(&prompter, &config).await?
    {
        explicit = Some(name);
        wanted = Some(url);
    }
    let name = profile_name(explicit, wanted.as_deref(), &config)?;
    let base_url = match wanted {
        Some(url) => url,
        None => config.profiles.get(&name).map_or_else(
            || DEFAULT_BASE_URL.to_owned(),
            |profile| profile.base_url.clone(),
        ),
    };
    let client = app.anonymous(&base_url)?;
    let method = choose_method(app, &prompter, leaf).await?;
    let (secret, kind) = if method == Method::ApiKey {
        let key = read_api_key(app, &prompter).await?;
        (Secret::ApiKey { api_key: key }, AuthKind::ApiKey)
    } else {
        let scope = oauth::scope(string(leaf, "scope"))?;
        let metadata = oauth::discover(&client).await?;
        let tokens = if method == Method::Device {
            device(app, &prompter, &client, &metadata, scope).await?
        } else {
            loopback(app, &prompter, &client, &metadata, scope).await?
        };
        (
            tokens.into_secret(SystemTime::now(), None)?,
            AuthKind::Oauth,
        )
    };
    let token = match &secret {
        Secret::ApiKey { api_key } => api_key.clone(),
        Secret::OAuth { access_token, .. } => access_token.clone(),
    };
    let me = company::fetch_me(&app.builder(&base_url).api_key(token).build()?).await?;
    let (organization, _) = organization_of(&me);
    let companies = company::from_me(&me);
    let mine = owned(&companies, organization.as_ref());
    let previous = config.profiles.get(&name).and_then(|previous| {
        let same = previous.base_url == base_url
            && previous.organization.as_ref().map(|org| &org.id)
                == organization.as_ref().map(|org| &org.id);
        if same { previous.company.clone() } else { None }
    });
    let chosen = choose_company(
        &prompter,
        string(leaf, "company"),
        &mine,
        previous.as_deref(),
    )
    .await?;
    app.secrets.set(&name, &secret).await?;
    // Read the configuration again: another command may have changed it while the login waited.
    // Only the entry of this profile, and the default when there is none, change.
    let mut config = app.config()?;
    let previous = config.profiles.remove(&name);
    let extra = previous.map(|profile| profile.extra).unwrap_or_default();
    let org_name = organization.as_ref().map(|org| org.name.clone());
    config.profiles.insert(
        name.clone(),
        Profile {
            base_url: base_url.clone(),
            auth: kind,
            organization,
            company: chosen.company.as_ref().map(|company| company.id.clone()),
            companies: Some(companies),
            extra,
        },
    );
    if config.default_name().is_none() || config.default_profile.is_none() {
        config.default_profile = Some(name.clone());
    }
    config.save(&app.paths)?;
    let email = me
        .pointer("/user/email")
        .and_then(Value::as_str)
        .unwrap_or("an API key");
    app.out.note(&render::success_text(&logged_in(
        email,
        org_name.as_deref(),
        chosen.company.as_ref(),
    )));
    // The hint follows the line it explains (`· no company`).
    if let Some(note) = &chosen.note {
        app.out.note(note);
    }
    Ok(())
}

/// The last line of a login: `Logged in as <email> · <organization> · company <name>`.
pub fn logged_in(email: &str, organization: Option<&str>, company: Option<&Company>) -> String {
    let dot = theme::symbol(Sym::Middot);
    let mut line = format!("Logged in as {}", theme::current().bold(email));
    if let Some(organization) = organization.filter(|name| !name.is_empty()) {
        line.push_str(&format!(" {dot} {organization}"));
    }
    match company {
        Some(company) => line.push_str(&format!(
            " {dot} company {}",
            theme::paint(Role::Accent, &company.name)
        )),
        None => line.push_str(&format!(" {dot} no company")),
    }
    line
}

/// The companies of the organization of the login; all of them when the login has no single
/// organization (a staff key).
fn owned(companies: &[Company], organization: Option<&Organization>) -> Vec<Company> {
    companies
        .iter()
        .filter(|company| {
            organization.is_none_or(|org| {
                company.organization_id.is_empty() || company.organization_id == org.id
            })
        })
        .cloned()
        .collect()
}

/// The default company a login chose, and a note on how to choose one when it chose none.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chosen {
    /// The company.
    pub company: Option<Company>,
    /// What the person should know: no company yet, or how to choose among several.
    pub note: Option<String>,
}

/// Choose the default company: `-c/--company`; the only one; a picker on a terminal; else the
/// previous default when it is still there. None: the CLI offers `oscar onboard`.
pub async fn choose_company(
    prompter: &Prompter,
    flag: Option<&str>,
    companies: &[Company],
    previous: Option<&str>,
) -> Result<Chosen> {
    let chosen = |company: Option<&Company>, note: Option<String>| Chosen {
        company: company.cloned(),
        note,
    };
    if let Some(value) = flag.filter(|value| !value.trim().is_empty()) {
        return match company::find(value, companies) {
            Found::One(company) => Ok(chosen(Some(company), None)),
            Found::Many(matches) => Err(company::ambiguous(value, &matches)),
            Found::None => Err(company::not_found(value)),
        };
    }
    let arrow = theme::symbol(Sym::Arrow);
    match companies {
        [] => Ok(chosen(
            None,
            Some(format!(
                "{arrow} No company yet. Run `oscar onboard` to set up your first company."
            )),
        )),
        [one] => Ok(chosen(Some(one), None)),
        several if prompter.interactive() => {
            let start = previous
                .and_then(|id| several.iter().position(|company| company.id == id))
                .unwrap_or(0);
            let options = company::choices(several);
            let picked = prompter
                .select_at("Default company", options, start, "-c/--company <x>")
                .await?;
            Ok(chosen(Some(&picked.company), None))
        }
        several => {
            let kept = previous.and_then(|id| several.iter().find(|company| company.id == id));
            if kept.is_some() {
                return Ok(chosen(kept, None));
            }
            Ok(chosen(
                None,
                Some(format!(
                    "{arrow} You have {} companies. Choose the default one: `oscar companies use <x>` \
                     (a name, a registry code or an id), or log in with -c/--company <x>.",
                    several.len()
                )),
            ))
        }
    }
}

/// The method: `--api-key`, `--device`; else the method prompt on a terminal; else the browser,
/// or the device code when no display is found.
async fn choose_method(app: &App, prompter: &Prompter, leaf: &ArgMatches) -> Result<Method> {
    if flag(leaf, "api-key") {
        return Ok(Method::ApiKey);
    }
    if flag(leaf, "device") {
        return Ok(Method::Device);
    }
    if flag(leaf, "browser") {
        return Ok(Method::Browser);
    }
    let default = if no_display(&app.env) {
        Method::Device
    } else {
        Method::Browser
    };
    if !prompter.interactive() {
        return Ok(default);
    }
    let mut options = vec![Method::Browser, Method::Device, Method::ApiKey];
    options.retain(|method| *method != default);
    options.insert(0, default);
    prompter
        .select(
            "How do you want to log in?",
            options,
            "--device or --api-key",
        )
        .await
}

/// The server prompt, only when a second server is in play: two profiles or more, or one
/// profile of a server that is not the default one. `None`: no prompt.
async fn choose_server(prompter: &Prompter, config: &Config) -> Result<Option<(String, String)>> {
    let in_play = config.profiles.len() >= 2
        || config
            .profiles
            .values()
            .any(|profile| profile.base_url != DEFAULT_BASE_URL);
    if !in_play {
        return Ok(None);
    }
    let mut options: Vec<Server> = config
        .profiles
        .iter()
        .map(|(name, profile)| Server::Profile {
            name: name.clone(),
            base_url: profile.base_url.clone(),
        })
        .collect();
    let start = config
        .default_name()
        .and_then(|default| {
            options.iter().position(
                |server| matches!(server, Server::Profile { name, .. } if name == default),
            )
        })
        .unwrap_or(0);
    options.push(Server::Other);
    let flag = "--base-url <url> or --profile <name>";
    match prompter.select_at("Server", options, start, flag).await? {
        Server::Profile { name, base_url } => Ok(Some((name, base_url))),
        Server::Other => {
            let url = prompter
                .text(
                    "Base URL",
                    TextOptions {
                        default: Some(DEFAULT_BASE_URL.to_owned()),
                        ..TextOptions::default()
                    },
                    "--base-url <url>",
                )
                .await?;
            let url = session::normalize(&url)?;
            if let Some(name) = config.name_for_base_url(&url) {
                return Ok(Some((name.to_owned(), url)));
            }
            let name = prompter
                .text(
                    "Profile name",
                    TextOptions {
                        default: Some(suggested_profile(&url, config)),
                        ..TextOptions::default()
                    },
                    "--profile <name>",
                )
                .await?;
            if config.profiles.contains_key(&name) {
                return Err(CliError::usage(format!(
                    "The profile `{name}` is for another server. Choose another name."
                )));
            }
            Ok(Some((name, url)))
        }
    }
}

/// A profile name for a new server: `default` when free, else the host of the URL.
fn suggested_profile(url: &str, config: &Config) -> String {
    if !config.profiles.contains_key("default") {
        return "default".to_owned();
    }
    url::Url::parse(url)
        .ok()
        .and_then(|url| url.host_str().map(str::to_owned))
        .unwrap_or_else(|| "other".to_owned())
}

/// The profile a login writes: `--profile` or `OSCAR_PROFILE`; else, for a `--base-url`, the
/// profile of that base URL; else the default profile, or `default`. A profile never serves
/// another base URL, so a login to another server with no profile given is refused rather than
/// replacing the default profile.
pub fn profile_name(
    explicit: Option<String>,
    wanted: Option<&str>,
    config: &Config,
) -> Result<String> {
    if let Some(name) = explicit {
        return Ok(name);
    }
    if let Some(url) = wanted
        && let Some(name) = config.name_for_base_url(url)
    {
        return Ok(name.to_owned());
    }
    let name = config.default_name().unwrap_or("default");
    if let (Some(url), Some(profile)) = (wanted, config.profiles.get(name))
        && profile.base_url != url
    {
        return Err(CliError::usage(format!(
            "The profile `{name}` is for {}, not {url}. Give --profile <name> to log in to {url}.",
            profile.base_url
        )));
    }
    Ok(name.to_owned())
}

/// No display: the device login is the default (spec section 4.3). On Linux when neither
/// `DISPLAY` nor `WAYLAND_DISPLAY` is set, and anywhere over SSH.
pub fn no_display(env: &Env) -> bool {
    env.var("SSH_CONNECTION").is_some()
        || (cfg!(target_os = "linux")
            && env.var("DISPLAY").is_none()
            && env.var("WAYLAND_DISPLAY").is_none())
}

/// The API key: a prompt that does not echo on a terminal, else standard input when it is not a
/// terminal. A terminal with `--no-input` is a usage error: the CLI does not ask.
async fn read_api_key(app: &App, prompter: &Prompter) -> Result<String> {
    let key = if prompter.interactive() {
        prompter
            .secret("API key", "the key on standard input")
            .await?
    } else if !app.env.stdin_tty {
        let mut text = String::new();
        std::io::stdin()
            .read_to_string(&mut text)
            .map_err(|error| CliError::usage(format!("Cannot read the API key: {error}")))?;
        text
    } else {
        return Err(prompter.missing(
            "it on standard input (`oscar login --api-key < key.txt`)",
            "An API key",
        ));
    };
    let key = key.trim().to_owned();
    if key.is_empty() {
        return Err(CliError::usage("No API key given."));
    }
    Ok(key)
}

/// The loopback login: the browser, and the redirect to a local port. The browser lands on the
/// page `/cli/done` of the web app when the server has it.
async fn loopback(
    app: &mut App,
    prompter: &Prompter,
    client: &Client,
    metadata: &Metadata,
    scope: &str,
) -> Result<TokenResponse> {
    let pages = loopback::has_pages(client).await;
    let listener = Loopback::bind().await?;
    let redirect_uri = listener.redirect_uri.clone();
    let verifier = oauth::random_token()?;
    let state = oauth::random_token()?;
    let resource = oauth::resource(client.base_url());
    let url = oauth::authorize_url(
        metadata,
        &AuthorizeRequest {
            redirect_uri: &redirect_uri,
            scope,
            state: &state,
            challenge: &oauth::challenge(&verifier),
            resource: &resource,
        },
    )?;
    if browser::open(&url, &app.env) {
        // The URL too: the browser may open on another screen, or not at all.
        app.out.note(&format!(
            "Opening the browser to log in. If it does not open, open this URL:\n\n    {url}\n"
        ));
    } else {
        app.out.note(&format!(
            "Open this URL in a browser to log in:\n\n    {url}\n"
        ));
    }
    app.out.flush();
    let landing = Landing {
        pages: pages.then(|| client.base_url().trim_end_matches('/').to_owned()),
        state: state.clone(),
        issuer: metadata.issuer.clone(),
    };
    let spinner = prompter.spinner("Waiting for the browser...");
    let params = listener.wait(LOGIN_TIMEOUT, landing).await;
    spinner.finish();
    let code = check_callback(&params?, &state, &metadata.issuer).map_err(|mut error| {
        if error.message == NO_COMPANY {
            error.details.push(no_company_hint(client.base_url()));
        }
        error
    })?;
    oauth::exchange_code(client, metadata, &code, &redirect_uri, &verifier).await
}

/// Check the redirect (spec section 4.2, step 5) and give its code.
pub fn check_callback(params: &[(String, String)], state: &str, issuer: &str) -> Result<String> {
    let get = |name: &str| {
        params
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    };
    if get("state") != Some(state) {
        return Err(CliError::auth(
            "The answer of the browser has another state; the login stopped.",
        ));
    }
    if get("iss").map(|iss| iss.trim_end_matches('/')) != Some(issuer.trim_end_matches('/')) {
        return Err(CliError::auth(
            "The answer of the browser has another issuer; the login stopped.",
        ));
    }
    if let Some(error) = get("error") {
        if error == "access_denied" && get("reason") == Some(loopback::NO_ORGANIZATION) {
            return Err(CliError::auth(NO_COMPANY));
        }
        let described = match get("error_description") {
            Some(description) => format!("{error}: {description}"),
            None => error.to_owned(),
        };
        return Err(CliError::auth(format!("The login failed: {described}")));
    }
    get("code")
        .map(str::to_owned)
        .ok_or_else(|| CliError::auth("The answer of the browser has no code."))
}

/// The device login: a link that carries the code (the browser shows it, the user checks it),
/// the link as a QR code on a terminal, the code to type when there is no link,
/// and a spinner while the CLI waits.
async fn device(
    app: &mut App,
    prompter: &Prompter,
    client: &Client,
    metadata: &Metadata,
    scope: &str,
) -> Result<TokenResponse> {
    let started = oauth::device_authorize(client, metadata, scope).await?;
    let code = theme::current().bold(&started.user_code);
    // The link carries the code: the browser shows it, and the user checks it against this one
    // (SEC-070). Without a link, the user types the code.
    let target = started
        .verification_uri_complete
        .as_deref()
        .unwrap_or(&started.verification_uri);
    if started.verification_uri_complete.is_some() {
        app.out.note(&format!("To log in, open {target}"));
    } else {
        app.out.note(&format!(
            "To log in, open {target} and enter the code {code}"
        ));
    }
    if app.env.stderr_tty
        && theme::current().unicode
        && let Some(qr) = qr_code(target)
    {
        app.out
            .note(&format!("or scan this QR code with a phone:\n{qr}"));
    }
    if started.verification_uri_complete.is_some() {
        app.out.note(&format!(
            "Check that the browser shows the same code: {code}"
        ));
        app.out.note(&format!(
            "(Or open {} and enter the code there.)",
            started.verification_uri
        ));
    }
    if !app.env.stderr_tty {
        app.out.note("Waiting for the approval...");
    }
    app.out.flush();
    let spinner = prompter.spinner("Waiting for the approval...");
    let tokens = oauth::poll_device(client, metadata, &started, tokio::time::sleep).await;
    spinner.finish();
    tokens.map_err(|mut error| {
        // The device page cannot say why it was denied: name the usual cause.
        if error.message == oauth::DENIED {
            error.details.push(format!(
                "If your account has no company yet: {}",
                no_company_hint(client.base_url())
            ));
        }
        error
    })
}

/// A QR code of the text in Unicode half blocks, light on dark so a phone reads it on a dark
/// terminal too. `None` when the text is too long for a QR code.
pub fn qr_code(text: &str) -> Option<String> {
    use qrcode::render::unicode::Dense1x2;
    let code = qrcode::QrCode::new(text.as_bytes()).ok()?;
    Some(
        code.render::<Dense1x2>()
            .dark_color(Dense1x2::Light)
            .light_color(Dense1x2::Dark)
            .quiet_zone(true)
            .build(),
    )
}

/// The organization of the credential, and the company when the organization has exactly one.
pub fn organization_of(me: &Value) -> (Option<Organization>, Option<String>) {
    let list = |key: &str| {
        me.get(key)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    };
    let (organizations, companies) = (list("organizations"), list("companies"));
    let id_of =
        |value: &Value, key: &str| value.get(key).and_then(Value::as_str).map(str::to_owned);
    let chosen = if organizations.len() == 1 {
        organizations.first().cloned()
    } else {
        let mut ids: Vec<String> = companies
            .iter()
            .filter_map(|c| id_of(c, "organization_id"))
            .collect();
        ids.dedup();
        match ids.as_slice() {
            [only] => organizations
                .iter()
                .find(|org| id_of(org, "id").as_deref() == Some(only))
                .cloned(),
            _ => None,
        }
    };
    let Some(chosen) = chosen else {
        return (None, None);
    };
    let Some(id) = id_of(&chosen, "id") else {
        return (None, None);
    };
    let organization = Organization {
        name: id_of(&chosen, "name").unwrap_or_default(),
        id: id.clone(),
        extra: Map::new(),
    };
    let mut owned = companies
        .iter()
        .filter(|company| id_of(company, "organization_id").is_none_or(|org| org == id));
    let company = match (owned.next(), owned.next()) {
        (Some(only), None) => id_of(only, "id"),
        _ => None,
    };
    (Some(organization), company)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::company::tests::company;

    fn pairs(items: &[(&str, &str)]) -> Vec<(String, String)> {
        items
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    fn quiet() -> Prompter {
        Prompter::new(&Env::with_vars(&[]), false)
    }

    /// The login the onboarding runs again after step 1 asks no method (P2-18).
    #[tokio::test]
    async fn the_browser_flag_asks_no_method() {
        let dir = tempfile::tempdir().unwrap();
        let app = crate::session::tests::app(dir.path(), &[]);
        let mut env = Env::with_vars(&[]);
        env.stdin_tty = true;
        env.stderr_tty = true;
        let interactive = Prompter::new(&env, false);
        let matches = crate::cli::command()
            .try_get_matches_from(["oscar", "login", "--browser"])
            .unwrap();
        let leaf = matches.subcommand_matches("login").unwrap();
        assert_eq!(
            choose_method(&app, &interactive, leaf).await.unwrap(),
            Method::Browser
        );
        assert!(
            crate::cli::command()
                .try_get_matches_from(["oscar", "login", "--browser", "--device"])
                .is_err()
        );
    }

    #[test]
    fn checks_the_callback() {
        let issuer = "https://x.test";
        let good = pairs(&[("code", "c"), ("state", "s"), ("iss", "https://x.test")]);
        assert_eq!(check_callback(&good, "s", issuer).unwrap(), "c");
        let cases = [
            (
                pairs(&[("code", "c"), ("state", "t"), ("iss", issuer)]),
                "another state",
            ),
            (pairs(&[("code", "c"), ("state", "s")]), "another issuer"),
            (
                pairs(&[("code", "c"), ("state", "s"), ("iss", "https://evil.test")]),
                "another issuer",
            ),
            (pairs(&[("state", "s"), ("iss", issuer)]), "no code"),
            (
                pairs(&[
                    ("error", "access_denied"),
                    ("error_description", "No."),
                    ("state", "s"),
                    ("iss", issuer),
                ]),
                "The login failed: access_denied: No.",
            ),
            (
                pairs(&[("error", "access_denied"), ("state", "s"), ("iss", issuer)]),
                "failed: access_denied",
            ),
            (
                pairs(&[
                    ("error", "access_denied"),
                    ("reason", "no_organization"),
                    ("state", "s"),
                    ("iss", issuer),
                ]),
                NO_COMPANY,
            ),
        ];
        for (params, message) in cases {
            let error = check_callback(&params, "s", issuer).unwrap_err();
            assert_eq!(error.code, crate::error::EXIT_AUTH);
            assert!(error.message.contains(message), "{}", error.message);
        }
    }

    #[test]
    fn detects_a_display() {
        assert!(no_display(&Env::with_vars(&[
            ("SSH_CONNECTION", "1 2 3 4"),
            ("DISPLAY", ":0")
        ])));
        assert!(!no_display(&Env::with_vars(&[("DISPLAY", ":0")])));
        assert!(!no_display(&Env::with_vars(&[(
            "WAYLAND_DISPLAY",
            "wayland-1"
        )])));
        assert_eq!(no_display(&Env::with_vars(&[])), cfg!(target_os = "linux"));
    }

    #[test]
    fn finds_the_organization() {
        let org = |id: &str| json!({"id": id, "name": format!("Org {id}")});
        let company = |id: &str, org: &str| json!({"id": id, "organization_id": org});
        let me = json!({"organizations": [org("o1")], "companies": [company("c1", "o1")]});
        let (organization, company_id) = organization_of(&me);
        assert_eq!(organization.unwrap().name, "Org o1");
        assert_eq!(company_id.as_deref(), Some("c1"));
        let me = json!({"organizations": [org("o1")], "companies": [company("c1", "o1"), company("c2", "o1")]});
        assert_eq!(organization_of(&me).1, None);
        let me =
            json!({"organizations": [org("o1"), org("o2")], "companies": [company("c2", "o2")]});
        let (organization, company_id) = organization_of(&me);
        assert_eq!(organization.unwrap().id, "o2");
        assert_eq!(company_id.as_deref(), Some("c2"));
        let me = json!({"organizations": [org("o1"), org("o2")], "companies": []});
        assert_eq!(organization_of(&me).0, None);
        assert_eq!(
            organization_of(&json!({"organizations": [{"name": "x"}]})).0,
            None
        );
        assert_eq!(organization_of(&json!({})).0, None);
    }

    #[test]
    fn keeps_the_companies_of_the_organization() {
        let mut other = company("c2", "B", "2");
        other.organization_id = "org2".into();
        let mut loose = company("c3", "C", "3");
        loose.organization_id.clear();
        let all = vec![company("c1", "A", "1"), other, loose];
        let org = Organization {
            id: "org1".into(),
            name: "Org".into(),
            extra: Map::new(),
        };
        let ids = |list: Vec<Company>| list.into_iter().map(|c| c.id).collect::<Vec<_>>();
        assert_eq!(ids(owned(&all, Some(&org))), ["c1", "c3"]);
        assert_eq!(ids(owned(&all, None)).len(), 3);
    }

    #[tokio::test]
    async fn chooses_the_company_without_a_terminal() {
        let prompter = quiet();
        let none = choose_company(&prompter, None, &[], None).await.unwrap();
        assert_eq!(none.company, None);
        assert!(none.note.unwrap().contains("oscar onboard"));
        let one = [company("c1", "Alpha OÜ", "1")];
        let chosen = choose_company(&prompter, None, &one, None).await.unwrap();
        assert_eq!(chosen.company.unwrap().id, "c1");
        assert_eq!(chosen.note, None);
        let several = [
            company("c1", "Alpha OÜ", "1"),
            company("c2", "Beta AS", "2"),
        ];
        // No terminal: no choice, and a note that says how to choose.
        let chosen = choose_company(&prompter, None, &several, None)
            .await
            .unwrap();
        assert_eq!(chosen.company, None);
        assert!(chosen.note.unwrap().contains("oscar companies use <x>"));
        // The previous default stays when it is still there.
        let chosen = choose_company(&prompter, None, &several, Some("c2"))
            .await
            .unwrap();
        assert_eq!(chosen.company.unwrap().id, "c2");
        let chosen = choose_company(&prompter, None, &several, Some("gone"))
            .await
            .unwrap();
        assert_eq!(chosen.company, None);
        // The flag wins, by name, code or id.
        let chosen = choose_company(&prompter, Some("beta"), &several, Some("c1"))
            .await
            .unwrap();
        assert_eq!(chosen.company.unwrap().id, "c2");
        let error = choose_company(&prompter, Some("zeta"), &several, None)
            .await
            .unwrap_err();
        assert_eq!(error.code, crate::error::EXIT_USAGE);
        let twins = [company("c1", "Alpha 1", "1"), company("c2", "Alpha 2", "2")];
        let error = choose_company(&prompter, Some("alpha"), &twins, None)
            .await
            .unwrap_err();
        assert_eq!(error.details.len(), 2);
    }

    #[tokio::test]
    async fn the_picker_is_a_usage_error_with_no_input() {
        let mut env = Env::with_vars(&[]);
        env.stdin_tty = true;
        env.stderr_tty = true;
        let prompter = Prompter::new(&env, true);
        let several = [company("c1", "A", "1"), company("c2", "B", "2")];
        // --no-input on a terminal: the same as no terminal, never a wait.
        let chosen = choose_company(&prompter, None, &several, None)
            .await
            .unwrap();
        assert_eq!(chosen.company, None);
        let config = Config::default();
        assert_eq!(choose_server(&prompter, &config).await.unwrap(), None);
    }

    #[test]
    fn the_last_line() {
        let _plain = theme::scoped(theme::Theme::PLAIN);
        let alpha = company("c1", "Alpha OÜ", "1");
        assert_eq!(
            logged_in("ann@example.com", Some("Org"), Some(&alpha)),
            "Logged in as ann@example.com · Org · company Alpha OÜ"
        );
        assert_eq!(
            logged_in("an API key", None, None),
            "Logged in as an API key · no company"
        );
        assert_eq!(
            logged_in("a@b", Some(""), None),
            "Logged in as a@b · no company"
        );
    }

    #[test]
    fn names_and_labels() {
        assert_eq!(Method::Browser.to_string(), "In the browser");
        assert!(Method::Device.to_string().starts_with("With a code"));
        assert_eq!(Method::ApiKey.to_string(), "With an API key");
        let server = Server::Profile {
            name: "dev".into(),
            base_url: "http://x.test".into(),
        };
        assert_eq!(server.to_string(), "http://x.test  (profile dev)");
        assert_eq!(Server::Other.to_string(), "Another server");
        let mut config = Config::default();
        assert_eq!(suggested_profile("https://x.test", &config), "default");
        config.profiles.insert(
            "default".into(),
            Profile {
                base_url: DEFAULT_BASE_URL.into(),
                auth: AuthKind::Oauth,
                organization: None,
                company: None,
                companies: None,
                extra: Map::new(),
            },
        );
        assert_eq!(
            suggested_profile("https://dev.x.test:8300", &config),
            "dev.x.test"
        );
        assert_eq!(suggested_profile("not a url", &config), "other");
    }

    #[test]
    fn draws_a_qr_code() {
        let qr = qr_code("https://oscaraccountant.com/oauth-device?user_code=ABCD-EFGH").unwrap();
        assert!(qr.lines().count() > 10);
        assert!(qr.contains('█') || qr.contains('▀') || qr.contains('▄'));
        assert_eq!(qr_code(&"x".repeat(8000)), None);
    }
}
