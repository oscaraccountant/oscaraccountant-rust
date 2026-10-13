//! The loopback redirect of the OAuth login (`clients/spec/README.md`, section 4.2, RFC 8252): a
//! listener on `127.0.0.1` (or `[::1]`) that takes the first `GET /callback`, answers it, and
//! closes.
//!
//! The answer is a redirect (302) to a page of the web app: `<base URL>/cli/done`, or
//! `<base URL>/cli/error?reason=<code>`. The page
//! gets no code and no token. A server with no such page (an older server) gets a plain page of
//! the CLI instead: [`has_pages`] asks before the login.

use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};
use std::time::Duration;

use oscaraccountant::{Client, HttpMethod};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use crate::error::{CliError, Result};

/// The longest wait for the browser.
pub const LOGIN_TIMEOUT: Duration = Duration::from_secs(300);
/// The longest request head the listener reads.
const HEAD_LIMIT: usize = 16 * 1024;
/// The longest wait for the head of one connection.
const READ_TIMEOUT: Duration = Duration::from_secs(10);
/// The longest wait for the check of the pages of the web app.
const PROBE_TIMEOUT: Duration = Duration::from_secs(3);

/// The page of the web app after a login.
pub const DONE_PATH: &str = "/cli/done";
/// The page of the web app after a failed login.
pub const ERROR_PATH: &str = "/cli/error";

/// The `reason` of a denial by a user who has no organization to connect yet: the sign-up
/// stopped before the company (backend `oauth.services.NO_ORGANIZATION`).
pub const NO_ORGANIZATION: &str = "no_organization";

const DONE_PAGE: &str = "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">\
<title>oscar</title></head><body><h1>You are logged in.</h1>\
<p>You can close this window and go back to the terminal.</p></body></html>";

const FAILED_PAGE: &str = "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">\
<title>oscar</title></head><body><h1>The login did not complete.</h1>\
<p>Go back to the terminal to see why.</p></body></html>";

/// Where the browser goes after the callback, and what a good callback holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Landing {
    /// The base URL of the web app, when it has the pages `/cli/done` and `/cli/error`; else
    /// the CLI answers with its own plain page.
    pub pages: Option<String>,
    /// The `state` of the login.
    pub state: String,
    /// The `issuer` of the server.
    pub issuer: String,
}

impl Landing {
    /// The reason code of `/cli/error` for a callback, or `None` for a good one:
    /// `state_mismatch`, `no_organization` (a denial by a user with no organization yet),
    /// `access_denied`, `invalid_request` or `server_error`.
    pub fn reason(&self, params: &[(String, String)]) -> Option<&'static str> {
        let get = |name: &str| {
            params
                .iter()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.as_str())
        };
        if get("state") != Some(self.state.as_str()) {
            return Some("state_mismatch");
        }
        if let Some(error) = get("error") {
            return Some(match error {
                "access_denied" if get("reason") == Some(NO_ORGANIZATION) => NO_ORGANIZATION,
                "access_denied" => "access_denied",
                "invalid_request"
                | "invalid_scope"
                | "unauthorized_client"
                | "unsupported_response_type" => "invalid_request",
                _ => "server_error",
            });
        }
        let issuer = self.issuer.trim_end_matches('/');
        if get("iss").map(|iss| iss.trim_end_matches('/')) != Some(issuer) || get("code").is_none()
        {
            return Some("invalid_request");
        }
        None
    }

    /// The answer to a callback.
    fn answer(&self, params: &[(String, String)]) -> String {
        let reason = self.reason(params);
        match (&self.pages, reason) {
            (Some(base), None) => redirect(&format!("{base}{DONE_PATH}")),
            (Some(base), Some(reason)) => redirect(&format!("{base}{ERROR_PATH}?reason={reason}")),
            (None, None) => page(DONE_PAGE),
            (None, Some(_)) => page(FAILED_PAGE),
        }
    }
}

