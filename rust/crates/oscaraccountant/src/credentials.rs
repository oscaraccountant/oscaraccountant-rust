//! The credential of a client (`clients/spec/README.md`, section 1).
//!
//! The order, the first match wins: an explicit credential ([`Credential::ApiKey`] or
//! [`Credential::TokenProvider`]), then `OSCAR_API_KEY`, then the stored login of the CLI when the
//! caller asks for it ([`Credential::Cli`] or `OSCAR_USE_CLI_LOGIN=1`). Otherwise every call fails
//! with [`Error::Authentication`] before it sends anything.

use std::fmt;
use std::future::Future;
use std::sync::Arc;

use futures::future::BoxFuture;

use crate::Error;
use crate::cli_login::CliLogin;
use crate::hooks::Hooks;

/// The error of a token provider.
pub type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// Gives the access token of each request. The CLI uses it to hand refreshed OAuth tokens to the
/// SDK.
///
/// It is called before each attempt, so it may cache the token itself. A closure
/// `|| async { Ok::<_, BoxError>(token) }` is a token provider.
pub trait TokenProvider: Send + Sync {
    /// The token for `Authorization: Bearer`.
    fn token(&self) -> BoxFuture<'_, Result<String, BoxError>>;
}

impl<F, Fut, E> TokenProvider for F
where
    F: Fn() -> Fut + Send + Sync,
    Fut: Future<Output = Result<String, E>> + Send + 'static,
    E: Into<BoxError>,
{
    fn token(&self) -> BoxFuture<'_, Result<String, BoxError>> {
        let future = self();
        Box::pin(async move { future.await.map_err(Into::into) })
    }
}

/// How a client authenticates.
#[derive(Clone)]
pub enum Credential {
    /// An organization API key (`osk_...`).
    ApiKey(String),
    /// A callback that gives an access token before each request.
    TokenProvider(Arc<dyn TokenProvider>),
    /// The stored login of the `oscar` CLI (section 1.3 of the spec). Only when asked for: the SDK
    /// never starts a process on its own.
    Cli,
}

impl fmt::Debug for Credential {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Never the value of a key.
        match self {
            Self::ApiKey(_) => f.write_str("ApiKey(***)"),
            Self::TokenProvider(_) => f.write_str("TokenProvider(..)"),
            Self::Cli => f.write_str("Cli"),
        }
    }
}

/// The credential a client resolved at construction.
#[derive(Clone)]
pub(crate) enum Auth {
    ApiKey(String),
    Provider(Arc<dyn TokenProvider>),
    Cli(Arc<CliLogin>),
    Missing,
}

/// The message of a client with no credential.
pub(crate) const MISSING: &str = "No credential for the Oscar API. Set OSCAR_API_KEY, give the client an API key or a \
    token provider, or run `oscar login` and use the CLI login (OSCAR_USE_CLI_LOGIN=1 or Credential::Cli).";

impl Auth {
    /// Resolve the credential in the order of section 1.1. An empty key counts as not set.
    pub(crate) fn resolve(
        explicit: Option<Credential>,
        base_url: &str,
        profile: Option<String>,
        hooks: &Hooks,
    ) -> Self {
        let explicit = explicit
            .filter(|credential| !matches!(credential, Credential::ApiKey(key) if key.is_empty()));
        match explicit {
            Some(Credential::ApiKey(key)) => return Self::ApiKey(key),
            Some(Credential::TokenProvider(provider)) => return Self::Provider(provider),
            Some(Credential::Cli) | None => {}
        }
        if let Some(key) = hooks.var("OSCAR_API_KEY") {
            return Self::ApiKey(key);
        }
        let asked = matches!(explicit, Some(Credential::Cli))
            || hooks.var("OSCAR_USE_CLI_LOGIN").as_deref() == Some("1");
        if asked {
            let profile = profile.or_else(|| hooks.var("OSCAR_PROFILE"));
            return Self::Cli(Arc::new(CliLogin::new(
                base_url.to_owned(),
                profile,
                hooks.clone(),
            )));
        }
        Self::Missing
    }

    /// The value for `Authorization: Bearer`.
    pub(crate) async fn token(&self) -> Result<String, Error> {
        match self {
            Self::ApiKey(key) => Ok(key.clone()),
            Self::Provider(provider) => match provider.token().await {
                Ok(token) if !token.is_empty() => Ok(token),
                Ok(_) => Err(Error::authentication(
                    "The token provider gave an empty token.",
                )),
                Err(error) => Err(Error::authentication(format!(
                    "The token provider failed: {error}"
                ))),
            },
            Self::Cli(login) => login.token().await,
            Self::Missing => Err(Error::authentication(MISSING)),
        }
    }

