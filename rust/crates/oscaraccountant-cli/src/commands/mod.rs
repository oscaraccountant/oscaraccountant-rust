//! The commands of `oscar`.

pub mod api;
pub mod auth;
pub mod companies;
pub mod completion;
pub mod files;
pub mod login;
pub mod logout;
pub mod onboard;
pub mod operation;
pub mod profiles;
pub mod settings;
pub mod whoami;

use std::io::Read;

use clap::ArgMatches;
use oscaraccountant::HttpMethod;
use serde_json::Value;

use crate::cli::{self, HAND_WRITTEN};
use crate::error::{CliError, Result};
use crate::generated::{COMMANDS, Command};
use crate::session::App;

/// Run the command `name` with its arguments.
pub async fn dispatch(app: &mut App, name: &str, matches: &ArgMatches) -> Result<()> {
    match (name, matches.subcommand()) {
        ("login", _) => login::run(app, matches).await,
        ("onboard", _) => onboard::run(app, matches).await,
        ("logout", _) => logout::run(app).await,
        ("whoami", _) => whoami::run(app).await,
        ("auth", Some(("token", _))) => auth::token(app).await,
        ("auth", Some(("status", _))) => auth::status(app).await,
        ("profiles", Some(("list", _))) => profiles::list(app),
        ("profiles", Some(("use", leaf))) => profiles::use_profile(app, leaf),
        ("config", Some((action, leaf))) => settings::run(app, action, leaf),
        ("api", _) => api::run(app, matches).await,
        ("companies", Some(("use", leaf))) => companies::use_company(app, leaf).await,
        ("completion", _) => completion::run(app, matches),
        (resource, sub) => {
            let (method, leaf) = sub.unwrap_or(("", matches));
            let operation = find(resource, method).ok_or_else(|| {
                let shown = format!("{resource} {method}");
                CliError::usage(format!("Unknown command `{}`.", shown.trim_end()))
            })?;
            match (operation.resource, operation.method) {
                ("files", "upload") => files::upload(app, operation, leaf).await,
                ("files", "download") => files::download(app, leaf).await,
                key => {
                    debug_assert!(!HAND_WRITTEN.contains(&key));
                    operation::run(app, operation, leaf).await
                }
            }
        }
    }
}

/// The operation of `oscar <resource> <name>`: by its CLI name, or by its SDK method (the hidden
/// alias). An empty name is the resource itself.
pub fn find(resource: &str, name: &str) -> Option<&'static Command> {
    COMMANDS.iter().find(|command| {
        command.resource == resource
            && (command.name == name || (!name.is_empty() && command.method == name))
    })
}

/// The deepest subcommand of a match: the leaf that holds the arguments.
pub fn leaf(matches: &ArgMatches) -> &ArgMatches {
    match matches.subcommand() {
        Some((_, sub)) => leaf(sub),
        None => matches,
    }
}

/// The JSON of `--data`: `'<json>'`, `@file.json`, or `-` for standard input.
pub fn read_data(value: &str) -> Result<Value> {
    let text = if value == "-" {
        let mut text = String::new();
        std::io::stdin()
            .read_to_string(&mut text)
            .map_err(|error| CliError::usage(format!("Cannot read standard input: {error}")))?;
        text
    } else if let Some(path) = value.strip_prefix('@') {
        std::fs::read_to_string(path)
            .map_err(|error| CliError::usage(format!("Cannot read {path}: {error}")))?
    } else {
        value.to_owned()
    };
    serde_json::from_str(&text)
        .map_err(|error| CliError::usage(format!("--data is not valid JSON: {error}")))
}

/// An HTTP method by name, in any case.
pub fn method(name: &str) -> Result<HttpMethod> {
    match name.to_ascii_uppercase().as_str() {
        "GET" => Ok(HttpMethod::Get),
        "POST" => Ok(HttpMethod::Post),
        "PUT" => Ok(HttpMethod::Put),
        "PATCH" => Ok(HttpMethod::Patch),
        "DELETE" => Ok(HttpMethod::Delete),
        _ => Err(CliError::usage(format!(
            "`{name}` is not a method: use GET, POST, PUT, PATCH or DELETE."
        ))),
    }
}

/// A string argument of the leaf.
pub fn string<'a>(leaf: &'a ArgMatches, id: &str) -> Option<&'a str> {
    cli::string(leaf, id)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn reads_data() {
        assert_eq!(read_data(r#"{"a": 1}"#).unwrap(), json!({"a": 1}));
        let file = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(file.path(), "[1, 2]").unwrap();
        assert_eq!(
            read_data(&format!("@{}", file.path().display())).unwrap(),
            json!([1, 2])
        );
        let error = read_data("{").unwrap_err();
        assert_eq!(error.code, crate::error::EXIT_USAGE);
        assert!(error.message.starts_with("--data is not valid JSON"));
        assert!(
            read_data("@/no/such/file.json")
                .unwrap_err()
                .message
                .starts_with("Cannot read")
        );
    }

    #[test]
    fn methods() {
        assert_eq!(method("get").unwrap(), HttpMethod::Get);
        assert_eq!(method("POST").unwrap(), HttpMethod::Post);
        assert_eq!(method("put").unwrap(), HttpMethod::Put);
        assert_eq!(method("Patch").unwrap(), HttpMethod::Patch);
        assert_eq!(method("delete").unwrap(), HttpMethod::Delete);
        assert_eq!(method("FETCH").unwrap_err().code, crate::error::EXIT_USAGE);
    }

    #[test]
    fn finds_the_leaf() {
        let matches = cli::command()
            .try_get_matches_from(["oscar", "auth", "token", "--json"])
            .unwrap();
        assert_eq!(cli::string(leaf(&matches), "json"), Some(""));
    }
}
