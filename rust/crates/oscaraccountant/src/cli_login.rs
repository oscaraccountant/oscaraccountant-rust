//! Step 3 of the credentials: the stored login of the `oscar` CLI (`clients/spec/README.md`,
//! section 1.3).
//!
//! The SDK finds `oscar` through `PATH` only, runs it by its absolute path with no shell, checks
//! `oscar --version` once per process, and asks `oscar auth token --json --base-url <base URL>`
//! for a token. It caches the token until 60 seconds before it expires, never logs it, and never
//! writes it anywhere.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{Duration, SystemTime};

use serde::Deserialize;

use crate::Error;
use crate::hooks::Hooks;
use crate::urls::normalize_base_url;

/// The timeout of `oscar auth token`: the CLI may refresh the token first.
const TOKEN_TIMEOUT: Duration = Duration::from_secs(30);
/// A token is used until this long before it expires.
const EXPIRY_MARGIN: Duration = Duration::from_secs(60);

/// The paths whose `--version` passed in this process.
static VERIFIED: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());

/// The answer of `oscar auth token --json`.
#[derive(Deserialize)]
struct TokenAnswer {
    token: String,
    expires_at: Option<String>,
    base_url: String,
}

struct Cached {
    token: String,
    /// `None`: valid for the life of the client (an API key).
    valid_until: Option<SystemTime>,
}

/// The CLI login of one client.
pub(crate) struct CliLogin {
    base_url: String,
    profile: Option<String>,
    hooks: Hooks,
    timeout: Duration,
    cache: tokio::sync::Mutex<Option<Cached>>,
}

impl CliLogin {
    pub(crate) fn new(base_url: String, profile: Option<String>, hooks: Hooks) -> Self {
        Self {
            base_url,
            profile,
            hooks,
            timeout: TOKEN_TIMEOUT,
            cache: tokio::sync::Mutex::new(None),
        }
    }

    /// The profile the SDK asks for, if any.
    #[cfg(test)]
    pub(crate) fn profile(&self) -> Option<&str> {
        self.profile.as_deref()
    }

    /// The cached token, or a new one from the CLI.
    pub(crate) async fn token(&self) -> Result<String, Error> {
        let mut cache = self.cache.lock().await;
        let now = (self.hooks.clock)();
        if let Some(cached) = cache.as_ref() {
            if cached.valid_until.is_none_or(|until| now < until) {
                return Ok(cached.token.clone());
            }
        }
        let fresh = self.ask().await?;
        let token = fresh.token.clone();
        *cache = Some(fresh);
        Ok(token)
    }

    /// Drop the cached token, after a 401 from the API.
    pub(crate) async fn invalidate(&self) {
        *self.cache.lock().await = None;
    }

    async fn ask(&self) -> Result<Cached, Error> {
        let program = find_oscar(
            self.hooks.var("PATH").map(OsString::from),
            self.hooks.var("PATHEXT").as_deref(),
            cfg!(windows),
        )
        .ok_or_else(|| {
            Error::authentication("The oscar CLI is not installed: no `oscar` on PATH.")
        })?;
        verify_version(&program, self.timeout).await?;
        let mut arguments = vec![
            "auth",
            "token",
            "--json",
            "--base-url",
            self.base_url.as_str(),
        ];
        if let Some(profile) = &self.profile {
            arguments.extend(["--profile", profile.as_str()]);
        }
        let output = run(&program, &arguments, self.timeout).await?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let line = stderr.lines().next().unwrap_or("").trim();
            let mut message = format!("`oscar auth token` failed: {line}");
            if output.status.code() == Some(3) {
                message.push_str(" Log in first: run `oscar login`.");
            }
            return Err(Error::authentication(message));
        }
        // The answer holds the token: no part of it goes into an error message.
        let answer: TokenAnswer = serde_json::from_slice(&output.stdout).map_err(|_| {
            Error::authentication(
                "`oscar auth token --json` gave an answer that is not the expected JSON.",
            )
        })?;
        let matches =
            normalize_base_url(&answer.base_url).is_ok_and(|value| value == self.base_url);
        if !matches {
            return Err(Error::authentication(format!(
                "The oscar CLI answered for {:?}, not for {:?}; the token is not used.",
                answer.base_url, self.base_url
            )));
        }
        if answer.token.is_empty() {
            return Err(Error::authentication(
                "`oscar auth token --json` gave an empty token.",
            ));
        }
        let valid_until = match answer.expires_at.as_deref() {
            None => None,
            Some(value) => Some(
                crate::rfc3339::parse(value)
                    .ok_or_else(|| {
                        Error::authentication(format!(
                            "`oscar auth token` gave an invalid expires_at {value:?}."
                        ))
                    })?
                    .checked_sub(EXPIRY_MARGIN)
                    .unwrap_or(SystemTime::UNIX_EPOCH),
            ),
        };
        Ok(Cached {
            token: answer.token,
            valid_until,
        })
    }

    #[cfg(all(test, unix))]
    pub(crate) fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }
}