/// The web app has the page `/cli/done`: a `GET` answers 2xx within a few seconds. Any other
/// answer, or none, means an older server, and the CLI answers the callback itself.
pub async fn has_pages(client: &Client) -> bool {
    client
        .request(HttpMethod::Get, DONE_PATH)
        .without_credential()
        .accept("text/html")
        .timeout(PROBE_TIMEOUT)
        .max_retries(0)
        .send()
        .await
        .is_ok()
}

/// The listener of one login.
#[derive(Debug)]
pub struct Loopback {
    listener: TcpListener,
    /// `http://127.0.0.1:<port>/callback`, with the real port.
    pub redirect_uri: String,
}

impl Loopback {
    /// Listen on `127.0.0.1` on a free port; on `[::1]` only when IPv4 loopback is not
    /// available.
    pub async fn bind() -> Result<Self> {
        let listener = match TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0))).await {
            Ok(listener) => listener,
            Err(_) => TcpListener::bind(SocketAddr::from((Ipv6Addr::LOCALHOST, 0)))
                .await
                .map_err(|error| {
                    CliError::failed(format!("Cannot listen on a loopback address: {error}"))
                })?,
        };
        let address = listener
            .local_addr()
            .map_err(|error| CliError::failed(format!("Cannot listen: {error}")))?;
        Ok(Self {
            redirect_uri: redirect_uri(address),
            listener,
        })
    }

    /// Wait for the first `GET /callback`, at most `timeout`, answer it as `landing` says, and
    /// give its query parameters. Any other request gets a 404 and the wait goes on.
    pub async fn wait(self, timeout: Duration, landing: Landing) -> Result<Vec<(String, String)>> {
        let accept = async {
            loop {
                let (stream, _) =
                    self.listener.accept().await.map_err(|error| {
                        CliError::failed(format!("The listener failed: {error}"))
                    })?;
                if let Some(params) = handle(stream, &landing).await {
                    return Ok(params);
                }
            }
        };
        tokio::time::timeout(timeout, accept)
            .await
            .unwrap_or_else(|_| {
                Err(CliError::auth(format!(
                    "No answer from the browser in {} minutes.",
                    timeout.as_secs() / 60
                )))
            })
    }
}

/// The redirect URI of a listening address.
fn redirect_uri(address: SocketAddr) -> String {
    match address {
        SocketAddr::V4(v4) => format!("http://{}:{}/callback", v4.ip(), v4.port()),
        SocketAddr::V6(v6) => format!("http://[{}]:{}/callback", v6.ip(), v6.port()),
    }
}

/// Serve one connection. `Some` with the query of a `GET /callback`.
async fn handle(mut stream: TcpStream, landing: &Landing) -> Option<Vec<(String, String)>> {
    let head = tokio::time::timeout(READ_TIMEOUT, read_head(&mut stream))
        .await
        .ok()
        .flatten();
    let params = head.as_deref().and_then(callback_params);
    let response = match &params {
        Some(params) => landing.answer(params),
        None => not_found(),
    };
    let _ = stream.write_all(response.as_bytes()).await;
    let _ = stream.shutdown().await;
    params
}

async fn read_head(stream: &mut TcpStream) -> Option<String> {
    let mut buffer = Vec::new();
    let mut chunk = [0_u8; 2048];
    while !buffer.windows(4).any(|window| window == b"\r\n\r\n") {
        let read = stream.read(&mut chunk).await.ok()?;
        if read == 0 || buffer.len() + read > HEAD_LIMIT {
            return None;
        }
        buffer.extend_from_slice(&chunk[..read]);
    }
    String::from_utf8(buffer).ok()
}

/// The query of `GET /callback?...` in a request head.
pub fn callback_params(head: &str) -> Option<Vec<(String, String)>> {
    let line = head.lines().next()?;
    let mut parts = line.split(' ');
    let (method, target, version) = (parts.next()?, parts.next()?, parts.next()?);
    if method != "GET" || !version.starts_with("HTTP/1.") || parts.next().is_some() {
        return None;
    }
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    if path != "/callback" {
        return None;
    }
    Some(
        url::form_urlencoded::parse(query.as_bytes())
            .map(|(name, value)| (name.into_owned(), value.into_owned()))
            .collect(),
    )
}

