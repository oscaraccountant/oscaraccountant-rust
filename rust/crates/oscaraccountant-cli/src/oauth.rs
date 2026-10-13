//! The OAuth client of the CLI (`clients/spec/README.md`, section 4): discovery, PKCE, the code
//! exchange, the device grant, refresh and revocation. Every call goes through the SDK's raw
//! request layer with no `Authorization` header.

use std::future::Future;
use std::time::{Duration, SystemTime};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use oscaraccountant::{Client, HttpMethod, RawResponse};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use sha2::{Digest, Sha256};

use crate::error::{CliError, Result};
use crate::secrets::Secret;
use crate::time;

/// The first-party client of the CLI at the authorization server.
pub const CLIENT_ID: &str = "oscar-cli";
/// The grant type of RFC 8628.
pub const DEVICE_GRANT: &str = "urn:ietf:params:oauth:grant-type:device_code";
/// The interval of the device grant when the server gives none.
const DEFAULT_INTERVAL: u64 = 5;
/// What `slow_down` adds to the interval.
const SLOW_DOWN: u64 = 5;
/// The life of an access token when the server gives no `expires_in`.
const DEFAULT_EXPIRES_IN: u64 = 3600;

/// The authorization server metadata of RFC 8414, the fields the CLI uses.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Metadata {
    /// Must equal the base URL.
    pub issuer: String,
    /// The authorization endpoint of the loopback login.
    pub authorization_endpoint: String,
    /// The token endpoint.
    pub token_endpoint: String,
    /// The device authorization endpoint.
    #[serde(default)]
    pub device_authorization_endpoint: Option<String>,
    /// The revocation endpoint.
    #[serde(default)]
    pub revocation_endpoint: Option<String>,
}

/// The error of a device login the person denied.
pub const DENIED: &str = "The login was denied.";

/// The resource of the tokens of the CLI: the API, `<base URL>/api`.
pub fn resource(base_url: &str) -> String {
    format!("{base_url}/api")
}

/// The scope of `--scope`: `read`, or `read write` (the default).
pub fn scope(value: Option<&str>) -> Result<&'static str> {
    match value {
        None | Some("write") => Ok("read write"),
        Some("read") => Ok("read"),
        Some(other) => Err(CliError::usage(format!(
            "--scope must be `read` or `write`, not `{other}`."
        ))),
    }
}

/// Read the metadata of the base URL of `client`, and check its issuer.
pub async fn discover(client: &Client) -> Result<Metadata> {
    let metadata: Metadata = client
        .request(HttpMethod::Get, "/.well-known/oauth-authorization-server")
        .without_credential()
        .send()
        .await?
        .json()
        .map_err(|error| CliError::auth(format!("The OAuth metadata is not valid: {error}")))?;
    if metadata.issuer.trim_end_matches('/') != client.base_url() {
        return Err(CliError::auth(format!(
            "The OAuth issuer {} is not the base URL {}.",
            metadata.issuer,
            client.base_url()
        )));
    }
    Ok(metadata)
}

/// The path of an endpoint under the base URL. An endpoint on another origin is refused: the
/// CLI sends its tokens to its base URL only.
pub fn endpoint_path(base_url: &str, endpoint: &str) -> Result<String> {
    match endpoint.strip_prefix(base_url) {
        Some(path) if path.starts_with('/') => Ok(path.to_owned()),
        _ => Err(CliError::auth(format!(
            "The OAuth endpoint {endpoint} is not under the base URL {base_url}."
        ))),
    }
}