/// The absolute path of `oscar` on `path`, never the current directory.
///
/// A relative entry of `PATH` resolves against the current directory, so it is skipped. On
/// Windows the candidates are `oscar.exe` and `oscar.cmd`, in the order of `PATHEXT`.
pub(crate) fn find_oscar(
    path: Option<OsString>,
    pathext: Option<&str>,
    windows: bool,
) -> Option<PathBuf> {
    let path = path?;
    std::env::split_paths(&path)
        .filter(|directory| directory.is_absolute())
        .flat_map(|directory| candidates(&directory, pathext, windows))
        .find(|candidate| is_executable(candidate, windows))
}

fn candidates(directory: &Path, pathext: Option<&str>, windows: bool) -> Vec<PathBuf> {
    if !windows {
        return vec![directory.join("oscar")];
    }
    pathext
        .unwrap_or(".COM;.EXE;.BAT;.CMD")
        .split(';')
        .map(|extension| extension.trim().to_ascii_lowercase())
        .filter(|extension| extension == ".exe" || extension == ".cmd")
        .map(|extension| directory.join(format!("oscar{extension}")))
        .collect()
}

fn is_executable(path: &Path, windows: bool) -> bool {
    let Ok(metadata) = std::fs::metadata(path) else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    if !windows {
        use std::os::unix::fs::PermissionsExt;
        return metadata.permissions().mode() & 0o111 != 0;
    }
    let _ = windows;
    true
}

/// Run `program` with `arguments`, no shell, no standard input, within `timeout`.
async fn run(
    program: &Path,
    arguments: &[&str],
    timeout: Duration,
) -> Result<std::process::Output, Error> {
    let mut command = tokio::process::Command::new(program);
    command
        .args(arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    match tokio::time::timeout(timeout, command.output()).await {
        Ok(Ok(output)) => Ok(output),
        Ok(Err(error)) => Err(Error::authentication(format!(
            "Could not run {}: {error}",
            program.display()
        ))),
        Err(_) => Err(Error::authentication(format!(
            "{} did not answer within {timeout:?}.",
            program.display()
        ))),
    }
}

/// Check once per process that `program --version` says `oscar-cli <semver>`.
async fn verify_version(program: &Path, timeout: Duration) -> Result<(), Error> {
    if verified().iter().any(|path| path == program) {
        return Ok(());
    }
    let output = run(program, &["--version"], timeout).await?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let first = stdout.lines().next().unwrap_or("");
    if !output.status.success() || !is_cli_version(first) {
        return Err(Error::authentication(format!(
            "`{}` is not the oscar CLI.",
            program.display()
        )));
    }
    verified().push(program.to_owned());
    Ok(())
}

fn verified() -> MutexGuard<'static, Vec<PathBuf>> {
    VERIFIED.lock().unwrap_or_else(PoisonError::into_inner)
}

