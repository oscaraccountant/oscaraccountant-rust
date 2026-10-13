//! The prompts of the CLI and its spinner.
//!
//! Every prompt has a flag. A [`Prompter`] asks only on a terminal: standard input and standard
//! error are terminals, and neither `--no-input` nor `OSCAR_NO_INPUT=1` is given. Otherwise a
//! prompt never waits: it fails at once with a usage error that names the flag
//! ([`Prompter::missing`]).
//!
//! The prompts are `inquire` prompts on standard error. They block, so each runs on a blocking
//! thread of the runtime; the source of an autocomplete is async and runs on the runtime.
//!
//! ```ignore
//! let prompter = Prompter::for_app(app);
//! let company = prompter.select("Company", companies, "--company <x>").await?;
//! let code = prompter.text("Registry code", TextOptions::default(), "--registry-code <code>").await?;
//! let spinner = prompter.spinner("Waiting for the approval");
//! spinner.finish();
//! ```

use std::fmt::Display;
use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use indicatif::{ProgressBar, ProgressDrawTarget, ProgressStyle};
use inquire::autocompletion::{Autocomplete, Replacement};
use inquire::error::CustomUserError;
use inquire::ui::RenderConfig;
use inquire::validator::Validation;
use inquire::{Confirm, InquireError, Password, PasswordDisplayMode, Select, Text};

use crate::env::Env;
use crate::error::{CliError, Result};
use crate::render::theme;
use crate::session::App;

/// The variable that turns every prompt off, like `--no-input`.
pub const NO_INPUT_VAR: &str = "OSCAR_NO_INPUT";

/// The global flag that turns every prompt off.
pub const NO_INPUT_FLAG: &str = "--no-input";

/// Why the CLI may or may not ask.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Standard input and standard error are terminals: the CLI asks.
    Interactive,
    /// Standard input or standard error is not a terminal.
    NoTerminal,
    /// `--no-input` or `OSCAR_NO_INPUT=1`.
    NoInput,
}

/// A check of the text of a prompt: `Err` holds the message shown under the prompt.
pub type Check = Arc<dyn Fn(&str) -> std::result::Result<(), String> + Send + Sync>;

/// The options of a text prompt.
#[derive(Clone, Default)]
pub struct TextOptions {
    /// The value an empty answer takes.
    pub default: Option<String>,
    /// The text the field starts with.
    pub initial: Option<String>,
    /// A line of help under the prompt.
    pub help: Option<String>,
    /// A grey hint in the empty field.
    pub placeholder: Option<String>,
    /// A check of the answer; the prompt asks again while it fails.
    pub check: Option<Check>,
}

/// What a search prompt gives: an item of the source, or the text the person typed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pick<T> {
    /// A suggestion of the source.
    Item(T),
    /// Text that is no suggestion.
    Typed(String),
}

/// Asks the person, or says which flag to give.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Prompter {
    mode: Mode,
    /// Standard error is a terminal: the spinner draws.
    stderr_tty: bool,
}

/// `OSCAR_NO_INPUT` turns the prompts off: any value but `0` and `false`.
pub fn no_input_env(env: &Env) -> bool {
    env.var(NO_INPUT_VAR)
        .is_some_and(|value| value != "0" && !value.eq_ignore_ascii_case("false"))
}

impl Prompter {
    /// The prompter of a run: `no_input` is `--no-input`.
    pub fn new(env: &Env, no_input: bool) -> Self {
        let mode = if no_input || no_input_env(env) {
            Mode::NoInput
        } else if env.stdin_tty && env.stderr_tty {
            Mode::Interactive
        } else {
            Mode::NoTerminal
        };
        Self {
            mode,
            stderr_tty: env.stderr_tty,
        }
    }

    /// The prompter of the run of `app`.
    pub fn for_app(app: &App) -> Self {
        Self::new(&app.env, app.globals.no_input)
    }

    /// The CLI may ask.
    pub fn interactive(&self) -> bool {
        self.mode == Mode::Interactive
    }

    /// The usage error of a value the CLI cannot ask for: `<what> is needed. Give <flag>
    /// (<why it cannot ask>).`
    pub fn missing(&self, flag: &str, what: &str) -> CliError {
        let why = match self.mode {
            Mode::NoInput => format!("{NO_INPUT_FLAG} is set, so the CLI does not ask"),
            Mode::NoTerminal | Mode::Interactive => {
                "this is not a terminal, so the CLI does not ask".to_owned()
            }
        };
        CliError::usage(format!("{what} is needed. Give {flag} ({why})."))
    }

