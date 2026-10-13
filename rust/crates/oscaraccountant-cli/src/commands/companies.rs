//! `oscar companies use <x>`: make a company the default of the profile.
//! `oscar companies list` and `oscar companies get`
//! are the generated commands of the API.

use clap::ArgMatches;
use serde_json::json;

use super::string;
use crate::company;
use crate::error::{CliError, LOGIN_HINT, Result};
use crate::render;
use crate::session::App;

/// Resolve `<x>` (an id, a registry code, or the start of a name) and store it as the default
/// company of the profile.
pub async fn use_company(app: &mut App, leaf: &ArgMatches) -> Result<()> {
    let value = string(leaf, "company").unwrap_or_default().to_owned();
    let selection = app.select(&app.config()?)?;
    if let Some(refused) = &selection.refused {
        return Err(CliError::usage(refused.clone()));
    }
    let Some(name) = selection.name.clone() else {
        return Err(CliError::auth(format!(
            "Not logged in to {}. {LOGIN_HINT}",
            selection.base_url
        )));
    };
    let selection = app.with_company(selection, Some(&value)).await?;
    let id = app.company(None, &selection)?;
    let mut config = app.config()?;
    let Some(profile) = config.profiles.get_mut(&name) else {
        return Err(CliError::usage(format!(
            "No profile `{name}`. Run `oscar login --profile {name}`."
        )));
    };
    profile.company = Some(id.clone());
    config.save(&app.paths)?;
    let label = company::label(&config, Some(&name), &id);
    if app.globals.json {
        app.out
            .json(&json!({"profile": name, "company": {"id": id, "name": label}}));
    } else {
        let message = format!("Default company: {label} (profile `{name}`)");
        render::success_line(&mut app.out, &message);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::Map;

    use super::*;
    use crate::cli;
    use crate::company::tests::company;
    use crate::config::{AuthKind, Config, Profile};
    use crate::session::tests::app;

    fn leaf(args: &[&str]) -> ArgMatches {
        let matches = cli::command().try_get_matches_from(args).unwrap();
        super::super::leaf(&matches).clone()
    }

    #[tokio::test]
    async fn uses_a_company_of_the_cache() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app(dir.path(), &[]);
        let use_beta = leaf(&["oscar", "companies", "use", "beta"]);
        let error = use_company(&mut app, &use_beta).await.unwrap_err();
        assert_eq!(error.code, crate::error::EXIT_AUTH);
        let mut config = Config::default();
        config.profiles.insert(
            "p".into(),
            Profile {
                base_url: "https://a.test".into(),
                auth: AuthKind::ApiKey,
                organization: None,
                company: Some("c1".into()),
                companies: Some(vec![
                    company("c1", "Alpha OÜ", "11111111"),
                    company("c2", "Beta AS", "22222222"),
                ]),
                extra: Map::new(),
            },
        );
        config.save(&app.paths).unwrap();
        use_company(&mut app, &use_beta).await.unwrap();
        assert_eq!(
            app.config().unwrap().profiles["p"].company.as_deref(),
            Some("c2")
        );
        use_company(&mut app, &leaf(&["oscar", "companies", "use", "11111111"]))
            .await
            .unwrap();
        assert_eq!(
            app.config().unwrap().profiles["p"].company.as_deref(),
            Some("c1")
        );
        app.globals.json = true;
        use_company(&mut app, &use_beta).await.unwrap();
        app.globals.base_url = Some("https://b.test".into());
        app.globals.profile = Some("p".into());
        let error = use_company(&mut app, &use_beta).await.unwrap_err();
        assert!(error.message.contains("is for https://a.test"));
    }
}