/// `^oscar-cli (\d+\.\d+\.\d+)(\S*)$`
fn is_cli_version(line: &str) -> bool {
    let Some(version) = line.strip_prefix("oscar-cli ") else {
        return false;
    };
    let mut parts = version.splitn(3, '.');
    let (Some(major), Some(minor), Some(rest)) = (parts.next(), parts.next(), parts.next()) else {
        return false;
    };
    let patch_length = rest.bytes().take_while(u8::is_ascii_digit).count();
    let numeric = |part: &str| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit());
    numeric(major)
        && numeric(minor)
        && patch_length > 0
        && !rest[patch_length..].chars().any(char::is_whitespace)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_the_version_line() {
        assert!(is_cli_version("oscar-cli 0.1.0"));
        assert!(is_cli_version("oscar-cli 10.20.30-beta.1+abc"));
        for line in [
            "oscar 0.1.0",
            "oscar-cli 0.1",
            "oscar-cli a.1.0",
            "oscar-cli 0..0",
            "oscar-cli 0.1.x",
            "oscar-cli 0.1.0 extra",
            "oscar-cli ",
            "",
        ] {
            assert!(!is_cli_version(line), "{line}");
        }
    }

    #[test]
    fn windows_candidates_follow_pathext() {
        let directory = Path::new("/bin");
        assert_eq!(
            candidates(directory, Some(".CMD;.BAT;.EXE"), true),
            vec![directory.join("oscar.cmd"), directory.join("oscar.exe")]
        );
        assert_eq!(
            candidates(directory, None, true),
            vec![directory.join("oscar.exe"), directory.join("oscar.cmd")]
        );
        assert_eq!(
            candidates(directory, None, false),
            vec![directory.join("oscar")]
        );
    }

    #[test]
    fn finds_only_an_executable_on_an_absolute_path_entry() {
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        assert_eq!(find_oscar(None, None, false), None);
        std::fs::create_dir(first.path().join("oscar")).unwrap();
        let script = second.path().join("oscar");
        std::fs::write(&script, "#!/bin/sh\n").unwrap();
        let path =
            std::env::join_paths([Path::new("relative"), first.path(), second.path()]).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(find_oscar(Some(path.clone()), None, false), None);
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        assert_eq!(find_oscar(Some(path), None, false), Some(script));
        let windows = tempfile::tempdir().unwrap();
        std::fs::write(windows.path().join("oscar.cmd"), "").unwrap();
        let path = std::env::join_paths([windows.path()]).unwrap();
        assert_eq!(
            find_oscar(Some(path), None, true),
            Some(windows.path().join("oscar.cmd"))
        );
    }

    #[test]
    fn the_verified_set_survives_a_poisoned_lock() {
        let _ = std::thread::spawn(|| {
            let _guard = VERIFIED.lock().unwrap();
            panic!("poison the lock");
        })
        .join();
        verified().push(PathBuf::from("/poisoned/oscar"));
        assert!(
            verified()
                .iter()
                .any(|path| path == Path::new("/poisoned/oscar"))
        );
    }
}

/// A fake `oscar` for the tests: a shell script that plays the files of its directory.
#[cfg(all(test, unix))]
pub(crate) mod fake {
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;

    /// The tests that write and run a script hold this lock: a script written while another
    /// thread forks can fail to run with "Text file busy".
    pub(crate) static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    pub(crate) struct FakeCli {
        pub(crate) dir: tempfile::TempDir,
    }

    impl FakeCli {
        pub(crate) fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path();
            let script = format!(
                "#!/bin/sh\n\
                 dir='{}'\n\
                 echo \"$*\" >> \"$dir/calls\"\n\
                 if [ \"$1\" = \"--version\" ]; then cat \"$dir/version\"; exit 0; fi\n\
                 if [ -f \"$dir/slow\" ]; then sleep 5; fi\n\
                 if [ -f \"$dir/stderr\" ]; then cat \"$dir/stderr\" >&2; fi\n\
                 cat \"$dir/answer\"\n\
                 exit \"$(cat \"$dir/exit\")\"\n",
                path.display()
            );
            std::fs::write(path.join("oscar"), script).unwrap();
            std::fs::set_permissions(path.join("oscar"), std::fs::Permissions::from_mode(0o755))
                .unwrap();
            let cli = Self { dir };
            cli.write("version", "oscar-cli 0.1.0\n");
            cli.write("exit", "0");
            cli.answer(
                "tok-1",
                Some("2026-09-27T11:00:00Z"),
                "https://api.example.com",
            );
            cli
        }

        pub(crate) fn write(&self, name: &str, content: &str) {
            std::fs::write(self.dir.path().join(name), content).unwrap();
        }