    fn check(&self, flag: &str, what: &str) -> Result<()> {
        if self.interactive() {
            Ok(())
        } else {
            Err(self.missing(flag, what))
        }
    }

    /// Choose one of `options`. Their `Display` is the line of each. With one option the
    /// person still confirms it by choosing.
    pub async fn select<T>(&self, message: &str, options: Vec<T>, flag: &str) -> Result<T>
    where
        T: Display + Send + 'static,
    {
        self.select_at(message, options, 0, flag).await
    }

    /// [`select`](Self::select), with the cursor on the option `start` (a previous choice).
    pub async fn select_at<T>(
        &self,
        message: &str,
        options: Vec<T>,
        start: usize,
        flag: &str,
    ) -> Result<T>
    where
        T: Display + Send + 'static,
    {
        self.check(flag, message)?;
        if options.is_empty() {
            return Err(CliError::usage(format!("Nothing to choose for {message}.")));
        }
        let start = start.min(options.len() - 1);
        let message = message.to_owned();
        blocking(move || {
            Select::new(&message, options)
                .with_render_config(render_config())
                .with_page_size(10)
                .with_starting_cursor(start)
                .prompt()
        })
        .await
    }

    /// Ask for a line of text.
    pub async fn text(&self, message: &str, options: TextOptions, flag: &str) -> Result<String> {
        self.check(flag, message)?;
        let message = message.to_owned();
        blocking(move || {
            let mut prompt = Text::new(&message).with_render_config(render_config());
            if let Some(default) = &options.default {
                prompt = prompt.with_default(default);
            }
            if let Some(initial) = &options.initial {
                prompt = prompt.with_initial_value(initial);
            }
            if let Some(help) = &options.help {
                prompt = prompt.with_help_message(help);
            }
            if let Some(placeholder) = &options.placeholder {
                prompt = prompt.with_placeholder(placeholder);
            }
            if let Some(check) = options.check.clone() {
                prompt = prompt.with_validator(move |text: &str| Ok(validation(&check, text)));
            }
            prompt.prompt()
        })
        .await
    }

    /// Ask for a secret, with no echo.
    pub async fn secret(&self, message: &str, flag: &str) -> Result<String> {
        self.check(flag, message)?;
        let message = message.to_owned();
        blocking(move || {
            Password::new(&message)
                .with_render_config(render_config())
                .with_display_mode(PasswordDisplayMode::Hidden)
                .without_confirmation()
                .prompt()
        })
        .await
    }

    /// Ask yes or no.
    pub async fn confirm(&self, message: &str, default: bool, flag: &str) -> Result<bool> {
        self.check(flag, message)?;
        let message = message.to_owned();
        blocking(move || {
            Confirm::new(&message)
                .with_render_config(render_config())
                .with_default(default)
                .prompt()
        })
        .await
    }

    /// Ask a yes or no question that has no safe default answer: with `default` None, Enter on
    /// an empty answer asks again, so a person never answers a risk question by pressing Enter.
    /// `default` is an answer given before. With no default the question says `(y/n)`, as a
    /// question with one says `(Y/n)`.
    pub async fn yes_no(&self, message: &str, default: Option<bool>, flag: &str) -> Result<bool> {
        self.check(flag, message)?;
        let message = yes_no_message(message, default);
        blocking(move || {
            let mut prompt = Confirm::new(&message)
                .with_render_config(render_config())
                .with_error_message("Answer y for yes or n for no.");
            if let Some(default) = default {
                prompt = prompt.with_default(default);
            }
            prompt.prompt()
        })
        .await
    }

