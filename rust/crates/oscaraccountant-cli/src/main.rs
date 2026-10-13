//! `oscar`, the command-line tool of the Oscar accounting API.
//!
//! The behaviour follows the shared rules of every Oscar client (`spec/README.md` of the
//! repository `oscaraccountant-rust`): the credentials, the store, the requests, the OAuth login
//! and the exit codes. The command table is generated
//! from the OpenAPI document of the public API by `oscar-codegen`, in the module `generated`.

#[rustfmt::skip]
mod generated;

mod app;
mod browser;
mod cli;
mod commands;
mod company;
mod config;
mod env;
mod error;
mod loopback;
mod oauth;
mod onboarding;
mod output;
mod prompt;
mod render;
mod secrets;
mod session;
mod time;

use std::process::ExitCode;

fn main() -> ExitCode {
    let env = env::Env::process();
    let out = output::Output::process(&env);
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("error: cannot start: {error}");
            return ExitCode::from(error::EXIT_API);
        }
    };
    let code = runtime.block_on(app::run(std::env::args_os(), env, out));
    ExitCode::from(code)
}