        pub(crate) fn answer(&self, token: &str, expires_at: Option<&str>, base_url: &str) {
            let answer = serde_json::json!({
                "token": token, "expires_at": expires_at, "kind": "oauth", "base_url": base_url,
                "profile": "default", "later": "ignored",
            });
            self.write("answer", &answer.to_string());
        }

        pub(crate) fn path_var(&self) -> String {
            self.dir.path().display().to_string()
        }

        pub(crate) fn program(&self) -> std::path::PathBuf {
            self.dir.path().join("oscar")
        }

        pub(crate) fn calls(&self) -> Vec<String> {
            std::fs::read_to_string(self.dir.path().join("calls"))
                .unwrap_or_default()
                .lines()
                .map(str::to_owned)
                .collect()
        }

        pub(crate) fn path(&self) -> &Path {
            self.dir.path()
        }
    }
}

#[cfg(all(test, unix))]
mod process_tests {
    use std::sync::{Arc, Mutex};

    use super::fake::{FakeCli, SERIAL};
    use super::*;

    fn at(seconds: u64) -> SystemTime {
        // 27.09.2026 10:00:00 UTC plus `seconds`.
        SystemTime::UNIX_EPOCH + Duration::from_secs(1_790_503_200 + seconds)
    }

    fn login(cli: &FakeCli, profile: Option<&str>, clock: &Arc<Mutex<SystemTime>>) -> CliLogin {
        let path = cli.path_var();
        let clock = clock.clone();
        let hooks = Hooks {
            env: Arc::new(move |name| (name == "PATH").then(|| path.clone())),
            clock: Arc::new(move || *clock.lock().unwrap()),
            ..Hooks::default()
        };
        CliLogin::new(
            "https://api.example.com".into(),
            profile.map(str::to_owned),
            hooks,
        )
    }

    #[tokio::test]
    async fn asks_the_cli_and_caches_the_token() {
        let _serial = SERIAL.lock().await;
        let cli = FakeCli::new();
        let clock = Arc::new(Mutex::new(at(0)));
        let login = login(&cli, Some("work"), &clock);
        assert_eq!(login.token().await.unwrap(), "tok-1");
        cli.answer(
            "tok-2",
            Some("2026-09-27T12:00:00Z"),
            "https://api.example.com/",
        );
        assert_eq!(login.token().await.unwrap(), "tok-1", "cached");
        *clock.lock().unwrap() = at(3539);
        assert_eq!(
            login.token().await.unwrap(),
            "tok-1",
            "still more than 60 seconds left"
        );
        *clock.lock().unwrap() = at(3540);
        assert_eq!(
            login.token().await.unwrap(),
            "tok-2",
            "asked again 60 seconds before expiry"
        );
        assert_eq!(
            cli.calls(),
            vec![
                "--version",
                "auth token --json --base-url https://api.example.com --profile work",
                "auth token --json --base-url https://api.example.com --profile work",
            ],
            "the version is checked once per process"
        );
        login.invalidate().await;
        assert_eq!(login.token().await.unwrap(), "tok-2");
        assert_eq!(cli.calls().len(), 4);
    }

    #[tokio::test]
    async fn an_api_key_is_cached_for_the_life_of_the_client() {
        let _serial = SERIAL.lock().await;
        let cli = FakeCli::new();
        cli.answer("osk_key", None, "https://api.example.com");
        let clock = Arc::new(Mutex::new(at(0)));
        let login = login(&cli, None, &clock);
        assert_eq!(login.token().await.unwrap(), "osk_key");
        *clock.lock().unwrap() = at(10_000_000);
        assert_eq!(login.token().await.unwrap(), "osk_key");
        assert_eq!(
            cli.calls(),
            vec![
                "--version",
                "auth token --json --base-url https://api.example.com"
            ]
        );
    }

    #[tokio::test]
    async fn refuses_a_token_of_another_base_url() {
        let _serial = SERIAL.lock().await;
        let cli = FakeCli::new();
        cli.answer("tok-secret", None, "https://other.example.com");
        let error = login(&cli, None, &Arc::new(Mutex::new(at(0))))
            .token()
            .await
            .unwrap_err();
        assert!(matches!(error, Error::Authentication(_)));
        assert!(error.to_string().contains("other.example.com"));
        assert!(!error.to_string().contains("tok-secret"));
    }