fn page(body: &str) -> String {
    format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\n\
         Referrer-Policy: no-referrer\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}

/// A redirect to a page of the web app. `no-referrer`: the URL of the callback holds the code.
fn redirect(location: &str) -> String {
    format!(
        "HTTP/1.1 302 Found\r\nLocation: {location}\r\nContent-Length: 0\r\n\
         Referrer-Policy: no-referrer\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n"
    )
}

fn not_found() -> String {
    "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n"
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn landing(pages: Option<&str>) -> Landing {
        Landing {
            pages: pages.map(str::to_owned),
            state: "s".into(),
            issuer: "https://x.test".into(),
        }
    }

    async fn get(uri: &str, target: &str) -> String {
        let address = uri
            .trim_start_matches("http://")
            .trim_end_matches("/callback")
            .to_owned();
        let mut stream = TcpStream::connect(address).await.unwrap();
        stream
            .write_all(format!("GET {target} HTTP/1.1\r\nHost: x\r\n\r\n").as_bytes())
            .await
            .unwrap();
        let mut answer = String::new();
        stream.read_to_string(&mut answer).await.unwrap();
        answer
    }

    #[test]
    fn parses_the_callback() {
        assert_eq!(
            callback_params("GET /callback?code=a%2Fb&state=s HTTP/1.1\r\nHost: x\r\n\r\n"),
            Some(vec![
                ("code".into(), "a/b".into()),
                ("state".into(), "s".into())
            ])
        );
        assert_eq!(
            callback_params("GET /callback HTTP/1.1\r\n\r\n"),
            Some(vec![])
        );
        for head in [
            "POST /callback?code=a HTTP/1.1\r\n\r\n",
            "GET /favicon.ico HTTP/1.1\r\n\r\n",
            "GET /callback?code=a SPDY/3\r\n\r\n",
            "GET /callback?code=a HTTP/1.1 extra\r\n\r\n",
            "GET\r\n\r\n",
            "",
        ] {
            assert_eq!(callback_params(head), None, "{head}");
        }
        assert_eq!(
            redirect_uri("[::1]:8000".parse().unwrap()),
            "http://[::1]:8000/callback"
        );
    }

    #[tokio::test]
    async fn takes_the_first_callback_only() {
        let loopback = Loopback::bind().await.unwrap();
        let uri = loopback.redirect_uri.clone();
        assert!(uri.starts_with("http://127.0.0.1:"), "{uri}");
        let waiting = tokio::spawn(loopback.wait(Duration::from_secs(10), landing(None)));
        let missing = get(&uri, "/favicon.ico").await;
        assert!(missing.starts_with("HTTP/1.1 404"));
        // A connection that sends nothing and closes is ignored too.
        drop(
            TcpStream::connect(
                uri.trim_start_matches("http://")
                    .trim_end_matches("/callback"),
            )
            .await
            .unwrap(),
        );
        let page = get(&uri, "/callback?code=c&state=s&iss=https%3A%2F%2Fx.test").await;
        assert!(page.starts_with("HTTP/1.1 200 OK"));
        assert!(page.contains("Referrer-Policy: no-referrer"));
        assert!(page.contains("Cache-Control: no-store"));
        assert!(page.contains("Content-Type: text/html; charset=utf-8"));
        assert!(page.contains("You are logged in."));
        let params = waiting.await.unwrap().unwrap();
        assert_eq!(params[0], ("code".into(), "c".into()));
        // The listener is closed.
        let address = uri
            .trim_start_matches("http://")
            .trim_end_matches("/callback")
            .to_owned();
        assert!(TcpStream::connect(address).await.is_err());
    }

    #[tokio::test]
    async fn an_error_gets_the_other_page() {
        let loopback = Loopback::bind().await.unwrap();
        let uri = loopback.redirect_uri.clone();
        let waiting = tokio::spawn(loopback.wait(Duration::from_secs(10), landing(None)));
        let page = get(&uri, "/callback?error=access_denied&state=s").await;
        assert!(page.contains("did not complete"));
        assert_eq!(waiting.await.unwrap().unwrap()[0].0, "error");
    }

    #[tokio::test]
    async fn gives_up_after_the_timeout() {
        let loopback = Loopback::bind().await.unwrap();
        let error = loopback
            .wait(Duration::from_millis(50), landing(None))
            .await
            .unwrap_err();
        assert_eq!(error.code, crate::error::EXIT_AUTH);
        assert_eq!(error.message, "No answer from the browser in 0 minutes.");
    }

    #[tokio::test]
    async fn refuses_a_head_that_is_too_long() {
        let loopback = Loopback::bind().await.unwrap();
        let uri = loopback.redirect_uri.clone();
        let waiting = tokio::spawn(loopback.wait(Duration::from_millis(500), landing(None)));
        let address = uri
            .trim_start_matches("http://")
            .trim_end_matches("/callback")
            .to_owned();
        let mut stream = TcpStream::connect(address).await.unwrap();
        let _ = stream.write_all(&vec![b'a'; HEAD_LIMIT + 10]).await;
        let mut answer = String::new();
        let _ = stream.read_to_string(&mut answer).await;
        assert!(answer.starts_with("HTTP/1.1 404"));
        assert!(waiting.await.unwrap().is_err());
    }

    #[test]
    fn the_reason_of_a_callback() {
        let pairs = |items: &[(&str, &str)]| -> Vec<(String, String)> {
            items
                .iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                .collect()
        };
        let landing = landing(Some("https://x.test"));
        let good = pairs(&[("code", "c"), ("state", "s"), ("iss", "https://x.test/")]);
        assert_eq!(landing.reason(&good), None);
        for (params, reason) in [
            (vec![("code", "c"), ("state", "t")], "state_mismatch"),
            (vec![("code", "c")], "state_mismatch"),
            (
                vec![("error", "access_denied"), ("state", "s")],
                "access_denied",
            ),
            (
                vec![
                    ("error", "access_denied"),
                    ("reason", "no_organization"),
                    ("state", "s"),
                ],
                "no_organization",
            ),
            (
                vec![("error", "invalid_scope"), ("state", "s")],
                "invalid_request",
            ),
            (
                vec![("error", "temporarily_unavailable"), ("state", "s")],
                "server_error",
            ),
            (
                vec![("code", "c"), ("state", "s"), ("iss", "https://evil.test")],
                "invalid_request",
            ),
            (
                vec![("state", "s"), ("iss", "https://x.test")],
                "invalid_request",
            ),
        ] {
            assert_eq!(landing.reason(&pairs(&params)), Some(reason), "{params:?}");
        }
        let answer = landing.answer(&good);
        assert!(answer.starts_with("HTTP/1.1 302 Found\r\nLocation: https://x.test/cli/done\r\n"));
        assert!(answer.contains("Referrer-Policy: no-referrer"));
        assert!(answer.contains("Cache-Control: no-store"));
        let answer = landing.answer(&pairs(&[("error", "access_denied"), ("state", "s")]));
        assert!(answer.contains("Location: https://x.test/cli/error?reason=access_denied\r\n"));
    }

    #[tokio::test]
    async fn redirects_to_the_web_app() {
        let loopback = Loopback::bind().await.unwrap();
        let uri = loopback.redirect_uri.clone();
        let waiting =
            tokio::spawn(loopback.wait(Duration::from_secs(10), landing(Some("https://x.test"))));
        let answer = get(&uri, "/callback?code=c&state=s&iss=https%3A%2F%2Fx.test").await;
        assert!(answer.starts_with("HTTP/1.1 302 Found"), "{answer}");
        assert!(answer.contains("Location: https://x.test/cli/done"));
        assert_eq!(waiting.await.unwrap().unwrap()[0].1, "c");
    }

    #[tokio::test]
    async fn probes_the_pages() {
        let client = Client::builder()
            .base_url("http://127.0.0.1:9")
            .env(|_| None)
            .build()
            .unwrap();
        assert!(!has_pages(&client).await);
    }
}
