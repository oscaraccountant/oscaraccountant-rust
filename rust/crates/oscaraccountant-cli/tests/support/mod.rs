//! The harness of the CLI suite: the mock server of `clients/mock-server`, started once per test,
//! and the real `oscar` binary, run as a process.
//!
//! The binary is the one Cargo built, or `OSCAR_CLI_BIN` (a cross-built or a musl binary). The
//! mock server runs with `uv run` from `OSCAR_MOCK_SERVER_DIR`, else `clients/mock-server`.

#![allow(dead_code)]

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use oscaraccountant::{TlsRoots, reqwest, reqwest_client};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};

/// The prefix of the ready line of the mock server.
const READY: &str = "OSCAR_MOCK_SERVER_READY ";

/// The directory of the mock server.
pub fn mock_dir() -> PathBuf {
    match std::env::var_os("OSCAR_MOCK_SERVER_DIR") {
        Some(dir) => PathBuf::from(dir),
        None => Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../mock-server"),
    }
}

/// The directory of the conformance cases.
pub fn cases_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../spec/conformance/cases")
}

/// The Python of the mock server's environment, for the test browser.
pub fn python() -> PathBuf {
    let venv = mock_dir().join(".venv");
    if cfg!(windows) {
        venv.join("Scripts").join("python.exe")
    } else {
        venv.join("bin").join("python")
    }
}

/// A running mock server.
pub struct Mock {
    child: Child,
    /// The base URL and OAuth issuer.
    pub url: String,
    /// The API key it accepts.
    pub api_key: String,
    /// The one company.
    pub company_id: String,
    /// The one organization.
    pub organization_id: String,
    http: reqwest::Client,
}

