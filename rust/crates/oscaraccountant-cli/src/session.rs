//! One run of the CLI: its environment, its output, the store, and the choice of the profile, the
//! base URL and the credential of a call.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use oscaraccountant::{Client, ClientBuilder, DEFAULT_BASE_URL, TlsRoots, normalize_base_url};

use crate::company;
use crate::config::{Config, Paths, Profile};
use crate::env::Env;
use crate::error::{CliError, LOGIN_HINT, Result};
use crate::oauth;
use crate::output::Output;
use crate::secrets::{Secret, Secrets};
use crate::time;

/// The version of the CLI, equal to `clients/VERSION`.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// A token that expires within this margin is refreshed first.
pub const REFRESH_MARGIN: Duration = Duration::from_secs(60);

/// The options every command takes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Globals {
    /// `--json`.
    pub json: bool,
    /// `--profile`.
    pub profile: Option<String>,
    /// `--base-url`.
    pub base_url: Option<String>,
    /// `--api-key`, for this call only.
    pub api_key: Option<String>,
    /// The output options: `-o`, `--columns`, `--json=fields`, `--jq`, `--template`.
    pub output: crate::render::Options,
    /// `--no-input` (or `OSCAR_NO_INPUT=1`, read by `prompt::Prompter`): never ask.
    pub no_input: bool,
}

/// The profile of a call, and its base URL.
#[derive(Debug, Clone, PartialEq)]
pub struct Selection {
    /// The name of the profile, when one applies.
    pub name: Option<String>,
    /// The profile, when it exists.
    pub profile: Option<Profile>,
    /// The normalized base URL of the call.
    pub base_url: String,
    /// Why the stored credential of the profile may not be used, when it may not.
    pub refused: Option<String>,
    /// The company `-c/--company` or `OSCAR_COMPANY` resolved to (`App::with_company`).
    pub company: Option<String>,
}

/// The credential of a call.
#[derive(Clone)]
pub enum Credential {
    /// An API key: `--api-key`, `OSCAR_API_KEY`, or the profile's.
    ApiKey(String),
    /// The OAuth login of the profile.
    OAuth(Arc<OAuthSession>),
}

/// One run.
pub struct App {
    /// The environment.
    pub env: Env,
    /// The output.
    pub out: Output,
    /// Where the files are.
    pub paths: Paths,
    /// The secrets.
    pub secrets: Secrets,
    /// The options every command takes.
    pub globals: Globals,
}

impl App {
    /// JSON output: `--json`, or standard output is not a terminal.
    pub fn json(&self) -> bool {
        self.globals.json || !self.out.stdout_tty
    }

    /// Read `config.json`.
    pub fn config(&self) -> Result<Config> {
        Config::load(&self.paths)
    }

    /// `--profile`, else `OSCAR_PROFILE`.
    pub fn explicit_profile(&self) -> Option<String> {
        self.globals
            .profile
            .clone()
            .filter(|name| !name.is_empty())
            .or_else(|| self.env.var("OSCAR_PROFILE").map(str::to_owned))
    }

    /// `--base-url`, else `OSCAR_BASE_URL`, normalized.
    pub fn base_url_override(&self) -> Result<Option<String>> {
        let value = self
            .globals
            .base_url
            .as_deref()
            .filter(|url| !url.is_empty())
            .or_else(|| self.env.var("OSCAR_BASE_URL"));
        value.map(normalize).transpose()
    }

    /// Choose the profile and the base URL of a call (`clients/spec/README.md`, sections 1.3 and
    /// 3.1). A profile never lends its credential to another base URL.
    pub fn select(&self, config: &Config) -> Result<Selection> {
        let wanted = self.base_url_override()?;
        let name = match self.explicit_profile() {
            Some(name) => Some(name),
            None => match &wanted {
                Some(url) => config.name_for_base_url(url).map(str::to_owned),
                None => config.default_name().map(str::to_owned),
            },
        };
        let profile = name
            .as_ref()
            .and_then(|name| config.profiles.get(name))
            .cloned();
        let refused = match (&name, &profile, &wanted) {
            (Some(name), None, _) => Some(format!(
                "No profile `{name}`. Run `oscar login --profile {name}`."
            )),
            (Some(name), Some(profile), Some(url)) if &profile.base_url != url => Some(format!(
                "The profile `{name}` is for {}, not {url}.",
                profile.base_url
            )),
            _ => None,
        };
        let base_url = match (wanted, &profile) {
            (Some(url), _) => url,
            (None, Some(profile)) => profile.base_url.clone(),
            (None, None) => DEFAULT_BASE_URL.to_owned(),
        };
        Ok(Selection {
            name,
            profile,
            base_url,
            refused,
            company: None,
        })
    }

