//! `oscar config get company` and `oscar config set company <id>`: the default company of the
//! profile.

use clap::ArgMatches;
use serde_json::json;

use super::string;
use crate::error::{CliError, Result};
use crate::session::App;

/// Read or change a setting of the selected profile.
pub fn run(app: &mut App, action: &str, leaf: &ArgMatches) -> Result<()> {
    let key = string(leaf, "key").unwrap_or_default();
    if key != "company" {
        return Err(CliError::usage(format!("Unknown setting `{key}`.")));
    }
    let mut config = app.config()?;
    let selection = app.select(&config)?;
    if let Some(refused) = selection.refused {
        return Err(CliError::auth(refused));
    }
    let Some(name) = selection.name else {
        return Err(CliError::auth("No profile. Run `oscar login` first."));
    };
    let profile = config
        .profiles
        .get_mut(&name)
        .ok_or_else(|| CliError::auth(format!("No profile `{name}`.")))?;
    if action == "set" {
        let value = string(leaf, "value").unwrap_or_default();
        profile.company = Some(value.to_owned());
        config.save(&app.paths)?;
        app.out
            .note(&format!("The default company of `{name}` is now {value}."));
        return Ok(());
    }
    let company = profile.company.clone();
    if app.json() {
        app.out.json(&json!({ "company": company }));
    } else {
        app.out.line(company.as_deref().unwrap_or_default());
    }
    Ok(())
}