/// 32 random bytes, base64url without padding: a PKCE verifier or a `state`.
pub fn random_token() -> Result<String> {
    let mut bytes = [0_u8; 32];
    getrandom::fill(&mut bytes)
        .map_err(|error| CliError::failed(format!("No random source: {error}")))?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

/// The S256 challenge of a PKCE verifier.
pub fn challenge(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

/// The parameters of an authorization request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthorizeRequest<'a> {
    /// The loopback redirect URI, with its real port.
    pub redirect_uri: &'a str,
    /// `read` or `read write`.
    pub scope: &'a str,
    /// The `state`.
    pub state: &'a str,
    /// The S256 challenge.
    pub challenge: &'a str,
    /// `<base URL>/api`.
    pub resource: &'a str,
}

/// The URL the browser opens.
pub fn authorize_url(metadata: &Metadata, request: &AuthorizeRequest<'_>) -> Result<String> {
    let mut url = url::Url::parse(&metadata.authorization_endpoint).map_err(|error| {
        CliError::auth(format!("The authorization endpoint is not a URL: {error}"))
    })?;
    url.query_pairs_mut()
        .append_pair("response_type", "code")
        .append_pair("client_id", CLIENT_ID)
        .append_pair("redirect_uri", request.redirect_uri)
        .append_pair("scope", request.scope)
        .append_pair("state", request.state)
        .append_pair("code_challenge", request.challenge)
        .append_pair("code_challenge_method", "S256")
        .append_pair("prompt", "consent")
        .append_pair("resource", request.resource);
    Ok(url.into())
}

/// A token response.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct TokenResponse {
    /// The access token.
    pub access_token: String,
    /// The refresh token.
    #[serde(default)]
    pub refresh_token: Option<String>,
    /// The life of the access token, in seconds.
    #[serde(default)]
    pub expires_in: Option<u64>,
}

impl TokenResponse {
    /// The secret to store. `previous` is the refresh token to keep when the server sends no
    /// new one.
    pub fn into_secret(self, now: SystemTime, previous: Option<&str>) -> Result<Secret> {
        let refresh_token = self
            .refresh_token
            .or_else(|| previous.map(str::to_owned))
            .ok_or_else(|| CliError::auth("The token response has no refresh token."))?;
        let expires_in = Duration::from_secs(self.expires_in.unwrap_or(DEFAULT_EXPIRES_IN));
        Ok(Secret::OAuth {
            refresh_token,
            access_token: self.access_token,
            expires_at: time::format(now + expires_in),
        })
    }
}

/// An OAuth error body: `{"error", "error_description"}`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct OAuthError {
    /// The code, for example `invalid_grant`.
    pub error: String,
    /// The text for a person.
    #[serde(default)]
    pub error_description: Option<String>,
}

impl OAuthError {
    /// `invalid_grant: The code expired.`
    pub fn describe(&self) -> String {
        match &self.error_description {
            Some(description) if !description.is_empty() => {
                format!("{}: {description}", self.error)
            }
            _ => self.error.clone(),
        }
    }

    /// The OAuth error in the body of a failed call, if it is one.
    pub fn from_error(error: &oscaraccountant::Error) -> Option<Self> {
        serde_json::from_str(&error.api_error()?.body).ok()
    }
}

/// A form `POST` to an endpoint, with no retry and no credential. An OAuth error body is the
/// inner `Err`.
async fn post_form_raw(
    client: &Client,
    endpoint: &str,
    form: &[(&str, &str)],
) -> Result<std::result::Result<RawResponse, OAuthError>> {
    let path = endpoint_path(client.base_url(), endpoint)?;
    let body = url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs(form)
        .finish();
    let answer = client
        .request(HttpMethod::Post, path)
        .without_credential()
        .max_retries(0)
        .body("application/x-www-form-urlencoded", body)
        .send()
        .await;
    match answer {
        Ok(response) => Ok(Ok(response)),
        Err(error) => match OAuthError::from_error(&error) {
            Some(oauth) => Ok(Err(oauth)),
            None => Err(error.into()),
        },
    }
}

/// [`post_form_raw`], with the JSON of a success parsed.
async fn post_form<T: DeserializeOwned>(
    client: &Client,
    endpoint: &str,
    form: &[(&str, &str)],
) -> Result<std::result::Result<T, OAuthError>> {
    match post_form_raw(client, endpoint, form).await? {
        Ok(response) => Ok(Ok(response.json().map_err(|error| {
            CliError::auth(format!(
                "The OAuth server answered unexpected JSON: {error}"
            ))
        })?)),
        Err(error) => Ok(Err(error)),
    }
}

/// Exchange an authorization code.
pub async fn exchange_code(
    client: &Client,
    metadata: &Metadata,
    code: &str,
    redirect_uri: &str,
    verifier: &str,
) -> Result<TokenResponse> {
    let resource = resource(client.base_url());
    let answer = post_form(
        client,
        &metadata.token_endpoint,
        &[
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", redirect_uri),
            ("client_id", CLIENT_ID),
            ("code_verifier", verifier),
            ("resource", &resource),
        ],
    )
    .await?;
    answer.map_err(|error| CliError::auth(format!("The login failed: {}", error.describe())))
}

