//! One run of `oscar`: parse the arguments, run the command, print the error, give the exit code.

use std::ffi::OsString;

use clap::error::{ContextKind, ContextValue, ErrorKind};

use crate::cli;
use crate::commands;
use crate::config::Paths;
use crate::env::Env;
use crate::error::Result;
use crate::output::Output;
use crate::secrets::Secrets;
use crate::session::{App, VERSION};

/// An unknown command of a resource names the commands it has (P2-25), never a hidden alias
/// such as the SDK method `retrieve` that clap would suggest.
fn command_tip(error: clap::Error, args: &[OsString]) -> clap::Error {
    if error.kind() != ErrorKind::InvalidSubcommand {
        return error;
    }
    let root = cli::command();
    let Some(group) = args
        .iter()
        .skip(1)
        .filter_map(|arg| arg.to_str())
        .find_map(|arg| root.find_subcommand(arg))
    else {
        return error;
    };
    let names = cli::visible_subcommands(group);
    let (Some(ContextValue::String(wrong)), false) =
        (error.get(ContextKind::InvalidSubcommand), names.is_empty())
    else {
        return error;
    };
    let mut better = clap::Error::new(ErrorKind::InvalidSubcommand).with_cmd(group);
    better.insert(
        ContextKind::InvalidSubcommand,
        ContextValue::String(wrong.clone()),
    );
    let tip = format!(
        "the commands of `oscar {}`: {}",
        group.get_name(),
        names.join(", ")
    );
    better.insert(
        ContextKind::Suggested,
        ContextValue::StyledStrs(vec![tip.into()]),
    );
    if let Some(usage) = error.get(ContextKind::Usage) {
        better.insert(ContextKind::Usage, usage.clone());
    }
    better
}

/// Run `oscar` with these arguments (the first is the program) and give the exit code.
pub async fn run<I, T>(args: I, env: Env, mut out: Output) -> u8
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    let args: Vec<OsString> = args.into_iter().map(Into::into).collect();
    let matches = match cli::command().try_get_matches_from(&args) {
        Ok(matches) => matches,
        Err(error) => {
            let error = command_tip(error, &args);
            let text = if out.color {
                error.render().ansi().to_string()
            } else {
                error.render().to_string()
            };
            let text = text.trim_end();
            if matches!(
                error.kind(),
                ErrorKind::DisplayHelp | ErrorKind::DisplayVersion
            ) {
                out.line(text);
            } else {
                out.note(text);
            }
            out.flush();
            return u8::try_from(error.exit_code()).unwrap_or(2);
        }
    };
    if cli::flag(&matches, "version") {
        out.line(&format!("oscar-cli {VERSION}"));
        out.flush();
        return 0;
    }
    let Some((name, sub)) = matches.subcommand() else {
        out.line(&cli::command().render_help().to_string());
        out.flush();
        return 2;
    };
    let globals = cli::globals(&matches, commands::leaf(sub));
    let mut app = match prepare(&env) {
        Ok((paths, secrets)) => App {
            env,
            out,
            paths,
            secrets,
            globals,
        },
        Err(error) => {
            out.error(&error);
            out.flush();
            return error.code;
        }
    };
    let result = commands::dispatch(&mut app, name, sub).await;
    for notice in app.secrets.take_notices() {
        app.out.note(&notice);
    }
    let code = match result {
        Ok(()) => 0,
        Err(error) => {
            app.out.error(&error);
            error.code
        }
    };
    app.out.flush();
    code
}

fn prepare(env: &Env) -> Result<(Paths, Secrets)> {
    let paths = Paths::resolve(env)?;
    let secrets = Secrets::new(env, &paths)?;
    Ok((paths, secrets))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::secrets::STORE_VARIABLE;

    async fn run_with(args: &[&str], vars: &[(&str, &str)]) -> (u8, String, String) {
        let (out, stdout, stderr) = Output::buffers();
        let code = run(args.iter().copied(), Env::with_vars(vars), out).await;
        (code, stdout.text(), stderr.text())
    }

    #[tokio::test]
    async fn prints_the_version() {
        let expected = format!("oscar-cli {}\n", env!("CARGO_PKG_VERSION"));
        let (code, out, _) = run_with(&["oscar", "--version"], &[]).await;
        assert_eq!((code, out.as_str()), (0, expected.as_str()));
        let (code, out, _) = run_with(&["oscar", "-V"], &[]).await;
        assert_eq!((code, out.as_str()), (0, expected.as_str()));
    }

    #[test]
    fn the_version_is_clients_version() {
        let file = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../VERSION");
        if let Ok(text) = std::fs::read_to_string(file) {
            assert_eq!(text.trim(), VERSION);
        }
    }

    #[tokio::test]
    async fn an_unknown_command_names_the_real_ones() {
        let (code, _, err) = run_with(&["oscar", "invoices", "create"], &[]).await;
        assert_eq!(code, 2);
        assert!(
            err.contains(
                "tip: the commands of `oscar invoices`: cancel, issue, list, record, get, revise"
            ),
            "{err}"
        );
        assert!(err.contains("'create'"), "{err}");
        assert!(!err.contains("retrieve"), "{err}");
        assert!(err.contains("Usage: oscar invoices"), "{err}");
        // With a global flag first, and for an unknown resource: clap's own message.
        let (_, _, err) = run_with(&["oscar", "--json", "vat-codes", "lst"], &[]).await;
        assert!(err.contains("`oscar vat-codes`: list"), "{err}");
        let (code, _, err) = run_with(&["oscar", "invoicez"], &[]).await;
        assert_eq!(code, 2);
        assert!(err.contains("invoices"), "{err}");
    }

    #[tokio::test]
    async fn usage_errors_and_help() {
        let (code, _, err) = run_with(&["oscar", "invoices", "list", "--nope"], &[]).await;
        assert_eq!(code, 2);
        assert!(err.contains("--nope"));
        let (code, out, _) = run_with(&["oscar", "--help"], &[]).await;
        assert_eq!(code, 0);
        assert!(out.contains("Exit codes"));
        let (code, _, err) = run_with(&["oscar"], &[]).await;
        assert_eq!(code, 2);
        assert!(err.contains("Usage"));
        let (code, out, _) = run_with(&["oscar", "--json"], &[]).await;
        assert_eq!(code, 2);
        assert!(out.contains("Usage"));
    }

    #[tokio::test]
    async fn a_bad_store_setting_is_a_usage_error() {
        let dir = tempfile::tempdir().unwrap();
        let dir = dir.path().to_str().unwrap();
        let (code, _, err) = run_with(
            &["oscar", "profiles", "list"],
            &[("OSCAR_CONFIG_DIR", dir), (STORE_VARIABLE, "vault")],
        )
        .await;
        assert_eq!(code, 2);
        assert!(err.contains("OSCAR_CREDENTIAL_STORE"));
    }

    #[tokio::test]
    async fn runs_a_local_command() {
        let dir = tempfile::tempdir().unwrap();
        let dir = dir.path().to_str().unwrap();
        let vars = [("OSCAR_CONFIG_DIR", dir), (STORE_VARIABLE, "file")];
        let (code, out, err) = run_with(&["oscar", "profiles", "list", "--json"], &vars).await;
        assert_eq!((code, out.as_str(), err.as_str()), (0, "[]\n", ""));
        let (code, _, err) = run_with(&["oscar", "whoami"], &vars).await;
        assert_eq!(code, 3);
        assert!(err.starts_with("error: Not logged in"), "{err}");
        let (code, out, _) = run_with(&["oscar", "completion", "bash"], &vars).await;
        assert_eq!(code, 0);
        assert!(out.contains("oscar"));
    }
}
