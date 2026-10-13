//! `oscar onboard`, `oscar onboard status` and `oscar onboard agreement`.
//! The steps live in `crate::onboarding`.

use clap::ArgMatches;
use serde_json::Map;

use crate::cli::{flag, string};
use crate::error::{CliError, Result};
use crate::onboarding::{self, Options, questions};
use crate::session::App;

/// Run `oscar onboard` or one of its subcommands.
pub async fn run(app: &mut App, matches: &ArgMatches) -> Result<()> {
    match matches.subcommand() {
        Some(("status", leaf)) => onboarding::status(app, options(leaf)?).await,
        Some(("agreement", leaf)) => {
            // `--download` with no path is an empty one: the default file name.
            let download = string(leaf, "download").map(str::to_owned);
            onboarding::agreement(app, options(leaf)?, download, flag(leaf, "force")).await
        }
        _ => onboarding::onboard(app, options(matches)?).await,
    }
}

fn text(leaf: &ArgMatches, id: &str) -> Option<String> {
    string(leaf, id).map(str::to_owned)
}

fn answer(leaf: &ArgMatches, id: &str) -> Option<bool> {
    text(leaf, id).and_then(|value| questions::yes_no(&value))
}

fn set(leaf: &ArgMatches, id: &str) -> bool {
    flag(leaf, id)
}

/// The options of the flags. The flags a subcommand has not are left out.
pub fn options(leaf: &ArgMatches) -> Result<Options> {
    let mut answers = Map::new();
    if let Some(path) = text(leaf, "answers") {
        let content = std::fs::read_to_string(&path)
            .map_err(|error| CliError::usage(format!("Cannot read {path}: {error}")))?;
        answers = questions::from_file(&content)?;
    }
    if let Ok(Some(pairs)) = leaf.try_get_many::<String>("answer") {
        for pair in pairs {
            let (key, value) = questions::from_flag(pair)?;
            answers.insert(key, value);
        }
    }
    Ok(Options {
        company: text(leaf, "company"),
        registry_code: text(leaf, "registry-code"),
        books_today: text(leaf, "books-today"),
        books_no_reason: text(leaf, "books-no-reason"),
        books_software: text(leaf, "books-software"),
        books_software_other: text(leaf, "books-software-other"),
        website: text(leaf, "website"),
        no_website: set(leaf, "no-website"),
        confirm_owners: set(leaf, "confirm-owners"),
        answers,
        consent: set(leaf, "consent"),
        pep: answer(leaf, "pep"),
        pep_related: answer(leaf, "pep-related"),
        pep_details: text(leaf, "pep-details"),
        invite_links: set(leaf, "invite-links"),
        language: text(leaf, "language"),
        plan: text(leaf, "plan"),
        no_wait: set(leaf, "no-wait"),
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::cli;

    fn parse(args: &[&str]) -> ArgMatches {
        cli::command()
            .try_get_matches_from(args)
            .unwrap_or_else(|error| panic!("{error}"))
    }

    fn onboard(matches: &ArgMatches) -> &ArgMatches {
        matches.subcommand_matches("onboard").unwrap()
    }

    #[test]
    fn reads_every_flag() {
        let file = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(file.path(), r#"{"cash_usage": "no"}"#).unwrap();
        let path = file.path().to_str().unwrap();
        let matches = parse(&[
            "oscar",
            "onboard",
            "-c",
            "acme",
            "--registry-code",
            "12345678",
            "--books-today",
            "yes",
            "--books-software",
            "other",
            "--books-software-other",
            "Excel",
            "--books-no-reason",
            "x",
            "--website",
            "acme.ee",
            "--confirm-owners",
            "--answers",
            path,
            "--answer",
            "needs_licence=no",
            "--answer",
            "customer_countries=ee,fi",
            "--consent",
            "--pep",
            "no",
            "--pep-related",
            "yes",
            "--pep-details",
            "Mayor",
            "--invite-links",
            "--language",
            "et",
            "--plan",
            "year",
            "--no-wait",
        ]);
        let options = options(onboard(&matches)).unwrap();
        assert_eq!(options.company.as_deref(), Some("acme"));
        assert_eq!(options.registry_code.as_deref(), Some("12345678"));
        assert_eq!(options.books_today.as_deref(), Some("yes"));
        assert_eq!(options.books_software_other.as_deref(), Some("Excel"));
        assert_eq!(options.website.as_deref(), Some("acme.ee"));
        assert!(
            options.confirm_owners && options.consent && options.invite_links && options.no_wait
        );
        assert!(!options.no_website);
        assert_eq!(
            serde_json::Value::Object(options.answers),
            json!({"cash_usage": "no", "needs_licence": false, "customer_countries": ["EE", "FI"]})
        );
        assert_eq!(
            (options.pep, options.pep_related),
            (Some(false), Some(true))
        );
        assert_eq!(options.language.as_deref(), Some("et"));
        assert_eq!(options.plan.as_deref(), Some("year"));
    }

    #[test]
    fn the_subcommands() {
        let matches = parse(&["oscar", "onboard", "status", "-c", "acme", "--json"]);
        let (name, leaf) = onboard(&matches).subcommand().unwrap();
        assert_eq!(name, "status");
        assert_eq!(options(leaf).unwrap().company.as_deref(), Some("acme"));
        let matches = parse(&["oscar", "onboard", "agreement", "--download"]);
        let (_, leaf) = onboard(&matches).subcommand().unwrap();
        assert_eq!(string(leaf, "download"), Some(""));
        let matches = parse(&[
            "oscar",
            "onboard",
            "agreement",
            "--download",
            "x.pdf",
            "--language",
            "en",
        ]);
        let (_, leaf) = onboard(&matches).subcommand().unwrap();
        assert_eq!(string(leaf, "download"), Some("x.pdf"));
        assert_eq!(options(leaf).unwrap().language.as_deref(), Some("en"));
        assert!(
            cli::command()
                .try_get_matches_from(["oscar", "onboard", "--plan", "weekly"])
                .is_err()
        );
        assert!(
            cli::command()
                .try_get_matches_from(["oscar", "onboard", "--website", "x", "--no-website"])
                .is_err()
        );
    }

    #[test]
    fn bad_answers_are_usage_errors() {
        let matches = parse(&["oscar", "onboard", "--answer", "cash_usage=lots"]);
        assert_eq!(
            options(onboard(&matches)).unwrap_err().code,
            crate::error::EXIT_USAGE
        );
        let matches = parse(&["oscar", "onboard", "--answers", "/no/such/file.json"]);
        assert!(
            options(onboard(&matches))
                .unwrap_err()
                .message
                .starts_with("Cannot read")
        );
    }
}