    /// A text prompt whose suggestions come from `source` (a search of the API) as the person
    /// types. `source` gets the text, with at least `min_chars` characters, and gives the items;
    /// their `Display` is the line of each. The answer is the chosen item, or the typed text.
    pub async fn search<T, F, Fut>(
        &self,
        message: &str,
        help: Option<&str>,
        min_chars: usize,
        source: F,
        flag: &str,
    ) -> Result<Pick<T>>
    where
        T: Display + Clone + Send + Sync + 'static,
        F: Fn(String) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Vec<T>> + Send + 'static,
    {
        self.check(flag, message)?;
        let handle = tokio::runtime::Handle::current();
        let found: Arc<Mutex<Vec<T>>> = Arc::default();
        let source: Arc<dyn Fn(String) -> Vec<T> + Send + Sync> = {
            let found = found.clone();
            Arc::new(move |text: String| {
                let items = handle.block_on(source(text));
                if let Ok(mut found) = found.lock() {
                    found.clone_from(&items);
                }
                items
            })
        };
        let completer = Remote {
            source,
            min_chars,
            last: Arc::default(),
        };
        let message = message.to_owned();
        let help = help.map(str::to_owned);
        let answer = blocking(move || {
            let mut prompt = Text::new(&message)
                .with_render_config(render_config())
                .with_autocomplete(completer)
                .with_page_size(8);
            if let Some(help) = &help {
                prompt = prompt.with_help_message(help);
            }
            prompt.prompt()
        })
        .await?;
        let items = found.lock().map(|items| items.clone()).unwrap_or_default();
        Ok(pick(items, answer))
    }

    /// A spinner on standard error, with the message after it. Drawn only on a terminal.
    pub fn spinner(&self, message: &str) -> Spinner {
        let bar = if self.stderr_tty {
            let bar = ProgressBar::with_draw_target(None, ProgressDrawTarget::stderr());
            let ticks: &[&str] = if theme::current().unicode {
                &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏", " "]
            } else {
                &["-", "\\", "|", "/", " "]
            };
            if let Ok(style) = ProgressStyle::with_template("{spinner} {msg}") {
                bar.set_style(style.tick_strings(ticks));
            }
            bar.enable_steady_tick(Duration::from_millis(100));
            bar
        } else {
            ProgressBar::hidden()
        };
        bar.set_message(message.to_owned());
        Spinner { bar }
    }
}

/// A spinner of [`Prompter::spinner`]. Dropping it clears it.
#[derive(Debug)]
pub struct Spinner {
    bar: ProgressBar,
}

impl Spinner {
    /// Stop and clear the spinner.
    pub fn finish(&self) {
        self.bar.finish_and_clear();
    }
}

impl Drop for Spinner {
    fn drop(&mut self) {
        self.bar.finish_and_clear();
    }
}

/// The item whose line is the answer, or the answer as typed.
fn pick<T: Display>(items: Vec<T>, answer: String) -> Pick<T> {
    items
        .into_iter()
        .find(|item| item.to_string() == answer)
        .map_or(Pick::Typed(answer), Pick::Item)
}

fn validation(check: &Check, text: &str) -> Validation {
    match check(text) {
        Ok(()) => Validation::Valid,
        Err(message) => Validation::Invalid(message.into()),
    }
}

/// The suggestions of a search: the source is asked once per new text.
#[derive(Clone)]
struct Remote<T> {
    source: Arc<dyn Fn(String) -> Vec<T> + Send + Sync>,
    min_chars: usize,
    last: Arc<Mutex<(String, Vec<String>)>>,
}

impl<T: Display + Clone + Send + Sync + 'static> Autocomplete for Remote<T> {
    fn get_suggestions(
        &mut self,
        input: &str,
    ) -> std::result::Result<Vec<String>, CustomUserError> {
        let input = input.trim();
        if input.chars().count() < self.min_chars {
            return Ok(Vec::new());
        }
        let mut last = self.last.lock().map_err(|_| "the search failed")?;
        if last.0 != input || last.1.is_empty() {
            let lines = (self.source)(input.to_owned())
                .iter()
                .map(ToString::to_string)
                .collect();
            *last = (input.to_owned(), lines);
        }
        Ok(last.1.clone())
    }

    fn get_completion(
        &mut self,
        _input: &str,
        highlighted: Option<String>,
    ) -> std::result::Result<Replacement, CustomUserError> {
        Ok(highlighted)
    }
}

/// The look of the prompts: colours only when standard error has colour.
fn render_config() -> RenderConfig<'static> {
    if theme::current().color {
        RenderConfig::default_colored()
    } else {
        RenderConfig::empty()
    }
}

