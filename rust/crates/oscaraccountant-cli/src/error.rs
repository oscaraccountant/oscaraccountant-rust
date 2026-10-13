//! The errors of the CLI and their exit codes (`clients/spec/README.md`, section 4.7).

use std::fmt;

use serde_json::Value;

/// An error of the API.
pub const EXIT_API: u8 = 1;
/// A usage error.
pub const EXIT_USAGE: u8 = 2;
/// Authentication needed.
pub const EXIT_AUTH: u8 = 3;
/// A network error.
pub const EXIT_NETWORK: u8 = 4;
/// Stopped with Ctrl-C (an interrupt): 128 + SIGINT, as a shell reports it.
pub const EXIT_INTERRUPTED: u8 = 130;

/// An error that ends a command: its exit code, and what to print.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CliError {
    /// The exit code.
    pub code: u8,
    /// One line for standard error.
    pub message: String,
    /// More lines, for example one per invalid field.
    pub details: Vec<String>,
    /// The body of an API error, which `oscar api` prints on standard output.
    pub body: Option<String>,
}

/// The result of a command.
pub type Result<T> = std::result::Result<T, CliError>;

impl CliError {
    fn new(code: u8, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            details: Vec::new(),
            body: None,
        }
    }

    /// A usage error: exit code 2.
    pub fn usage(message: impl Into<String>) -> Self {
        Self::new(EXIT_USAGE, message)
    }

    /// Authentication needed: exit code 3.
    pub fn auth(message: impl Into<String>) -> Self {
        Self::new(EXIT_AUTH, message)
    }

    /// Any other failure: exit code 1.
    pub fn failed(message: impl Into<String>) -> Self {
        Self::new(EXIT_API, message)
    }

    /// A network error: exit code 4.
    pub fn network(message: impl Into<String>) -> Self {
        Self::new(EXIT_NETWORK, message)
    }

    /// Stopped with Ctrl-C: exit code 130.
    pub fn interrupted(message: impl Into<String>) -> Self {
        Self::new(EXIT_INTERRUPTED, message)
    }

    /// A failed file operation on `path`.
    pub fn io(action: &str, path: &std::path::Path, error: &std::io::Error) -> Self {
        Self::failed(format!("Cannot {action} {}: {error}", path.display()))
    }
}

impl fmt::Display for CliError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

/// "Log in first", the hint of every authentication error.
pub const LOGIN_HINT: &str = "Run `oscar login`, or set OSCAR_API_KEY.";

impl From<oscaraccountant::Error> for CliError {
    fn from(error: oscaraccountant::Error) -> Self {
        use oscaraccountant::Error;
        let mut cli = match &error {
            Error::Authentication(auth) => match &auth.api {
                Some(_) => Self::auth(format!("{error}. {LOGIN_HINT}")),
                None => Self::auth(auth.message.clone()),
            },
            Error::Config(message) => Self::usage(message.clone()),
            error if error.is_network() => Self::network(error.to_string()),
            Error::Json(json) => Self::failed(format!("The API answered unexpected JSON: {json}")),
            _ => match error.api_error().filter(|api| api.code == "unknown") {
                Some(api) => Self::failed(not_a_problem(api)),
                None => Self::failed(error.to_string()),
            },
        };
        if let Some(api) = error.api_error() {
            cli.details = api.errors.iter().map(describe_field_error).collect();
            cli.body = Some(api.body.clone());
        }
        cli
    }
}

/// The variable that prints the whole body of an error answer.
pub const DEBUG_VARIABLE: &str = "OSCAR_DEBUG";

/// The longest line of a body that is not a problem that the error repeats.
const SHORT_LINE: usize = 100;

/// The message of an error answer whose body is not a problem (an HTML page of a proxy, a plain
/// text): the status, its reason, and at most one short line of the body, never the body.
fn not_a_problem(api: &oscaraccountant::ApiError) -> String {
    let body = api.body.trim();
    let html = body.starts_with('<');
    let line = if html {
        tag_text(body, "title").or_else(|| tag_text(body, "h1"))
    } else {
        body.lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .map(str::to_owned)
    }
    .map(|line| crate::render::format::cut(&crate::render::format::one_line(&line), SHORT_LINE));
    let what = match (html, body.is_empty()) {
        (_, true) => "an empty answer".to_owned(),
        (true, _) => "an HTML page".to_owned(),
        (false, _) => "text".to_owned(),
    };
    let mut message = format!("{} {}: the server answered {what}", api.status, api.title);
    if let Some(line) = line.filter(|line| !line.is_empty()) {
        message.push_str(&format!(" (\"{line}\")"));
    }
    message.push_str(", not JSON");
    if let Some(id) = &api.request_id {
        message.push_str(&format!(" (request {id})"));
    }
    message.push('.');
    if !body.is_empty() {
        message.push_str(&format!(" {DEBUG_VARIABLE}=1 prints the whole answer."));
    }
    message
}

/// The text of the first `<tag>` of an HTML page, its spaces collapsed.
fn tag_text(html: &str, tag: &str) -> Option<String> {
    let lower = html.to_ascii_lowercase();
    let open = lower.find(&format!("<{tag}"))?;
    let start = open + lower[open..].find('>')? + 1;
    let end = start + lower[start..].find(&format!("</{tag}"))?;
    let text = html
        .get(start..end)?
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    (!text.is_empty()).then_some(text)
}