    #[tokio::test]
    async fn a_failing_cli_gives_its_first_error_line() {
        let _serial = SERIAL.lock().await;
        let cli = FakeCli::new();
        cli.write("exit", "3");
        cli.write(
            "stderr",
            "no profile for https://api.example.com\nmore detail\n",
        );
        cli.write("answer", "");
        let error = login(&cli, None, &Arc::new(Mutex::new(at(0))))
            .token()
            .await
            .unwrap_err();
        assert_eq!(
            error.to_string(),
            "`oscar auth token` failed: no profile for https://api.example.com Log in first: run `oscar login`."
        );
        cli.write("exit", "1");
        let error = login(&cli, None, &Arc::new(Mutex::new(at(0))))
            .token()
            .await
            .unwrap_err();
        assert!(!error.to_string().contains("Log in first"));
    }

    #[tokio::test]
    async fn refuses_an_answer_that_is_not_the_expected_json() {
        let _serial = SERIAL.lock().await;
        let cli = FakeCli::new();
        let clock = Arc::new(Mutex::new(at(0)));
        cli.write("answer", "token=tok-secret");
        let error = login(&cli, None, &clock).token().await.unwrap_err();
        assert!(error.to_string().contains("not the expected JSON"));
        assert!(!error.to_string().contains("tok-secret"));
        cli.answer("", None, "https://api.example.com");
        assert!(
            login(&cli, None, &clock)
                .token()
                .await
                .unwrap_err()
                .to_string()
                .contains("empty token")
        );
        cli.answer("tok-secret", Some("tomorrow"), "https://api.example.com");
        let error = login(&cli, None, &clock).token().await.unwrap_err();
        assert!(error.to_string().contains("invalid expires_at"));
        assert!(!error.to_string().contains("tok-secret"));
    }

    #[tokio::test]
    async fn refuses_a_program_that_is_not_the_cli() {
        let _serial = SERIAL.lock().await;
        let cli = FakeCli::new();
        cli.write("version", "something else 1.0\n");
        let error = login(&cli, None, &Arc::new(Mutex::new(at(0))))
            .token()
            .await
            .unwrap_err();
        assert!(
            error.to_string().ends_with("is not the oscar CLI."),
            "{error}"
        );
        assert_eq!(cli.calls(), vec!["--version"]);
    }

    #[tokio::test]
    async fn no_cli_on_the_path() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().display().to_string();
        let hooks = Hooks {
            env: Arc::new(move |name| (name == "PATH").then(|| path.clone())),
            ..Hooks::default()
        };
        let login = CliLogin::new("https://api.example.com".into(), None, hooks);
        let error = login.token().await.unwrap_err();
        assert_eq!(
            error.to_string(),
            "The oscar CLI is not installed: no `oscar` on PATH."
        );
    }

    #[tokio::test]
    async fn a_cli_that_does_not_answer_in_time() {
        let _serial = SERIAL.lock().await;
        let cli = FakeCli::new();
        let clock = Arc::new(Mutex::new(at(0)));
        // Check the version first, with the normal timeout.
        verify_version(&cli.program(), TOKEN_TIMEOUT).await.unwrap();
        cli.write("slow", "");
        let login = login(&cli, None, &clock).with_timeout(Duration::from_millis(200));
        let error = login.token().await.unwrap_err();
        assert!(
            error.to_string().ends_with("did not answer within 200ms."),
            "{error}"
        );
    }

    #[tokio::test]
    async fn a_program_that_cannot_run() {
        let _serial = SERIAL.lock().await;
        let cli = FakeCli::new();
        std::fs::write(cli.program(), [0u8, 1, 2, 3]).unwrap();
        let error = login(&cli, None, &Arc::new(Mutex::new(at(0))))
            .token()
            .await
            .unwrap_err();
        // Linux refuses to start the file; macOS starts it through a shell, which then fails the
        // version check. Either way the program is refused.
        let message = error.to_string();
        assert!(
            message.starts_with("Could not run ") || message.ends_with("is not the oscar CLI."),
            "{error}"
        );
        assert!(cli.path().exists());
    }
}