    /// A client builder for a base URL: the CLI's `User-Agent`, the platform's certificates,
    /// and no environment (the CLI chose the credential and the base URL already).
    pub fn builder(&self, base_url: &str) -> ClientBuilder {
        Client::builder()
            .base_url(base_url)
            .env(|_| None)
            .user_agent(oscaraccountant::user_agent("oscar-cli", VERSION))
            .tls_roots(TlsRoots::Platform)
    }

    /// A client with no credential, for the OAuth endpoints.
    pub fn anonymous(&self, base_url: &str) -> Result<Client> {
        Ok(self.builder(base_url).build()?)
    }

    /// The credential of a call, in the order of section 1.1: `--api-key`, `OSCAR_API_KEY`,
    /// then the profile.
    pub async fn credential(&self, selection: &Selection) -> Result<Credential> {
        if let Some(key) = self
            .globals
            .api_key
            .as_deref()
            .filter(|key| !key.is_empty())
        {
            return Ok(Credential::ApiKey(key.to_owned()));
        }
        if let Some(key) = self.env.var("OSCAR_API_KEY") {
            return Ok(Credential::ApiKey(key.to_owned()));
        }
        self.stored(selection).await
    }

    /// The stored credential of the selected profile, with no `--api-key` and no
    /// `OSCAR_API_KEY`: what `oscar auth token` answers with.
    pub async fn stored(&self, selection: &Selection) -> Result<Credential> {
        if let Some(refused) = &selection.refused {
            return Err(CliError::auth(refused.clone()));
        }
        let Some(name) = &selection.name else {
            return Err(CliError::auth(format!(
                "Not logged in to {}. {LOGIN_HINT}",
                selection.base_url
            )));
        };
        match self.secrets.get(name).await? {
            Some(Secret::ApiKey { api_key }) => Ok(Credential::ApiKey(api_key)),
            Some(secret @ Secret::OAuth { .. }) => Ok(Credential::OAuth(Arc::new(OAuthSession {
                profile: name.clone(),
                client: self.anonymous(&selection.base_url)?,
                secrets: self.secrets.clone(),
                secret: tokio::sync::Mutex::new((secret, false)),
            }))),
            None => Err(CliError::auth(format!(
                "The profile `{name}` has no stored credential. Run `oscar login`."
            ))),
        }
    }

    /// A client with the credential of the call, and the selection it was made for. An OAuth
    /// token is refreshed first when it expires soon, so a failed refresh ends the command with
    /// its own exit code.
    pub async fn client(&self) -> Result<(Client, Selection)> {
        let selection = self.select(&self.config()?)?;
        Ok((self.client_for(&selection).await?, selection))
    }

    /// [`client`](Self::client), for a selection the caller made first.
    pub async fn client_for(&self, selection: &Selection) -> Result<Client> {
        let builder = self.builder(&selection.base_url);
        let builder = match self.credential(selection).await? {
            Credential::ApiKey(key) => builder.api_key(key),
            Credential::OAuth(session) => {
                session.token().await?;
                builder.token_provider(move || {
                    let session = session.clone();
                    async move { session.token().await.map_err(|error| error.message) }
                })
            }
        };
        Ok(builder.build()?)
    }

    /// The company of a command: the one [`with_company`](Self::with_company) resolved, else
    /// `--company`, else `OSCAR_COMPANY`, else the profile's.
    pub fn company(&self, flag: Option<&str>, selection: &Selection) -> Result<String> {
        if let Some(company) = &selection.company {
            return Ok(company.clone());
        }
        if let Some(company) = self
            .wanted_company(flag)
            .or_else(|| selection.profile.as_ref()?.company.as_deref())
        {
            return Ok(company.to_owned());
        }
        // With no credential at all, the login is what is missing, not the company.
        self.logged_out(selection)?;
        Err(CliError::usage(format!(
            "No company. {}",
            company::CHOOSE_HINT
        )))
    }