/// One entry of the `errors` of a problem, as one line: `<field>: <detail>`.
fn describe_field_error(entry: &Value) -> String {
    let field = ["attr", "field", "pointer", "name"]
        .iter()
        .find_map(|key| entry.get(*key).and_then(Value::as_str));
    let detail = ["detail", "message"]
        .iter()
        .find_map(|key| entry.get(*key).and_then(Value::as_str));
    match (field, detail) {
        (Some(field), Some(detail)) => format!("{field}: {detail}"),
        (None, Some(detail)) => detail.to_owned(),
        _ => entry.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use oscaraccountant::{ApiError, Error};
    use serde_json::json;

    use super::*;

    fn api(status: u16, body: &str) -> Error {
        Error::from_api(ApiError::from_response(
            status,
            Some("req-1".into()),
            body.as_bytes(),
        ))
    }

    #[test]
    fn maps_the_exit_codes() {
        assert_eq!(CliError::from(Error::authentication("no")).code, EXIT_AUTH);
        assert_eq!(CliError::from(api(401, "{}")).code, EXIT_AUTH);
        assert_eq!(
            CliError::from(Error::Connection("refused".into())).code,
            EXIT_NETWORK
        );
        assert_eq!(
            CliError::from(Error::Timeout(Duration::from_secs(1))).code,
            EXIT_NETWORK
        );
        assert_eq!(
            CliError::from(Error::Config("bad url".into())).code,
            EXIT_USAGE
        );
        assert_eq!(CliError::from(api(404, "{}")).code, EXIT_API);
        assert_eq!(CliError::from(api(500, "oops")).code, EXIT_API);
        let json = serde_json::from_str::<Value>("{").unwrap_err();
        let json = CliError::from(Error::Json(json));
        assert_eq!(json.code, EXIT_API);
        assert!(json.message.starts_with("The API answered unexpected JSON"));
    }

    #[test]
    fn a_401_says_how_to_log_in() {
        let error = CliError::from(api(
            401,
            r#"{"code": "authentication_failed", "detail": "Invalid token."}"#,
        ));
        assert_eq!(
            error.message,
            "401 authentication_failed: Invalid token. (request req-1). Run `oscar login`, or set OSCAR_API_KEY."
        );
        assert_eq!(
            CliError::from(Error::authentication("Not logged in.")).message,
            "Not logged in."
        );
    }

    #[test]
    fn keeps_the_field_errors_and_the_body() {
        let body = json!({
            "code": "validation_error",
            "detail": "Invalid input.",
            "errors": [
                {"attr": "limit", "detail": "Too big."},
                {"detail": "Something."},
                {"code": "x"},
            ],
        })
        .to_string();
        let error = CliError::from(api(400, &body));
        assert_eq!(error.code, EXIT_API);
        assert_eq!(
            error.details,
            vec!["limit: Too big.", "Something.", r#"{"code":"x"}"#]
        );
        assert_eq!(error.body.as_deref(), Some(body.as_str()));
        assert_eq!(error.to_string(), error.message);
    }

    #[test]
    fn a_body_that_is_not_a_problem_is_summed_up() {
        // Self-test N2-2: the whole Django 404 page after `error: 404 unknown:`.
        let page = format!(
            "<!DOCTYPE html>\n<html><head><title>Page not found at /api/companies/me/</title></head><body>{}</body></html>",
            "<p>line</p>\n".repeat(400)
        );
        let error = CliError::from(api(404, &page));
        assert_eq!(
            error.message,
            "404 Not Found: the server answered an HTML page (\"Page not found at /api/companies/me/\"), not JSON (request req-1). OSCAR_DEBUG=1 prints the whole answer."
        );
        assert_eq!(error.code, EXIT_API);
        assert_eq!(error.body.as_deref(), Some(page.as_str()));
        let text = CliError::from(api(502, "\n  Bad gateway: upstream down\nmore\n"));
        assert_eq!(
            text.message,
            "502 Bad Gateway: the server answered text (\"Bad gateway: upstream down\"), not JSON (request req-1). OSCAR_DEBUG=1 prints the whole answer."
        );
        let empty = CliError::from(api(503, ""));
        assert_eq!(
            empty.message,
            "503 Service Unavailable: the server answered an empty answer, not JSON (request req-1)."
        );
        let long = CliError::from(api(500, &"x".repeat(5000)));
        assert!(long.message.len() < 250, "{}", long.message);
        assert_eq!(
            tag_text("<h1 class=a>\n Oops \n</h1>", "h1").as_deref(),
            Some("Oops")
        );
        assert_eq!(tag_text("<title></title>", "title"), None);
        assert_eq!(tag_text("<p>x</p>", "title"), None);
    }

    #[test]
    fn constructors() {
        assert_eq!(CliError::usage("u").code, EXIT_USAGE);
        assert_eq!(CliError::auth("a").code, EXIT_AUTH);
        assert_eq!(CliError::failed("f").code, EXIT_API);
        assert_eq!(CliError::network("n").code, EXIT_NETWORK);
        let io = std::io::Error::other("denied");
        let error = CliError::io("read", std::path::Path::new("/x"), &io);
        assert_eq!(error.message, "Cannot read /x: denied");
    }
}