/// Refresh an access token. A reused or dead refresh token means "log in again".
pub async fn refresh(
    client: &Client,
    metadata: &Metadata,
    refresh_token: &str,
) -> Result<TokenResponse> {
    let resource = resource(client.base_url());
    let answer = post_form(
        client,
        &metadata.token_endpoint,
        &[
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
            ("client_id", CLIENT_ID),
            ("resource", &resource),
        ],
    )
    .await?;
    answer.map_err(|error| {
        CliError::auth(format!(
            "The login has ended ({}). Run `oscar login` again.",
            error.describe()
        ))
    })
}

/// Revoke a refresh token (RFC 7009).
pub async fn revoke(client: &Client, metadata: &Metadata, refresh_token: &str) -> Result<()> {
    let endpoint = metadata
        .revocation_endpoint
        .as_deref()
        .ok_or_else(|| CliError::failed("The server has no revocation endpoint."))?;
    let answer = post_form_raw(
        client,
        endpoint,
        &[
            ("token", refresh_token),
            ("token_type_hint", "refresh_token"),
            ("client_id", CLIENT_ID),
        ],
    )
    .await?;
    answer
        .map(drop)
        .map_err(|error| CliError::failed(format!("The revocation failed: {}", error.describe())))
}

/// The answer of the device authorization endpoint.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct DeviceAuthorization {
    /// The code the CLI polls with.
    pub device_code: String,
    /// The code the user types.
    pub user_code: String,
    /// The page where the user types it.
    pub verification_uri: String,
    /// The page with the code in it.
    #[serde(default)]
    pub verification_uri_complete: Option<String>,
    /// The seconds between two polls.
    #[serde(default)]
    pub interval: Option<u64>,
}

/// Start a device login.
pub async fn device_authorize(
    client: &Client,
    metadata: &Metadata,
    scope: &str,
) -> Result<DeviceAuthorization> {
    let endpoint = metadata
        .device_authorization_endpoint
        .as_deref()
        .ok_or_else(|| CliError::auth("The server does not offer the device login."))?;
    let resource = resource(client.base_url());
    let answer = post_form(
        client,
        endpoint,
        &[
            ("client_id", CLIENT_ID),
            ("scope", scope),
            ("resource", &resource),
        ],
    )
    .await?;
    answer.map_err(|error| CliError::auth(format!("The device login failed: {}", error.describe())))
}