    /// The name of the kind, for `Debug`.
    pub(crate) fn kind(&self) -> &'static str {
        match self {
            Self::ApiKey(_) => "api_key",
            Self::Provider(_) => "token_provider",
            Self::Cli(_) => "cli",
            Self::Missing => "missing",
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn hooks(vars: &[(&str, &str)]) -> Hooks {
        let vars: HashMap<String, String> = vars
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        Hooks {
            env: Arc::new(move |name| vars.get(name).cloned()),
            ..Hooks::default()
        }
    }

    fn provider(token: &'static str) -> Credential {
        Credential::TokenProvider(Arc::new(move || async move {
            Ok::<_, BoxError>(token.to_owned())
        }))
    }

    #[tokio::test]
    async fn the_explicit_credential_wins() {
        let env = hooks(&[("OSCAR_API_KEY", "osk_env"), ("OSCAR_USE_CLI_LOGIN", "1")]);
        let auth = Auth::resolve(
            Some(Credential::ApiKey("osk_code".into())),
            "https://x.test",
            None,
            &env,
        );
        assert_eq!(auth.token().await.unwrap(), "osk_code");
        let auth = Auth::resolve(Some(provider("tok")), "https://x.test", None, &env);
        assert_eq!(auth.kind(), "token_provider");
        assert_eq!(auth.token().await.unwrap(), "tok");
    }

    #[tokio::test]
    async fn then_the_environment() {
        let env = hooks(&[("OSCAR_API_KEY", "osk_env"), ("OSCAR_USE_CLI_LOGIN", "1")]);
        let auth = Auth::resolve(
            Some(Credential::ApiKey(String::new())),
            "https://x.test",
            None,
            &env,
        );
        assert_eq!(auth.kind(), "api_key");
        assert_eq!(auth.token().await.unwrap(), "osk_env");
        let auth = Auth::resolve(Some(Credential::Cli), "https://x.test", None, &env);
        assert_eq!(auth.token().await.unwrap(), "osk_env");
    }

    #[test]
    fn then_the_cli_login_only_when_asked() {
        let asked = hooks(&[("OSCAR_USE_CLI_LOGIN", "1"), ("OSCAR_PROFILE", "work")]);
        let auth = Auth::resolve(None, "https://x.test", None, &asked);
        match &auth {
            Auth::Cli(login) => assert_eq!(login.profile(), Some("work")),
            _ => panic!("expected the CLI login"),
        }
        assert_eq!(auth.kind(), "cli");
        let auth = Auth::resolve(
            Some(Credential::Cli),
            "https://x.test",
            Some("own".into()),
            &hooks(&[]),
        );
        match &auth {
            Auth::Cli(login) => assert_eq!(login.profile(), Some("own")),
            _ => panic!("expected the CLI login"),
        }
        let not_one = hooks(&[("OSCAR_USE_CLI_LOGIN", "true")]);
        assert_eq!(
            Auth::resolve(None, "https://x.test", None, &not_one).kind(),
            "missing"
        );
    }

    #[tokio::test]
    async fn otherwise_an_authentication_error() {
        let auth = Auth::resolve(None, "https://x.test", None, &hooks(&[]));
        let error = auth.token().await.unwrap_err();
        assert!(matches!(error, Error::Authentication(_)));
        assert!(error.to_string().contains("OSCAR_API_KEY"));
        assert!(error.to_string().contains("oscar login"));
    }

    #[tokio::test]
    async fn a_failing_provider() {
        let failing = Credential::TokenProvider(Arc::new(|| async {
            Err::<String, _>("the vault is locked")
        }));
        let auth = Auth::resolve(Some(failing), "https://x.test", None, &hooks(&[]));
        let error = auth.token().await.unwrap_err();
        assert_eq!(
            error.to_string(),
            "The token provider failed: the vault is locked"
        );
        let empty = Auth::resolve(Some(provider("")), "https://x.test", None, &hooks(&[]));
        assert!(
            empty
                .token()
                .await
                .unwrap_err()
                .to_string()
                .contains("empty")
        );
    }

    #[test]
    fn debug_never_shows_a_key() {
        assert_eq!(
            format!("{:?}", Credential::ApiKey("osk_secret".into())),
            "ApiKey(***)"
        );
        assert_eq!(format!("{:?}", provider("t")), "TokenProvider(..)");
        assert_eq!(format!("{:?}", Credential::Cli), "Cli");
    }
}
