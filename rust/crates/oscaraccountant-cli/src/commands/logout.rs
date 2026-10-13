//! `oscar logout` (`clients/spec/README.md`, section 4.6): revoke, then forget.

use crate::error::{CliError, Result};
use crate::oauth;
use crate::secrets::Secret;
use crate::session::App;

/// Revoke the refresh token of the profile, then delete its secrets and its entry.
pub async fn run(app: &mut App) -> Result<()> {
    let mut config = app.config()?;
    let name = app
        .explicit_profile()
        .or_else(|| config.default_name().map(str::to_owned))
        .ok_or_else(|| CliError::auth("Not logged in."))?;
    let profile = config.profiles.get(&name).cloned();
    let secret = app.secrets.get(&name).await?;
    if profile.is_none() && secret.is_none() {
        return Err(CliError::usage(format!("No profile `{name}`.")));
    }
    match (&secret, &profile) {
        (Some(Secret::OAuth { refresh_token, .. }), Some(profile)) => {
            let revoked = async {
                let client = app.anonymous(&profile.base_url)?;
                let metadata = oauth::discover(&client).await?;
                oauth::revoke(&client, &metadata, refresh_token).await
            }
            .await;
            if let Err(error) = revoked {
                app.out.warn(&format!(
                    "The server did not revoke the login ({}). It ends when it expires.",
                    error.message
                ));
            }
        }
        (Some(Secret::ApiKey { .. }), _) => app.out.note(
            "The API key stays valid at the server. Delete it there when it is no longer needed.",
        ),
        _ => {}
    }
    app.secrets.delete(&name).await?;
    // The keychain did not answer: a copy of the credential may remain in it.
    for notice in app.secrets.take_notices() {
        app.out.warn(&notice);
    }
    config.remove(&name);
    config.save(&app.paths)?;
    app.out
        .note(&format!("Logged out of the profile `{name}`."));
    Ok(())
}