/// Poll the token endpoint until the user decides, at the interval of the server, 5 seconds
/// more after each `slow_down`. `sleep` waits; a test gives its own.
pub async fn poll_device<S, F>(
    client: &Client,
    metadata: &Metadata,
    device: &DeviceAuthorization,
    sleep: S,
) -> Result<TokenResponse>
where
    S: Fn(Duration) -> F,
    F: Future<Output = ()>,
{
    let mut interval = device.interval.unwrap_or(DEFAULT_INTERVAL);
    loop {
        sleep(Duration::from_secs(interval)).await;
        let answer = post_form(
            client,
            &metadata.token_endpoint,
            &[
                ("grant_type", DEVICE_GRANT),
                ("device_code", &device.device_code),
                ("client_id", CLIENT_ID),
            ],
        )
        .await?;
        let error = match answer {
            Ok(tokens) => return Ok(tokens),
            Err(error) => error,
        };
        match error.error.as_str() {
            "authorization_pending" => {}
            "slow_down" => interval += SLOW_DOWN,
            "access_denied" => return Err(CliError::auth(DENIED)),
            "expired_token" => {
                return Err(CliError::auth("The code expired. Run `oscar login` again."));
            }
            _ => {
                return Err(CliError::auth(format!(
                    "The device login failed: {}",
                    error.describe()
                )));
            }
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::sync::{Arc, Mutex};

    use futures::future::BoxFuture;
    use oscaraccountant::{HttpRequest, HttpResponse, HttpTransport, TransportError};

    use super::*;

    /// A transport that answers from a list and records the requests.
    #[derive(Clone, Default)]
    pub struct Script {
        answers: Arc<Mutex<Vec<(u16, String)>>>,
        pub requests: Arc<Mutex<Vec<HttpRequest>>>,
    }

    impl Script {
        pub fn new(answers: &[(u16, &str)]) -> Self {
            Self {
                answers: Arc::new(Mutex::new(
                    answers
                        .iter()
                        .rev()
                        .map(|(s, b)| (*s, (*b).to_owned()))
                        .collect(),
                )),
                requests: Arc::default(),
            }
        }

        fn bodies(&self) -> Vec<String> {
            self.requests
                .lock()
                .unwrap()
                .iter()
                .map(|request| {
                    String::from_utf8_lossy(request.body.as_deref().unwrap_or_default())
                        .into_owned()
                })
                .collect()
        }
    }

    impl HttpTransport for Script {
        fn send(
            &self,
            request: HttpRequest,
        ) -> BoxFuture<'_, std::result::Result<HttpResponse, TransportError>> {
            self.requests.lock().unwrap().push(request);
            let next = self.answers.lock().unwrap().pop();
            Box::pin(async move {
                match next {
                    Some((status, body)) => Ok(HttpResponse::new(
                        status,
                        vec![("content-type".into(), "application/json".into())],
                        body,
                    )),
                    None => Err(TransportError::Connection("no answer".into())),
                }
            })
        }
    }

    const BASE: &str = "https://oscar.test";

    fn client(script: &Script) -> Client {
        Client::builder()
            .base_url(BASE)
            .env(|_| None)
            .max_retries(0)
            .transport(script.clone())
            .build()
            .unwrap()
    }

    fn metadata() -> Metadata {
        Metadata {
            issuer: BASE.into(),
            authorization_endpoint: format!("{BASE}/oauth/authorize"),
            token_endpoint: format!("{BASE}/oauth/token"),
            device_authorization_endpoint: Some(format!("{BASE}/oauth/device_authorization")),
            revocation_endpoint: Some(format!("{BASE}/oauth/revoke")),
        }
    }

    fn device(interval: Option<u64>) -> DeviceAuthorization {
        DeviceAuthorization {
            device_code: "dev".into(),
            user_code: "ABCD-EFGH".into(),
            verification_uri: format!("{BASE}/oauth-device"),
            verification_uri_complete: None,
            interval,
        }
    }

    #[tokio::test]
    async fn discovers_and_checks_the_issuer() {
        let body = serde_json::json!({
            "issuer": BASE,
            "authorization_endpoint": format!("{BASE}/oauth/authorize"),
            "token_endpoint": format!("{BASE}/oauth/token"),
        })
        .to_string();
        let script = Script::new(&[(200, &body)]);
        let found = discover(&client(&script)).await.unwrap();
        assert_eq!(found.token_endpoint, format!("{BASE}/oauth/token"));
        assert_eq!(found.revocation_endpoint, None);
        let request = script.requests.lock().unwrap()[0].clone();
        assert_eq!(
            request.url,
            format!("{BASE}/.well-known/oauth-authorization-server")
        );
        assert_eq!(request.header("authorization"), None);

        let other = body.replace("\"https://oscar.test\"", "\"https://evil.test\"");
        let error = discover(&client(&Script::new(&[(200, &other)])))
            .await
            .unwrap_err();
        assert_eq!(error.code, crate::error::EXIT_AUTH);
        let error = discover(&client(&Script::new(&[(200, "[]")])))
            .await
            .unwrap_err();
        assert!(error.message.contains("metadata is not valid"));
        let error = discover(&client(&Script::new(&[]))).await.unwrap_err();
        assert_eq!(error.code, crate::error::EXIT_NETWORK);
    }

    #[test]
    fn endpoints_must_be_under_the_base_url() {
        assert_eq!(
            endpoint_path(BASE, "https://oscar.test/oauth/token").unwrap(),
            "/oauth/token"
        );
        assert!(endpoint_path(BASE, "https://oscar.test.evil/oauth/token").is_err());
        assert!(endpoint_path(BASE, "https://evil.test/oauth/token").is_err());
    }

    #[test]
    fn pkce_state_and_the_authorize_url() {
        let verifier = random_token().unwrap();
        assert_eq!(verifier.len(), 43);
        assert_ne!(verifier, random_token().unwrap());
        // RFC 7636, appendix B.
        assert_eq!(
            challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
        let url = authorize_url(
            &metadata(),
            &AuthorizeRequest {
                redirect_uri: "http://127.0.0.1:5000/callback",
                scope: "read write",
                state: "st",
                challenge: "ch",
                resource: "https://oscar.test/api",
            },
        )
        .unwrap();
        assert_eq!(
            url,
            "https://oscar.test/oauth/authorize?response_type=code&client_id=oscar-cli\
             &redirect_uri=http%3A%2F%2F127.0.0.1%3A5000%2Fcallback&scope=read+write&state=st\
             &code_challenge=ch&code_challenge_method=S256&prompt=consent\
             &resource=https%3A%2F%2Foscar.test%2Fapi"
        );
        let mut broken = metadata();
        broken.authorization_endpoint = "not a url".into();
        assert!(
            authorize_url(
                &broken,
                &AuthorizeRequest {
                    redirect_uri: "",
                    scope: "",
                    state: "",
                    challenge: "",
                    resource: "",
                }
            )
            .is_err()
        );
    }

    #[test]
    fn scopes_and_resources() {
        assert_eq!(scope(None).unwrap(), "read write");
        assert_eq!(scope(Some("write")).unwrap(), "read write");
        assert_eq!(scope(Some("read")).unwrap(), "read");
        assert_eq!(
            scope(Some("admin")).unwrap_err().code,
            crate::error::EXIT_USAGE
        );
        assert_eq!(resource(BASE), "https://oscar.test/api");
    }

    #[test]
    fn token_responses_become_secrets() {
        let now = std::time::UNIX_EPOCH;
        let tokens = TokenResponse {
            access_token: "a".into(),
            refresh_token: Some("r".into()),
            expires_in: Some(60),
        };
        assert_eq!(
            tokens.clone().into_secret(now, None).unwrap(),
            Secret::OAuth {
                refresh_token: "r".into(),
                access_token: "a".into(),
                expires_at: "1970-01-01T00:01:00Z".into(),
            }
        );
        let kept = TokenResponse {
            refresh_token: None,
            expires_in: None,
            ..tokens
        };
        match kept.clone().into_secret(now, Some("old")).unwrap() {
            Secret::OAuth {
                refresh_token,
                expires_at,
                ..
            } => {
                assert_eq!(refresh_token, "old");
                assert_eq!(expires_at, "1970-01-01T01:00:00Z");
            }
            Secret::ApiKey { .. } => panic!("expected OAuth"),
        }
        assert!(kept.into_secret(now, None).is_err());
    }

    #[tokio::test]
    async fn exchanges_refreshes_and_revokes() {
        let tokens = r#"{"access_token": "a", "refresh_token": "r", "expires_in": 60, "token_type": "Bearer"}"#;
        let script = Script::new(&[
            (200, tokens),
            (200, tokens),
            (
                400,
                r#"{"error": "invalid_grant", "error_description": "Reused."}"#,
            ),
            (200, ""),
            (400, r#"{"error": "unsupported_token_type"}"#),
            (400, r#"{"error": "invalid_grant"}"#),
        ]);
        let client = client(&script);
        let meta = metadata();
        let got = exchange_code(&client, &meta, "c", "http://127.0.0.1:1/callback", "v")
            .await
            .unwrap();
        assert_eq!(got.access_token, "a");
        assert_eq!(
            refresh(&client, &meta, "r0")
                .await
                .unwrap()
                .refresh_token
                .as_deref(),
            Some("r")
        );
        let error = refresh(&client, &meta, "r0").await.unwrap_err();
        assert_eq!(error.code, crate::error::EXIT_AUTH);
        assert!(error.message.contains("invalid_grant: Reused."));
        revoke(&client, &meta, "r").await.unwrap();
        let error = revoke(&client, &meta, "r").await.unwrap_err();
        assert_eq!(
            error.message,
            "The revocation failed: unsupported_token_type"
        );
        let error = exchange_code(&client, &meta, "c", "u", "v")
            .await
            .unwrap_err();
        assert_eq!(error.message, "The login failed: invalid_grant");
        let bodies = script.bodies();
        assert_eq!(
            bodies[0],
            "grant_type=authorization_code&code=c&redirect_uri=http%3A%2F%2F127.0.0.1%3A1%2Fcallback\
             &client_id=oscar-cli&code_verifier=v&resource=https%3A%2F%2Foscar.test%2Fapi"
        );
        assert_eq!(
            bodies[1],
            "grant_type=refresh_token&refresh_token=r0&client_id=oscar-cli&resource=https%3A%2F%2Foscar.test%2Fapi"
        );
        assert_eq!(
            bodies[3],
            "token=r&token_type_hint=refresh_token&client_id=oscar-cli"
        );
        let request = script.requests.lock().unwrap()[0].clone();
        assert_eq!(
            request.header("content-type"),
            Some("application/x-www-form-urlencoded")
        );
        assert_eq!(request.header("authorization"), None);

        let mut none = metadata();
        none.revocation_endpoint = None;
        none.device_authorization_endpoint = None;
        assert!(revoke(&client, &none, "r").await.is_err());
        assert!(device_authorize(&client, &none, "read").await.is_err());
        let error = exchange_code(&client, &meta, "c", "u", "v")
            .await
            .unwrap_err();
        assert_eq!(error.code, crate::error::EXIT_NETWORK);
        let script = Script::new(&[(200, "not json"), (500, "oops")]);
        let client = self::client(&script);
        assert!(
            exchange_code(&client, &meta, "c", "u", "v")
                .await
                .unwrap_err()
                .message
                .contains("unexpected JSON")
        );
        assert_eq!(
            exchange_code(&client, &meta, "c", "u", "v")
                .await
                .unwrap_err()
                .code,
            crate::error::EXIT_API
        );
    }

    #[tokio::test]
    async fn the_device_grant() {
        let start = r#"{"device_code": "dev", "user_code": "ABCD-EFGH", "verification_uri": "https://oscar.test/oauth-device", "expires_in": 600, "interval": 2}"#;
        let pending = r#"{"error": "authorization_pending"}"#;
        let slow = r#"{"error": "slow_down"}"#;
        let tokens = r#"{"access_token": "a", "refresh_token": "r", "expires_in": 60}"#;
        let script = Script::new(&[(200, start), (400, pending), (400, slow), (200, tokens)]);
        let client = client(&script);
        let meta = metadata();
        let started = device_authorize(&client, &meta, "read").await.unwrap();
        assert_eq!(started.interval, Some(2));
        let waits = Arc::new(Mutex::new(Vec::new()));
        let recorded = waits.clone();
        let got = poll_device(&client, &meta, &started, move |wait| {
            recorded.lock().unwrap().push(wait.as_secs());
            async {}
        })
        .await
        .unwrap();
        assert_eq!(got.access_token, "a");
        assert_eq!(*waits.lock().unwrap(), vec![2, 2, 7]);
        let bodies = script.bodies();
        assert_eq!(
            bodies[0],
            "client_id=oscar-cli&scope=read&resource=https%3A%2F%2Foscar.test%2Fapi"
        );
        assert_eq!(
            bodies[1],
            "grant_type=urn%3Aietf%3Aparams%3Aoauth%3Agrant-type%3Adevice_code&device_code=dev&client_id=oscar-cli"
        );
    }

    #[tokio::test]
    async fn the_device_grant_ends() {
        for (body, message) in [
            (r#"{"error": "access_denied"}"#, "The login was denied."),
            (
                r#"{"error": "expired_token"}"#,
                "The code expired. Run `oscar login` again.",
            ),
            (
                r#"{"error": "invalid_grant", "error_description": "Unknown."}"#,
                "The device login failed: invalid_grant: Unknown.",
            ),
        ] {
            let script = Script::new(&[(400, body)]);
            let waits = Arc::new(Mutex::new(Vec::new()));
            let recorded = waits.clone();
            let error = poll_device(&client(&script), &metadata(), &device(None), move |wait| {
                recorded.lock().unwrap().push(wait.as_secs());
                async {}
            })
            .await
            .unwrap_err();
            assert_eq!(error.code, crate::error::EXIT_AUTH);
            assert_eq!(error.message, message);
            assert_eq!(*waits.lock().unwrap(), vec![5]);
        }
        let error = device_authorize(
            &client(&Script::new(&[(400, r#"{"error": "invalid_scope"}"#)])),
            &metadata(),
            "x",
        )
        .await
        .unwrap_err();
        assert_eq!(error.message, "The device login failed: invalid_scope");
    }

    #[test]
    fn describes_oauth_errors() {
        let bare = OAuthError {
            error: "x".into(),
            error_description: Some(String::new()),
        };
        assert_eq!(bare.describe(), "x");
        assert!(OAuthError::from_error(&oscaraccountant::Error::Connection("x".into())).is_none());
    }
}
