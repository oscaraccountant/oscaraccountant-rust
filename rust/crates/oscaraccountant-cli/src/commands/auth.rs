//! `oscar auth token` and `oscar auth status`.

use serde_json::json;

use crate::error::{CliError, Result};
use crate::session::{App, Credential};

/// Print a fresh access token, or the API key, of the profile of the base URL
/// (`clients/spec/README.md`, section 1.3, the CLI side). Never `--api-key` or `OSCAR_API_KEY`.
pub async fn token(app: &mut App) -> Result<()> {
    let config = app.config()?;
    let selection = app.select(&config)?;
    let (token, expires_at, kind) = match app.stored(&selection).await? {
        Credential::ApiKey(key) => (key, None, "api_key"),
        Credential::OAuth(session) => {
            let token = session.token().await?;
            (token, session.expires_at().await, "oauth")
        }
    };
    if app.globals.json {
        let answer = json!({
            "token": token,
            "expires_at": expires_at,
            "kind": kind,
            "base_url": selection.base_url,
            "profile": selection.name,
        });
        app.out.line(&answer.to_string());
    } else {
        app.out.line(&token);
    }
    Ok(())
}

/// Show the state of the login, with no secret.
pub async fn status(app: &mut App) -> Result<()> {
    let config = app.config()?;
    let selection = app.select(&config)?;
    if app.globals.api_key.is_some() || app.env.var("OSCAR_API_KEY").is_some() {
        app.out.note(
            "An API key is given (--api-key or OSCAR_API_KEY); it wins over the stored login.",
        );
    }
    if let Some(refused) = &selection.refused {
        return Err(CliError::auth(refused.clone()));
    }
    let (Some(name), Some(profile)) = (&selection.name, &selection.profile) else {
        return Err(CliError::auth(format!(
            "Not logged in to {}. Run `oscar login`.",
            selection.base_url
        )));
    };
    let location = app.secrets.location(name).await?;
    let expires_at = match app.stored(&selection).await {
        Ok(Credential::OAuth(session)) => session.expires_at().await,
        _ => None,
    };
    let status = json!({
        "profile": name,
        "base_url": profile.base_url,
        "auth": profile.auth.as_str(),
        "organization": profile.organization.as_ref().map(|org| json!({"id": org.id, "name": org.name})),
        "company": profile.company,
        "store": location.as_ref().map(ToString::to_string),
        "expires_at": expires_at,
    });
    let json = app.json();
    app.out.value(&status, json);
    if location.is_none() {
        return Err(CliError::auth(format!(
            "The profile `{name}` has no stored credential. Run `oscar login`."
        )));
    }
    Ok(())
}
