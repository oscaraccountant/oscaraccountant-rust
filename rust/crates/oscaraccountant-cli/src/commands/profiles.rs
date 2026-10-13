//! `oscar profiles list` and `oscar profiles use <name>`.

use clap::ArgMatches;
use serde_json::{Value, json};

use super::string;
use crate::error::{CliError, Result};
use crate::session::App;

/// List the profiles, with no secret.
pub fn list(app: &mut App) -> Result<()> {
    let config = app.config()?;
    let default = config.default_name().map(str::to_owned);
    let rows: Vec<Value> = config
        .profiles
        .iter()
        .map(|(name, profile)| {
            json!({
                "name": name,
                "default": default.as_deref() == Some(name.as_str()),
                "base_url": profile.base_url,
                "auth": profile.auth.as_str(),
                "organization": profile.organization.as_ref().map(|org| &org.name),
                "company": profile.company,
            })
        })
        .collect();
    if rows.is_empty() && !app.json() {
        app.out.note("No profile. Run `oscar login`.");
        return Ok(());
    }
    let json = app.json();
    app.out.value(&Value::Array(rows), json);
    Ok(())
}

/// Make a profile the default.
pub fn use_profile(app: &mut App, leaf: &ArgMatches) -> Result<()> {
    let name = string(leaf, "name").unwrap_or_default();
    let mut config = app.config()?;
    if !config.profiles.contains_key(name) {
        return Err(CliError::usage(format!(
            "No profile `{name}`. `oscar profiles list` shows them."
        )));
    }
    config.default_profile = Some(name.to_owned());
    config.save(&app.paths)?;
    app.out
        .note(&format!("The default profile is now `{name}`."));
    Ok(())
}