    /// The authentication error of a call that has no credential to use: no `--api-key`, no
    /// `OSCAR_API_KEY`, and no profile (or a profile that may not be used here).
    fn logged_out(&self, selection: &Selection) -> Result<()> {
        let key = self
            .globals
            .api_key
            .as_deref()
            .is_some_and(|key| !key.is_empty())
            || self.env.var("OSCAR_API_KEY").is_some();
        if key {
            return Ok(());
        }
        if let Some(refused) = &selection.refused {
            return Err(CliError::auth(refused.clone()));
        }
        if selection.name.is_none() {
            return Err(CliError::auth(format!(
                "Not logged in to {}. {LOGIN_HINT}",
                selection.base_url
            )));
        }
        Ok(())
    }

    /// `-c/--company`, else `OSCAR_COMPANY`.
    fn wanted_company<'a>(&'a self, flag: Option<&'a str>) -> Option<&'a str> {
        flag.filter(|value| !value.trim().is_empty())
            .or_else(|| self.env.var("OSCAR_COMPANY"))
    }

    /// Resolve `-c/--company` or `OSCAR_COMPANY` (an id, a registry code, or the start of a
    /// name) to the id of a company, into `selection.company`. The cache of the profile answers
    /// first; when it does not know the value, `GET /api/me/` is read and the cache stored again.
    /// With neither given, the profile's default company is used as it is.
    pub async fn with_company(
        &self,
        mut selection: Selection,
        flag: Option<&str>,
    ) -> Result<Selection> {
        let Some(wanted) = self.wanted_company(flag).map(str::to_owned) else {
            return Ok(selection);
        };
        let cache = selection
            .profile
            .as_ref()
            .and_then(|profile| profile.companies.clone());
        if let Some(cache) = &cache
            && let company::Found::One(found) = company::find(&wanted, cache)
        {
            selection.company = Some(found.id.clone());
            return Ok(selection);
        }
        let client = self.client_for(&selection).await?;
        let fresh = company::from_me(&company::fetch_me(&client).await?);
        if let Some(name) = &selection.name {
            self.store_companies(name, &selection.base_url, &fresh)?;
        }
        let id = match company::find(&wanted, &fresh) {
            company::Found::One(found) => found.id.clone(),
            company::Found::Many(matches) => return Err(company::ambiguous(&wanted, &matches)),
            company::Found::None => return Err(company::not_found(&wanted)),
        };
        selection.company = Some(id);
        Ok(selection)
    }

    /// The name of the company of a command, from the cache of the profile (else its id), for
    /// the success line of a write. `None` when the command has no company.
    pub fn company_label(&self, selection: &Selection, flag: Option<&str>) -> Option<String> {
        let id = self.company(flag, selection).ok()?;
        let config = self.config().ok()?;
        Some(company::label(&config, selection.name.as_deref(), &id))
    }

    /// Store the companies of `GET /api/me/` in the cache of a profile, when it is a profile of
    /// that base URL. The configuration is read again first, so only this key changes.
    pub fn store_companies(
        &self,
        name: &str,
        base_url: &str,
        companies: &[crate::config::Company],
    ) -> Result<()> {
        let mut config = self.config()?;
        match config.profiles.get_mut(name) {
            Some(profile) if profile.base_url == base_url => {
                if profile.companies.as_deref() == Some(companies) {
                    return Ok(());
                }
                profile.companies = Some(companies.to_vec());
            }
            _ => return Ok(()),
        }
        config.save(&self.paths)
    }

    /// The organization of a command: `--organization`, else the profile's.
    pub fn organization(&self, flag: Option<&str>, selection: &Selection) -> Result<String> {
        flag.filter(|value| !value.is_empty())
            .map(str::to_owned)
            .or_else(|| {
                Some(
                    selection
                        .profile
                        .as_ref()?
                        .organization
                        .as_ref()?
                        .id
                        .clone(),
                )
            })
            .ok_or_else(|| CliError::usage("No organization. Give --organization <id>."))
    }
}

/// Normalize a base URL; a wrong one is a usage error.
pub fn normalize(url: &str) -> Result<String> {
    normalize_base_url(url).map_err(|error| CliError::usage(error.to_string()))
}

/// The OAuth login of a profile: the stored tokens, refreshed when they expire soon.
pub struct OAuthSession {
    profile: String,
    client: Client,
    secrets: Secrets,
    /// The secret, and whether this run refreshed it already.
    secret: tokio::sync::Mutex<(Secret, bool)>,
}

