//! `oscar completion <shell>`.

use clap::ArgMatches;
use clap_complete::Shell;

use crate::cli;
use crate::error::{CliError, Result};
use crate::session::App;

/// Print the completion script of a shell.
pub fn run(app: &mut App, matches: &ArgMatches) -> Result<()> {
    let shell = *matches
        .get_one::<Shell>("shell")
        .ok_or_else(|| CliError::usage("Give a shell."))?;
    // `clap_complete::generate` panics when a write fails: write the script to memory, then to
    // standard output, where a closed pipe (`oscar completion bash | head`) is not an error.
    let mut script = Vec::new();
    clap_complete::generate(shell, &mut cli::command(), "oscar", &mut script);
    let _ = app.out.stdout().write_all(&script);
    Ok(())
}