/// Run a prompt on a blocking thread and map its error.
async fn blocking<T, F>(prompt: F) -> Result<T>
where
    T: Send + 'static,
    F: FnOnce() -> std::result::Result<T, InquireError> + Send + 'static,
{
    tokio::task::spawn_blocking(prompt)
        .await
        .map_err(|error| CliError::failed(format!("The prompt failed: {error}")))?
        .map_err(prompt_error)
}

/// The question of [`Prompter::yes_no`]: with no default, `(y/n)` after it (N3-4).
fn yes_no_message(message: &str, default: Option<bool>) -> String {
    match default {
        None => format!("{message} (y/n)"),
        Some(_) => message.to_owned(),
    }
}

/// The error of a prompt: a cancel is a usage error, as a missing value is.
fn prompt_error(error: InquireError) -> CliError {
    match error {
        InquireError::OperationCanceled => CliError::usage("Cancelled."),
        // Ctrl-C: the code of an interrupt, as a shell gives it (N3-j).
        InquireError::OperationInterrupted => CliError::interrupted("Cancelled."),
        InquireError::NotTTY => CliError::usage(format!(
            "This is not a terminal. Give the value as a flag, or use {NO_INPUT_FLAG}."
        )),
        other => CliError::failed(format!("The prompt failed: {other}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::EXIT_USAGE;

    fn env(vars: &[(&str, &str)], tty: bool) -> Env {
        let mut env = Env::with_vars(vars);
        env.stdin_tty = tty;
        env.stderr_tty = tty;
        env
    }

    #[test]
    fn a_yes_no_question_says_how_to_answer() {
        assert_eq!(yes_no_message("Licence?", None), "Licence? (y/n)");
        // Inquire adds `(Y/n)` or `(y/N)` itself when there is a default.
        assert_eq!(yes_no_message("Licence?", Some(true)), "Licence?");
    }

    #[test]
    fn the_mode() {
        assert_eq!(
            Prompter::new(&env(&[], true), false).mode,
            Mode::Interactive
        );
        assert!(Prompter::new(&env(&[], true), false).interactive());
        assert_eq!(
            Prompter::new(&env(&[], false), false).mode,
            Mode::NoTerminal
        );
        assert_eq!(Prompter::new(&env(&[], true), true).mode, Mode::NoInput);
        for value in ["1", "true", "yes"] {
            let env = env(&[(NO_INPUT_VAR, value)], true);
            assert_eq!(Prompter::new(&env, false).mode, Mode::NoInput, "{value}");
        }
        for value in ["0", "false", "FALSE", ""] {
            let env = env(&[(NO_INPUT_VAR, value)], true);
            assert_eq!(
                Prompter::new(&env, false).mode,
                Mode::Interactive,
                "{value}"
            );
        }
        let mut half = env(&[], true);
        half.stderr_tty = false;
        assert_eq!(Prompter::new(&half, false).mode, Mode::NoTerminal);
    }

    #[test]
    fn missing_names_the_flag() {
        let error = Prompter::new(&env(&[], false), false).missing("--company <x>", "A company");
        assert_eq!(error.code, EXIT_USAGE);
        assert_eq!(
            error.message,
            "A company is needed. Give --company <x> (this is not a terminal, so the CLI does not ask)."
        );
        let error = Prompter::new(&env(&[], true), true).missing("--yes", "A confirmation");
        assert!(
            error
                .message
                .ends_with("(--no-input is set, so the CLI does not ask).")
        );
    }

    #[tokio::test]
    async fn every_prompt_fails_at_once_without_input() {
        for prompter in [
            Prompter::new(&env(&[], false), false),
            Prompter::new(&env(&[], true), true),
            Prompter::new(&env(&[(NO_INPUT_VAR, "1")], true), false),
        ] {
            let select = prompter
                .select("Company", vec!["a", "b"], "--company")
                .await;
            assert!(select.unwrap_err().message.contains("Give --company"));
            let text = prompter
                .text("Name", TextOptions::default(), "--name <name>")
                .await;
            assert!(text.unwrap_err().message.contains("Give --name <name>"));
            let secret = prompter.secret("API key", "--api-key").await;
            assert_eq!(secret.unwrap_err().code, EXIT_USAGE);
            let confirm = prompter.confirm("Go on?", true, "--yes").await;
            assert!(confirm.unwrap_err().message.starts_with("Go on? is needed"));
            let yes_no = prompter.yes_no("Crypto?", None, "--answer x=yes|no").await;
            assert!(
                yes_no
                    .unwrap_err()
                    .message
                    .contains("Give --answer x=yes|no")
            );
            let search = prompter
                .search(
                    "Company",
                    None,
                    2,
                    |_text: String| async { vec!["x".to_owned()] },
                    "--registry-code <code>",
                )
                .await;
            assert!(search.unwrap_err().message.contains("--registry-code"));
        }
    }

    #[tokio::test]
    async fn an_empty_choice_is_an_error() {
        let prompter = Prompter::new(&env(&[], true), false);
        let error = prompter
            .select::<&str>("Company", vec![], "--company")
            .await
            .unwrap_err();
        assert_eq!(error.message, "Nothing to choose for Company.");
    }

    #[tokio::test]
    async fn a_prompt_with_no_terminal_says_so() {
        // The process of a test has no terminal: inquire refuses, and the CLI maps the error.
        let result = blocking(|| Err::<(), _>(InquireError::NotTTY)).await;
        assert!(
            result
                .unwrap_err()
                .message
                .starts_with("This is not a terminal")
        );
        let cancel = blocking(|| Err::<(), _>(InquireError::OperationCanceled)).await;
        assert_eq!(cancel.unwrap_err().message, "Cancelled.");
        let interrupt = blocking(|| Err::<(), _>(InquireError::OperationInterrupted)).await;
        assert_eq!(interrupt.unwrap_err().code, crate::error::EXIT_INTERRUPTED);
        let other = blocking(|| Err::<(), _>(InquireError::InvalidConfiguration("x".into()))).await;
        assert_eq!(other.unwrap_err().code, crate::error::EXIT_API);
        assert_eq!(blocking(|| Ok(3)).await.unwrap(), 3);
    }

    #[test]
    fn picks_an_item() {
        let items = vec!["Alpha OÜ".to_owned(), "Beta AS".to_owned()];
        assert_eq!(
            pick(items.clone(), "Beta AS".into()),
            Pick::Item("Beta AS".to_owned())
        );
        assert_eq!(pick(items, "Gamma".into()), Pick::Typed("Gamma".into()));
        let check: Check = Arc::new(|text: &str| {
            if text.len() == 8 {
                Ok(())
            } else {
                Err("8 digits".into())
            }
        });
        assert_eq!(validation(&check, "12345678"), Validation::Valid);
        assert!(matches!(validation(&check, "1"), Validation::Invalid(_)));
    }

    #[test]
    fn the_remote_source_is_asked_once_per_text() {
        let calls = Arc::new(Mutex::new(0));
        let counted = calls.clone();
        let mut remote = Remote {
            source: Arc::new(move |text: String| {
                *counted.lock().unwrap() += 1;
                vec![format!("{text} 1"), format!("{text} 2")]
            }),
            min_chars: 2,
            last: Arc::default(),
        };
        assert!(remote.get_suggestions("a").unwrap().is_empty());
        assert_eq!(remote.get_suggestions("ab").unwrap(), ["ab 1", "ab 2"]);
        assert_eq!(remote.get_suggestions(" ab ").unwrap().len(), 2);
        assert_eq!(*calls.lock().unwrap(), 1);
        assert_eq!(remote.get_suggestions("abc").unwrap()[0], "abc 1");
        assert_eq!(*calls.lock().unwrap(), 2);
        assert_eq!(
            remote.get_completion("ab", Some("ab 1".into())).unwrap(),
            Some("ab 1".into())
        );
    }

    #[test]
    fn the_spinner_is_hidden_without_a_terminal() {
        let spinner = Prompter::new(&env(&[], false), false).spinner("Waiting");
        spinner.finish();
        let spinner = Prompter::new(&env(&[], true), false).spinner("Waiting");
        drop(spinner);
        let _ascii = theme::scoped(theme::Theme {
            unicode: false,
            ..theme::Theme::PLAIN
        });
        let _ = Prompter::new(&env(&[], true), false).spinner("Waiting");
        let _ = render_config();
        let _colour = theme::scoped(theme::Theme {
            color: true,
            ..theme::Theme::PLAIN
        });
        let _ = render_config();
    }
}