impl OAuthSession {
    /// A fresh access token. A refresh stores the rotated refresh token before the new access
    /// token is used. A token this run refreshed is used until it expires, also when the server
    /// gives tokens that live less than the margin.
    ///
    /// Before a refresh, the session waits for the lock of the profile across processes and reads
    /// the store again: when another process refreshed in the meantime, its token is used, and
    /// its rotated refresh token is never sent twice.
    pub async fn token(&self) -> Result<String> {
        let mut state = self.secret.lock().await;
        let (secret, refreshed) = &mut *state;
        let Secret::OAuth {
            access_token,
            expires_at,
            ..
        } = &*secret
        else {
            return Err(CliError::auth("The profile holds no OAuth login."));
        };
        let now = SystemTime::now();
        let valid = time::parse(expires_at).is_some_and(|at| at > now);
        if !expiring(expires_at, now) || (*refreshed && valid) {
            return Ok(access_token.clone());
        }
        let _lock = self.secrets.lock(&self.profile).await?;
        if let Some(stored @ Secret::OAuth { .. }) = self.secrets.get(&self.profile).await? {
            *secret = stored;
        }
        let Secret::OAuth {
            refresh_token,
            access_token,
            expires_at,
        } = &*secret
        else {
            unreachable!("the secret is an OAuth login");
        };
        let now = SystemTime::now();
        if !expiring(expires_at, now) {
            return Ok(access_token.clone());
        }
        let metadata = oauth::discover(&self.client).await?;
        let tokens = oauth::refresh(&self.client, &metadata, refresh_token).await?;
        let fresh = tokens.into_secret(now, Some(refresh_token))?;
        self.secrets.set(&self.profile, &fresh).await?;
        *secret = fresh;
        *refreshed = true;
        match &*secret {
            Secret::OAuth { access_token, .. } => Ok(access_token.clone()),
            Secret::ApiKey { .. } => unreachable!("a refresh gives an OAuth secret"),
        }
    }

    /// The expiry of the current access token.
    pub async fn expires_at(&self) -> Option<String> {
        match &self.secret.lock().await.0 {
            Secret::OAuth { expires_at, .. } => Some(expires_at.clone()),
            Secret::ApiKey { .. } => None,
        }
    }
}

/// The access token expires within [`REFRESH_MARGIN`] of `now`, or its expiry is unreadable.
pub fn expiring(expires_at: &str, now: SystemTime) -> bool {
    time::parse(expires_at).is_none_or(|at| at <= now + REFRESH_MARGIN)
}

#[cfg(test)]
pub mod tests {
    use std::path::Path;

    use serde_json::Map;

    use super::*;
    use crate::config::AuthKind;

    pub fn app(dir: &Path, vars: &[(&str, &str)]) -> App {
        let env = Env::with_vars(vars);
        let paths = Paths {
            config_dir: dir.to_owned(),
            data_dir: dir.to_owned(),
        };
        let secrets = Secrets::with_keychain(None, paths.credentials_file());
        let (out, _, _) = Output::buffers();
        App {
            env,
            out,
            paths,
            secrets,
            globals: Globals::default(),
        }
    }

    fn config() -> Config {
        let mut config = Config::default();
        for (name, url) in [("a", "https://a.test"), ("b", "https://b.test")] {
            config.profiles.insert(
                name.into(),
                Profile {
                    base_url: url.into(),
                    auth: AuthKind::ApiKey,
                    organization: None,
                    company: Some(format!("{name}-company")),
                    companies: None,
                    extra: Map::new(),
                },
            );
        }
        config.default_profile = Some("a".into());
        config
    }

    #[test]
    fn selects_the_profile_and_the_base_url() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app(dir.path(), &[]);
        let chosen = app.select(&config()).unwrap();
        assert_eq!(
            (chosen.name.as_deref(), chosen.base_url.as_str()),
            (Some("a"), "https://a.test")
        );
        assert_eq!(chosen.refused, None);

        app.globals.base_url = Some("https://B.test/".into());
        let chosen = app.select(&config()).unwrap();
        assert_eq!(
            (chosen.name.as_deref(), chosen.base_url.as_str()),
            (Some("b"), "https://b.test")
        );

        app.globals.profile = Some("a".into());
        let chosen = app.select(&config()).unwrap();
        assert_eq!(chosen.base_url, "https://b.test");
        assert_eq!(
            chosen.refused.as_deref(),
            Some("The profile `a` is for https://a.test, not https://b.test.")
        );

        app.globals.profile = Some("zzz".into());
        let chosen = app.select(&config()).unwrap();
        assert!(chosen.refused.unwrap().contains("No profile `zzz`"));