impl Mock {
    /// Start a mock server with these settings.
    pub async fn start(config: Value) -> Self {
        let mut child = Command::new("uv")
            .args(["run", "--quiet", "--locked", "--project"])
            .arg(mock_dir())
            .args(["oscar-mock-server", "--port", "0", "--config"])
            .arg(config.to_string())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true)
            .spawn()
            .expect("uv runs the mock server (clients/mock-server)");
        let stdout = child
            .stdout
            .take()
            .expect("the mock server's standard output");
        let mut lines = BufReader::new(stdout).lines();
        let ready = tokio::time::timeout(Duration::from_secs(120), async {
            while let Some(line) = lines.next_line().await.expect("a line") {
                if let Some(json) = line.strip_prefix(READY) {
                    return serde_json::from_str::<Value>(json).expect("the ready JSON");
                }
            }
            panic!("the mock server ended before it was ready");
        })
        .await
        .expect("the mock server is ready within 2 minutes");
        // Keep reading, so the server never blocks on a full pipe.
        tokio::spawn(async move { while let Ok(Some(_)) = lines.next_line().await {} });
        let text = |key: &str| ready[key].as_str().expect(key).to_owned();
        Self {
            url: text("url"),
            api_key: text("api_key"),
            company_id: text("company_id"),
            organization_id: text("organization_id"),
            child,
            http: reqwest_client(TlsRoots::Bundled).expect("an HTTP client"),
        }
    }

    /// A call of the control API.
    pub async fn control(&self, method: &str, path: &str, body: Option<Value>) -> Value {
        let method = reqwest::Method::from_bytes(method.as_bytes()).expect("a method");
        let mut request = self.http.request(method, format!("{}{path}", self.url));
        if let Some(body) = body {
            request = request
                .header("content-type", "application/json")
                .body(body.to_string());
        }
        let response = request.send().await.expect("the control API answers");
        let text = response.text().await.expect("a body");
        serde_json::from_str(&text).unwrap_or(Value::Null)
    }

    /// The request log.
    pub async fn requests(&self) -> Vec<Value> {
        self.control("GET", "/_mock/requests", None).await["requests"]
            .as_array()
            .cloned()
            .unwrap_or_default()
    }

    /// The requests of the log to `path`.
    pub async fn requests_to(&self, path: &str) -> Vec<Value> {
        self.requests()
            .await
            .into_iter()
            .filter(|request| request["path"] == path)
            .collect()
    }

    /// The requests to `path` once there are at least `count`, or what came within 10 seconds.
    /// The test browser follows the redirect of the loopback on its own, so the page request
    /// can arrive after the CLI has ended.
    pub async fn wait_for_requests(&self, path: &str, count: usize) -> Vec<Value> {
        let started = std::time::Instant::now();
        loop {
            let requests = self.requests_to(path).await;
            if requests.len() >= count || started.elapsed() > std::time::Duration::from_secs(10) {
                return requests;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    }
}

impl Drop for Mock {
    fn drop(&mut self) {
        // `uv run` does not always pass a kill on to Python, so ask the server to stop first.
        let address = self.url.trim_start_matches("http://").to_owned();
        if let Ok(mut stream) = std::net::TcpStream::connect(address) {
            let _ = stream.write_all(
                b"POST /_mock/shutdown HTTP/1.1\r\nHost: mock\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            );
            let _ = std::io::Read::read(&mut stream, &mut [0_u8; 256]);
        }
        let _ = self.child.start_kill();
    }
}

/// What one run of `oscar` gave.
#[derive(Debug)]
pub struct Run {
    /// The exit code.
    pub code: i32,
    /// Standard output.
    pub stdout: String,
    /// Standard error.
    pub stderr: String,
}

impl Run {
    /// Standard output as JSON.
    pub fn json(&self) -> Value {
        serde_json::from_str(&self.stdout).unwrap_or_else(|error| {
            panic!(
                "{error}: stdout {:?}, stderr {:?}",
                self.stdout, self.stderr
            )
        })
    }

    /// Assert the exit code, with the output in the message.
    pub fn expect(self, code: i32) -> Self {
        assert_eq!(
            self.code, code,
            "exit code; stdout {:?}; stderr {:?}",
            self.stdout, self.stderr
        );
        self
    }
}

/// A user of the CLI: a config directory of its own, and the file store.
pub struct User {
    /// The config directory (`OSCAR_CONFIG_DIR`).
    pub dir: tempfile::TempDir,
    /// Extra variables.
    pub vars: Vec<(String, String)>,
}

impl User {
    /// A user with an empty config directory.
    pub fn new() -> Self {
        Self {
            dir: tempfile::tempdir().expect("a temporary directory"),
            vars: Vec::new(),
        }
    }

    /// Set a variable for every later run.
    pub fn set(&mut self, name: &str, value: &str) -> &mut Self {
        self.vars.retain(|(key, _)| key != name);
        self.vars.push((name.to_owned(), value.to_owned()));
        self
    }

    /// The binary under test.
    pub fn binary() -> PathBuf {
        match std::env::var_os("OSCAR_CLI_BIN") {
            Some(path) => PathBuf::from(path),
            None => PathBuf::from(env!("CARGO_BIN_EXE_oscar")),
        }
    }

    /// Run `oscar` with these arguments and this standard input.
    pub async fn run_with(&self, args: &[&str], stdin: Option<&str>) -> Run {
        let mut command = Command::new(Self::binary());
        command
            .args(args)
            .current_dir(self.dir.path())
            .stdin(if stdin.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        for (name, _) in std::env::vars_os() {
            let text = name.to_string_lossy();
            if text.starts_with("OSCAR_")
                || ["BROWSER", "SSH_CONNECTION", "NO_COLOR"].contains(&text.as_ref())
            {
                command.env_remove(&name);
            }
        }
        command
            .env("OSCAR_CONFIG_DIR", self.dir.path())
            .env("OSCAR_CREDENTIAL_STORE", "file")
            .env("NO_COLOR", "1")
            // A display, so the loopback login is the default on Linux.
            .env("DISPLAY", ":99")
            // Never the real browser of the person who runs the tests: `true` opens nothing.
            // A test that needs a browser sets its own (`test_browser`).
            .env("BROWSER", "true");
        for (name, value) in &self.vars {
            command.env(name, value);
        }
        let mut child = command.spawn().expect("the oscar binary runs");
        if let Some(text) = stdin {
            let mut pipe = child.stdin.take().expect("a pipe");
            pipe.write_all(text.as_bytes()).await.expect("stdin");
            drop(pipe);
        }
        let output = tokio::time::timeout(Duration::from_secs(120), child.wait_with_output())
            .await
            .expect("oscar ends within 2 minutes")
            .expect("oscar ran");
        Run {
            code: output.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        }
    }

    /// Run `oscar` with these arguments.
    pub async fn run(&self, args: &[&str]) -> Run {
        self.run_with(args, None).await
    }

    /// `config.json`.
    pub fn config(&self) -> Value {
        let text = std::fs::read_to_string(self.dir.path().join("config.json")).unwrap_or_default();
        serde_json::from_str(&text).unwrap_or(Value::Null)
    }

    /// `credentials.json`.
    pub fn credentials(&self) -> Value {
        let text =
            std::fs::read_to_string(self.dir.path().join("credentials.json")).unwrap_or_default();
        serde_json::from_str(&text).unwrap_or(Value::Null)
    }

    /// Log in to the mock with its API key, as the profile `default`.
    pub async fn login_with_key(&self, mock: &Mock) -> Run {
        self.run_with(
            &["login", "--api-key", "--base-url", &mock.url],
            Some(&format!("{}\n", mock.api_key)),
        )
        .await
    }

    /// Log in to the mock with the device grant, approved on the first poll.
    pub async fn login_with_device(&self, mock: &Mock) -> Run {
        mock.control(
            "POST",
            "/_mock/config",
            Some(json!({"device_interval": 0, "device_auto_after_polls": 1})),
        )
        .await;
        self.run(&["login", "--device", "--base-url", &mock.url])
            .await
    }
}

/// A browser for the loopback login: a Python script that follows the URL with a `GET`.
pub fn test_browser(dir: &Path) -> String {
    let script = dir.join("browser.py");
    std::fs::write(
        &script,
        "import sys, urllib.request\nurllib.request.urlopen(sys.argv[1], timeout=60).read()\n",
    )
    .expect("the browser script");
    format!("{} {}", python().display(), script.display())
}