        let app = self::app(dir.path(), &[("OSCAR_BASE_URL", "https://c.test")]);
        let chosen = app.select(&config()).unwrap();
        assert_eq!(
            (chosen.name, chosen.base_url.as_str()),
            (None, "https://c.test")
        );
        let chosen = self::app(dir.path(), &[])
            .select(&Config::default())
            .unwrap();
        assert_eq!(chosen.base_url, DEFAULT_BASE_URL);

        let app = self::app(dir.path(), &[("OSCAR_BASE_URL", "ftp://x.test")]);
        assert_eq!(
            app.select(&config()).unwrap_err().code,
            crate::error::EXIT_USAGE
        );
        let app = self::app(dir.path(), &[("OSCAR_PROFILE", "b")]);
        assert_eq!(app.select(&config()).unwrap().base_url, "https://b.test");
    }

    #[tokio::test]
    async fn chooses_the_credential() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app(dir.path(), &[("OSCAR_API_KEY", "osk_env")]);
        let chosen = app.select(&config()).unwrap();
        app.globals.api_key = Some("osk_flag".into());
        assert!(
            matches!(app.credential(&chosen).await.unwrap(), Credential::ApiKey(k) if k == "osk_flag")
        );
        app.globals.api_key = None;
        assert!(
            matches!(app.credential(&chosen).await.unwrap(), Credential::ApiKey(k) if k == "osk_env")
        );

        let app = self::app(dir.path(), &[]);
        let error = app.credential(&chosen).await.err().unwrap();
        assert_eq!(
            error.message,
            "The profile `a` has no stored credential. Run `oscar login`."
        );
        app.secrets
            .set(
                "a",
                &Secret::ApiKey {
                    api_key: "osk_stored".into(),
                },
            )
            .await
            .unwrap();
        assert!(
            matches!(app.credential(&chosen).await.unwrap(), Credential::ApiKey(k) if k == "osk_stored")
        );
        let none = app.select(&Config::default()).unwrap();
        let error = app.credential(&none).await.err().unwrap();
        assert_eq!(error.code, crate::error::EXIT_AUTH);
        assert!(
            error
                .message
                .starts_with("Not logged in to https://oscaraccountant.com.")
        );
        let refused = Selection {
            refused: Some("no".into()),
            ..chosen.clone()
        };
        assert_eq!(app.credential(&refused).await.err().unwrap().message, "no");

        let later = time::format(SystemTime::now() + Duration::from_secs(3600));
        app.secrets
            .set(
                "a",
                &Secret::OAuth {
                    refresh_token: "r".into(),
                    access_token: "at".into(),
                    expires_at: later.clone(),
                },
            )
            .await
            .unwrap();
        let Credential::OAuth(session) = app.credential(&chosen).await.unwrap() else {
            panic!("expected OAuth");
        };
        assert_eq!(session.token().await.unwrap(), "at");
        assert_eq!(session.expires_at().await, Some(later));
        std::fs::write(
            app.paths.config_file(),
            serde_json::to_string(&config()).unwrap(),
        )
        .unwrap();
        let (client, selection) = app.client().await.unwrap();
        assert_eq!(client.base_url(), "https://a.test");
        assert_eq!(selection.name.as_deref(), Some("a"));
    }

    #[test]
    fn company_and_organization() {
        let dir = tempfile::tempdir().unwrap();
        let app = app(dir.path(), &[("OSCAR_COMPANY", "env-company")]);
        let mut chosen = app.select(&config()).unwrap();
        assert_eq!(app.company(Some("flag"), &chosen).unwrap(), "flag");
        assert_eq!(app.company(None, &chosen).unwrap(), "env-company");
        let app = self::app(dir.path(), &[]);
        assert_eq!(app.company(Some(""), &chosen).unwrap(), "a-company");
        chosen.profile = None;
        assert_eq!(
            app.company(None, &chosen).unwrap_err().code,
            crate::error::EXIT_USAGE
        );
        assert_eq!(
            app.company_label(&chosen, Some("c9")).as_deref(),
            Some("c9")
        );
        assert_eq!(app.company_label(&chosen, None), None);
        chosen.company = Some("resolved".into());
        assert_eq!(app.company(Some("flag"), &chosen).unwrap(), "resolved");
        chosen.company = None;
        // No login at all: "Not logged in" (exit 3), not "No company".
        let mut nobody = chosen.clone();
        nobody.name = None;
        let error = app.company(None, &nobody).unwrap_err();
        assert_eq!(error.code, crate::error::EXIT_AUTH);
        assert!(
            error.message.starts_with("Not logged in to"),
            "{}",
            error.message
        );
        nobody.refused = Some("No profile `nope`. Run `oscar login --profile nope`.".into());
        assert!(
            app.company(None, &nobody)
                .unwrap_err()
                .message
                .contains("nope")
        );
        let keyed = self::app(dir.path(), &[("OSCAR_API_KEY", "osk_x")]);
        assert_eq!(
            keyed.company(None, &nobody).unwrap_err().code,
            crate::error::EXIT_USAGE
        );
        assert_eq!(app.organization(Some("o"), &chosen).unwrap(), "o");
        assert!(app.organization(None, &chosen).is_err());
    }

    #[test]
    fn refreshes_before_expiry() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        let at = |seconds: u64| time::format(SystemTime::UNIX_EPOCH + Duration::from_secs(seconds));
        assert!(expiring(&at(1_000_059), now));
        assert!(expiring(&at(1_000_060), now));
        assert!(!expiring(&at(1_000_061), now));
        assert!(expiring("garbage", now));
    }

    /// A token endpoint that rotates the refresh token, counts the refreshes, and refuses a
    /// refresh token it rotated away already (as the server does, with reuse protection).
    #[derive(Clone, Default)]
    struct Rotating {
        refreshes: Arc<std::sync::Mutex<u32>>,
    }

    impl oscaraccountant::HttpTransport for Rotating {
        fn send(
            &self,
            request: oscaraccountant::HttpRequest,
        ) -> futures::future::BoxFuture<
            '_,
            std::result::Result<oscaraccountant::HttpResponse, oscaraccountant::TransportError>,
        > {
            let base = "https://oscar.test";
            let answer = if request
                .url
                .ends_with("/.well-known/oauth-authorization-server")
            {
                (
                    200,
                    serde_json::json!({
                        "issuer": base,
                        "authorization_endpoint": format!("{base}/oauth/authorize"),
                        "token_endpoint": format!("{base}/oauth/token"),
                    })
                    .to_string(),
                )
            } else {
                let body = String::from_utf8_lossy(request.body.as_deref().unwrap_or_default())
                    .into_owned();
                let mut refreshes = self.refreshes.lock().unwrap();
                if body.contains("refresh_token=r1&") && *refreshes == 0 {
                    *refreshes += 1;
                    (
                        200,
                        r#"{"access_token": "a2", "refresh_token": "r2", "expires_in": 3600, "token_type": "Bearer"}"#
                            .to_owned(),
                    )
                } else {
                    *refreshes += 1;
                    (400, r#"{"error": "invalid_grant"}"#.to_owned())
                }
            };
            Box::pin(async move {
                // Let the other session run while this refresh is in flight.
                tokio::time::sleep(Duration::from_millis(50)).await;
                Ok(oscaraccountant::HttpResponse::new(
                    answer.0,
                    vec![("content-type".into(), "application/json".into())],
                    answer.1,
                ))
            })
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn two_sessions_refresh_once() {
        let dir = tempfile::tempdir().unwrap();
        let secrets = Secrets::with_keychain(None, dir.path().join("credentials.json"));
        let expiring = Secret::OAuth {
            refresh_token: "r1".into(),
            access_token: "a1".into(),
            expires_at: time::format(SystemTime::now() + Duration::from_secs(10)),
        };
        secrets.set("default", &expiring).await.unwrap();
        let endpoint = Rotating::default();
        let session = || OAuthSession {
            profile: "default".into(),
            client: Client::builder()
                .base_url("https://oscar.test")
                .env(|_| None)
                .max_retries(0)
                .transport(endpoint.clone())
                .build()
                .unwrap(),
            secrets: secrets.clone(),
            secret: tokio::sync::Mutex::new((expiring.clone(), false)),
        };
        let (first, second) = (session(), session());
        let (a, b) = tokio::join!(first.token(), second.token());
        assert_eq!(a.unwrap(), "a2");
        assert_eq!(b.unwrap(), "a2");
        assert_eq!(*endpoint.refreshes.lock().unwrap(), 1);
        match secrets.get("default").await.unwrap() {
            Some(Secret::OAuth { refresh_token, .. }) => assert_eq!(refresh_token, "r2"),
            other => panic!("expected the rotated login, got {other:?}"),
        }
    }

    #[test]
    fn json_output() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app(dir.path(), &[]);
        assert!(app.json());
        app.out.stdout_tty = true;
        assert!(!app.json());
        app.globals.json = true;
        assert!(app.json());
        assert!(app.anonymous("https://a.test").is_ok());
        assert!(app.anonymous("http://a.test").is_err());
    }
}
