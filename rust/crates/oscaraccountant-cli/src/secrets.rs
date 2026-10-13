//! The secrets of the profiles (`clients/spec/README.md`, section 2.2): the OS keychain, service
//! `oscaraccountant`, account `profile:<name>`, and `credentials.json` when no keychain service
//! answers.
//!
//! The keychain calls block (the Secret Service backend runs its own event loop), so they run on
//! the blocking threads of tokio.

use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::config::{Paths, create_dir, write_atomic};
use crate::env::Env;
use crate::error::{CliError, Result};

/// The keychain service of every entry.
pub const SERVICE: &str = "oscaraccountant";

/// The variable that chooses the store: `file` keeps the secrets in `credentials.json` and never
/// asks the keychain (tests, and machines where the keychain prompts). Unset: the keychain, with
/// the file as the fallback.
pub const STORE_VARIABLE: &str = "OSCAR_CREDENTIAL_STORE";

/// The secret of one profile.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Secret {
    /// The OAuth login.
    OAuth {
        /// The refresh token. The server rotates it at each refresh.
        refresh_token: String,
        /// The access token.
        access_token: String,
        /// When the access token expires, RFC 3339.
        expires_at: String,
    },
    /// An organization API key.
    ApiKey {
        /// The key, `osk_...`.
        api_key: String,
    },
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Never a secret value.
        match self {
            Self::OAuth { expires_at, .. } => write!(f, "OAuth(expires_at: {expires_at})"),
            Self::ApiKey { .. } => f.write_str("ApiKey(***)"),
        }
    }
}

/// The failure of a keychain call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeychainError {
    /// No keychain service answers: fall back to the file.
    Unavailable(String),
    /// Any other failure: shown to the user, with no fallback.
    Failed(String),
}

/// The OS keychain, or a fake one in a test.
pub trait Keychain: Send + Sync {
    /// The value of an account, `None` when there is none.
    fn get(&self, account: &str) -> std::result::Result<Option<String>, KeychainError>;
    /// Set the value of an account.
    fn set(&self, account: &str, value: &str) -> std::result::Result<(), KeychainError>;
    /// Delete an account. A missing account is not an error.
    fn delete(&self, account: &str) -> std::result::Result<(), KeychainError>;
}

/// The keychain of the OS, through the `keyring` crate.
#[derive(Debug, Clone, Copy)]
pub struct OsKeychain;

impl OsKeychain {
    fn entry(account: &str) -> std::result::Result<keyring::Entry, KeychainError> {
        keyring::Entry::new(SERVICE, account).map_err(keychain_error)
    }
}

/// "No backend or no service" (the platform failed, or it refused access) is `Unavailable`; a
/// bad value or an ambiguous entry is `Failed`.
fn keychain_error(error: keyring::Error) -> KeychainError {
    match error {
        keyring::Error::PlatformFailure(_) | keyring::Error::NoStorageAccess(_) => {
            KeychainError::Unavailable(error.to_string())
        }
        other => KeychainError::Failed(other.to_string()),
    }
}

impl Keychain for OsKeychain {
    fn get(&self, account: &str) -> std::result::Result<Option<String>, KeychainError> {
        match Self::entry(account)?.get_password() {
            Ok(value) => Ok(Some(value)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(error) => Err(keychain_error(error)),
        }
    }

    fn set(&self, account: &str, value: &str) -> std::result::Result<(), KeychainError> {
        Self::entry(account)?
            .set_password(value)
            .map_err(keychain_error)
    }

    fn delete(&self, account: &str) -> std::result::Result<(), KeychainError> {
        match Self::entry(account)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(error) => Err(keychain_error(error)),
        }
    }
}

/// Where a secret went.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Location {
    /// The OS keychain.
    Keychain,
    /// The fallback file.
    File(PathBuf),
}

impl fmt::Display for Location {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Keychain => f.write_str("the OS keychain"),
            Self::File(path) => write!(f, "{}", path.display()),
        }
    }
}

/// The content of `credentials.json`.
#[derive(Debug, Default, Serialize, Deserialize)]
struct CredentialsFile {
    version: u32,
    #[serde(default)]
    profiles: BTreeMap<String, Secret>,
    #[serde(flatten)]
    extra: Map<String, Value>,
}

/// The secrets of the CLI.
#[derive(Clone)]
pub struct Secrets {
    keychain: Option<Arc<dyn Keychain>>,
    file: PathBuf,
    notices: Arc<Mutex<Vec<String>>>,
}

impl fmt::Debug for Secrets {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Secrets")
            .field("keychain", &self.keychain.is_some())
            .field("file", &self.file)
            .finish_non_exhaustive()
    }
}

impl Secrets {
    /// The store of this run: the OS keychain and the fallback file, or the file only when
    /// `OSCAR_CREDENTIAL_STORE=file`.
    pub fn new(env: &Env, paths: &Paths) -> Result<Self> {
        let keychain: Option<Arc<dyn Keychain>> = match env.var(STORE_VARIABLE) {
            None | Some("auto") => Some(Arc::new(OsKeychain)),
            Some("file") => None,
            Some(other) => {
                return Err(CliError::usage(format!(
                    "{STORE_VARIABLE} must be `file` or `auto`, not `{other}`."
                )));
            }
        };
        Ok(Self::with_keychain(keychain, paths.credentials_file()))
    }

    /// A store with this keychain (none: the file only) and this fallback file.
    pub fn with_keychain(keychain: Option<Arc<dyn Keychain>>, file: PathBuf) -> Self {
        Self {
            keychain,
            file,
            notices: Arc::default(),
        }
    }

    /// The messages for the user that the store left, once each: "the credentials are in the
    /// file".
    pub fn take_notices(&self) -> Vec<String> {
        std::mem::take(&mut *self.notices.lock().unwrap_or_else(PoisonError::into_inner))
    }

    async fn keychain_call<T: Send + 'static>(
        &self,
        call: impl FnOnce(&dyn Keychain) -> std::result::Result<T, KeychainError> + Send + 'static,
    ) -> Option<std::result::Result<T, KeychainError>> {
        let keychain = self.keychain.clone()?;
        let joined = tokio::task::spawn_blocking(move || call(keychain.as_ref())).await;
        Some(joined.unwrap_or_else(|error| Err(KeychainError::Failed(error.to_string()))))
    }

    /// The secret of a profile: the file first, then the keychain. A copy in the file is always
    /// the newer one: only a set that the keychain could not take writes it, and a set that the
    /// keychain takes removes it.
    pub async fn get(&self, profile: &str) -> Result<Option<Secret>> {
        if let Some(secret) = read_file(&self.file)?.profiles.remove(profile) {
            return Ok(Some(secret));
        }
        let account = account(profile);
        match self
            .keychain_call(move |keychain| keychain.get(&account))
            .await
        {
            Some(Ok(Some(text))) => serde_json::from_str(&text).map(Some).map_err(|_| {
                CliError::failed(format!(
                    "The keychain entry of the profile `{profile}` is not valid. Run `oscar login` again."
                ))
            }),
            Some(Err(KeychainError::Failed(message))) => {
                Err(CliError::failed(format!("The keychain failed: {message}")))
            }
            Some(Ok(None) | Err(KeychainError::Unavailable(_))) | None => Ok(None),
        }
    }

    /// Store the secret of a profile: in the keychain, else in the file.
    pub async fn set(&self, profile: &str, secret: &Secret) -> Result<Location> {
        let text =
            serde_json::to_string(secret).map_err(|error| CliError::failed(error.to_string()))?;
        let account = account(profile);
        match self
            .keychain_call(move |keychain| keychain.set(&account, &text))
            .await
        {
            Some(Ok(())) => {
                // An older copy in the file must not outlive the new one.
                self.remove_from_file(profile)?;
                return Ok(Location::Keychain);
            }
            Some(Err(KeychainError::Failed(message))) => {
                return Err(CliError::failed(format!("The keychain failed: {message}")));
            }
            Some(Err(KeychainError::Unavailable(_))) | None => {}
        }
        let created = !self.file.exists();
        let mut data = read_file(&self.file)?;
        data.profiles.insert(profile.to_owned(), secret.clone());
        write_file(&self.file, &data)?;
        if created {
            let notice = if self.keychain.is_some() {
                format!(
                    "No keychain is available; the credentials are in {}.",
                    self.file.display()
                )
            } else {
                format!(
                    "The credentials are in {} (OSCAR_CREDENTIAL_STORE=file).",
                    self.file.display()
                )
            };
            self.notices
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(notice);
        }
        Ok(Location::File(self.file.clone()))
    }

    /// Delete the secret of a profile, from the keychain and from the file. When the keychain
    /// does not answer, the file copy still goes, and a notice says that a copy may remain in the
    /// keychain.
    pub async fn delete(&self, profile: &str) -> Result<()> {
        let account = account(profile);
        match self
            .keychain_call(move |keychain| keychain.delete(&account))
            .await
        {
            Some(Err(KeychainError::Failed(message))) => {
                return Err(CliError::failed(format!("The keychain failed: {message}")));
            }
            Some(Err(KeychainError::Unavailable(_))) => self
                .notices
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(format!(
                    "The OS keychain did not answer; a copy of the credential of the profile `{profile}` may remain in it."
                )),
            Some(Ok(())) | None => {}
        }
        self.remove_from_file(profile)
    }

    /// Where the secret of a profile is, for `oscar auth status`: the file first, as in
    /// [`get`](Self::get).
    pub async fn location(&self, profile: &str) -> Result<Option<Location>> {
        if read_file(&self.file)?.profiles.contains_key(profile) {
            return Ok(Some(Location::File(self.file.clone())));
        }
        let account = account(profile);
        match self
            .keychain_call(move |keychain| keychain.get(&account))
            .await
        {
            Some(Ok(Some(_))) => Ok(Some(Location::Keychain)),
            _ => Ok(None),
        }
    }

    /// Wait for the exclusive lock of a profile across processes, so only one of them refreshes
    /// its login at a time. The lock file is `<profile>.lock` next to `credentials.json`, never
    /// the credentials file itself, and holds nothing. The lock ends when the value is dropped.
    pub async fn lock(&self, profile: &str) -> Result<ProfileLock> {
        let dir = self.file.parent().unwrap_or_else(|| Path::new("."));
        create_dir(dir)?;
        let path = dir.join(format!(
            "{}.lock",
            utf8_percent_encode(profile, NON_ALPHANUMERIC)
        ));
        let mut options = fs::OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        let file = options
            .open(&path)
            .map_err(|error| CliError::io("lock", &path, &error))?;
        let locked = tokio::task::spawn_blocking(move || file.lock().map(|()| file))
            .await
            .map_err(|error| CliError::failed(error.to_string()))?;
        let file = locked.map_err(|error| CliError::io("lock", &path, &error))?;
        Ok(ProfileLock { _file: file })
    }

    fn remove_from_file(&self, profile: &str) -> Result<()> {
        if !self.file.exists() {
            return Ok(());
        }
        let mut data = read_file(&self.file)?;
        if data.profiles.remove(profile).is_some() {
            write_file(&self.file, &data)?;
        }
        Ok(())
    }
}

/// The lock of one profile across processes, from [`Secrets::lock`]. Closing the file on drop
/// ends it.
#[derive(Debug)]
pub struct ProfileLock {
    _file: fs::File,
}

fn account(profile: &str) -> String {
    format!("profile:{profile}")
}

fn read_file(path: &Path) -> Result<CredentialsFile> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(CredentialsFile {
                version: 1,
                ..CredentialsFile::default()
            });
        }
        Err(error) => return Err(CliError::io("read", path, &error)),
    };
    check_private(path)?;
    serde_json::from_slice(&bytes)
        .map_err(|error| CliError::failed(format!("{} is not valid: {error}", path.display())))
}

fn write_file(path: &Path, data: &CredentialsFile) -> Result<()> {
    let mut text =
        serde_json::to_string_pretty(data).map_err(|error| CliError::failed(error.to_string()))?;
    text.push('\n');
    write_atomic(path, text.as_bytes(), true)
}

/// Refuse a credentials file that the group or others may read.
#[cfg(unix)]
fn check_private(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mode = fs::metadata(path)
        .map_err(|error| CliError::io("read", path, &error))?
        .permissions()
        .mode();
    if mode & 0o077 != 0 {
        return Err(CliError::failed(format!(
            "{} may be read by other users (mode {:o}). Run `chmod 600 {}`.",
            path.display(),
            mode & 0o777,
            path.display()
        )));
    }
    Ok(())
}

#[cfg(not(unix))]
fn check_private(_path: &Path) -> Result<()> {
    Ok(())
}

/// Make a new file readable by its owner only. On Unix the mode of the file does it; on Windows
/// an access list for the current user only, with no inherited entry (`icacls`, by its absolute
/// path).
#[cfg(windows)]
pub fn restrict(path: &Path) -> Result<()> {
    let root = std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
    let icacls = Path::new(&root).join("System32").join("icacls.exe");
    let user = match (std::env::var("USERDOMAIN"), std::env::var("USERNAME")) {
        (Ok(domain), Ok(user)) if !user.is_empty() => format!("{domain}\\{user}"),
        // The "owner rights" SID.
        _ => "*S-1-3-4".to_owned(),
    };
    let status = std::process::Command::new(&icacls)
        .arg(path)
        .args(["/inheritance:r", "/grant:r"])
        .arg(format!("{user}:(F)"))
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map_err(|error| CliError::io("protect", path, &error))?;
    if status.success() {
        Ok(())
    } else {
        Err(CliError::failed(format!(
            "Cannot protect {}: icacls failed.",
            path.display()
        )))
    }
}

/// Make a new file readable by its owner only. On Unix the mode of the file does it.
#[cfg(not(windows))]
#[allow(clippy::unnecessary_wraps)]
pub fn restrict(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(test)]
pub mod tests {
    use std::collections::HashMap;

    use super::*;

    /// A keychain in memory that can be made to fail.
    #[derive(Default)]
    pub struct FakeKeychain {
        pub entries: Mutex<HashMap<String, String>>,
        pub error: Mutex<Option<KeychainError>>,
    }

    impl FakeKeychain {
        fn fail(&self) -> std::result::Result<(), KeychainError> {
            match self.error.lock().unwrap().clone() {
                Some(error) => Err(error),
                None => Ok(()),
            }
        }
    }

    impl Keychain for FakeKeychain {
        fn get(&self, account: &str) -> std::result::Result<Option<String>, KeychainError> {
            self.fail()?;
            Ok(self.entries.lock().unwrap().get(account).cloned())
        }

        fn set(&self, account: &str, value: &str) -> std::result::Result<(), KeychainError> {
            self.fail()?;
            self.entries
                .lock()
                .unwrap()
                .insert(account.to_owned(), value.to_owned());
            Ok(())
        }

        fn delete(&self, account: &str) -> std::result::Result<(), KeychainError> {
            self.fail()?;
            self.entries.lock().unwrap().remove(account);
            Ok(())
        }
    }

    fn oauth() -> Secret {
        Secret::OAuth {
            refresh_token: "r1".into(),
            access_token: "a1".into(),
            expires_at: "2026-09-27T10:00:00Z".into(),
        }
    }

    fn key() -> Secret {
        Secret::ApiKey {
            api_key: "osk_1".into(),
        }
    }

    fn store(dir: &Path) -> (Secrets, Arc<FakeKeychain>) {
        let keychain = Arc::new(FakeKeychain::default());
        let secrets = Secrets::with_keychain(
            Some(keychain.clone()),
            dir.join("data").join("credentials.json"),
        );
        (secrets, keychain)
    }

    #[test]
    fn the_formats_of_the_spec() {
        assert_eq!(
            serde_json::to_string(&oauth()).unwrap(),
            r#"{"refresh_token":"r1","access_token":"a1","expires_at":"2026-09-27T10:00:00Z"}"#
        );
        assert_eq!(
            serde_json::to_string(&key()).unwrap(),
            r#"{"api_key":"osk_1"}"#
        );
        assert_eq!(format!("{:?}", key()), "ApiKey(***)");
        assert_eq!(
            format!("{:?}", oauth()),
            "OAuth(expires_at: 2026-09-27T10:00:00Z)"
        );
        assert_eq!(Location::Keychain.to_string(), "the OS keychain");
        assert_eq!(Location::File("/x".into()).to_string(), "/x");
    }

    #[tokio::test]
    async fn keeps_secrets_in_the_keychain() {
        let dir = tempfile::tempdir().unwrap();
        let (secrets, keychain) = store(dir.path());
        assert_eq!(secrets.get("default").await.unwrap(), None);
        assert_eq!(
            secrets.set("default", &oauth()).await.unwrap(),
            Location::Keychain
        );
        assert!(
            keychain
                .entries
                .lock()
                .unwrap()
                .contains_key("profile:default")
        );
        assert_eq!(secrets.get("default").await.unwrap(), Some(oauth()));
        assert_eq!(
            secrets.location("default").await.unwrap(),
            Some(Location::Keychain)
        );
        secrets.delete("default").await.unwrap();
        assert_eq!(secrets.get("default").await.unwrap(), None);
        assert_eq!(secrets.location("default").await.unwrap(), None);
        assert!(!dir.path().join("data").exists());
        assert!(secrets.take_notices().is_empty());
        assert!(format!("{secrets:?}").contains("keychain: true"));
    }

    #[tokio::test]
    async fn falls_back_to_the_file_and_says_so_once() {
        let dir = tempfile::tempdir().unwrap();
        let (secrets, keychain) = store(dir.path());
        *keychain.error.lock().unwrap() = Some(KeychainError::Unavailable("no dbus".into()));
        let file = dir.path().join("data").join("credentials.json");
        assert_eq!(
            secrets.set("default", &oauth()).await.unwrap(),
            Location::File(file.clone())
        );
        secrets.set("other", &key()).await.unwrap();
        assert_eq!(
            secrets.take_notices(),
            vec![format!(
                "No keychain is available; the credentials are in {}.",
                file.display()
            )]
        );
        assert_eq!(secrets.get("default").await.unwrap(), Some(oauth()));
        assert_eq!(secrets.get("other").await.unwrap(), Some(key()));
        assert_eq!(
            secrets.location("other").await.unwrap(),
            Some(Location::File(file.clone()))
        );
        let text: Value = serde_json::from_slice(&fs::read(&file).unwrap()).unwrap();
        assert_eq!(text["version"], 1);
        assert_eq!(text["profiles"]["other"]["api_key"], "osk_1");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&file).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        secrets.delete("default").await.unwrap();
        assert_eq!(secrets.get("default").await.unwrap(), None);
        // The keychain answers again: the new secret goes there, and the file copy goes away.
        *keychain.error.lock().unwrap() = None;
        secrets.set("other", &key()).await.unwrap();
        let text: Value = serde_json::from_slice(&fs::read(&file).unwrap()).unwrap();
        assert_eq!(text["profiles"], serde_json::json!({}));
    }

    #[tokio::test]
    async fn the_file_copy_is_the_newer_one() {
        let dir = tempfile::tempdir().unwrap();
        let (secrets, keychain) = store(dir.path());
        let file = dir.path().join("data").join("credentials.json");
        let (a, b, c) = (
            key(),
            Secret::ApiKey {
                api_key: "osk_b".into(),
            },
            Secret::ApiKey {
                api_key: "osk_c".into(),
            },
        );
        secrets.set("default", &a).await.unwrap();
        *keychain.error.lock().unwrap() = Some(KeychainError::Unavailable("no dbus".into()));
        assert_eq!(
            secrets.set("default", &b).await.unwrap(),
            Location::File(file.clone())
        );
        // The keychain answers again, with the older secret A still in it.
        *keychain.error.lock().unwrap() = None;
        assert_eq!(secrets.get("default").await.unwrap(), Some(b));
        assert_eq!(
            secrets.location("default").await.unwrap(),
            Some(Location::File(file.clone()))
        );
        assert_eq!(
            secrets.set("default", &c).await.unwrap(),
            Location::Keychain
        );
        let text: Value = serde_json::from_slice(&fs::read(&file).unwrap()).unwrap();
        assert_eq!(text["profiles"], serde_json::json!({}));
        assert_eq!(secrets.get("default").await.unwrap(), Some(c));
    }

    #[tokio::test]
    async fn a_silent_keychain_at_logout_leaves_a_notice() {
        let dir = tempfile::tempdir().unwrap();
        let (secrets, keychain) = store(dir.path());
        secrets.set("default", &key()).await.unwrap();
        *keychain.error.lock().unwrap() = Some(KeychainError::Unavailable("no dbus".into()));
        secrets.set("other", &key()).await.unwrap();
        secrets.take_notices();
        secrets.delete("default").await.unwrap();
        secrets.delete("other").await.unwrap();
        assert_eq!(secrets.location("other").await.unwrap(), None);
        let notices = secrets.take_notices();
        assert_eq!(notices.len(), 2);
        assert_eq!(
            notices[0],
            "The OS keychain did not answer; a copy of the credential of the profile `default` may remain in it."
        );

        let file_only = Secrets::with_keychain(None, dir.path().join("credentials.json"));
        file_only.set("default", &key()).await.unwrap();
        file_only.take_notices();
        file_only.delete("default").await.unwrap();
        assert_eq!(file_only.get("default").await.unwrap(), None);
        assert!(file_only.take_notices().is_empty());
    }

    #[tokio::test]
    async fn locks_a_profile_across_handles() {
        let dir = tempfile::tempdir().unwrap();
        let (secrets, _) = store(dir.path());
        let first = secrets.lock("a/b").await.unwrap();
        assert!(dir.path().join("data").join("a%2Fb.lock").exists());
        let waiting = tokio::spawn({
            let secrets = secrets.clone();
            async move { secrets.lock("a/b").await.map(|_| ()) }
        });
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        assert!(
            !waiting.is_finished(),
            "the second lock waits for the first"
        );
        drop(first);
        waiting.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn another_keychain_error_is_shown() {
        let dir = tempfile::tempdir().unwrap();
        let (secrets, keychain) = store(dir.path());
        keychain
            .entries
            .lock()
            .unwrap()
            .insert("profile:bad".into(), "not json".into());
        assert!(
            secrets
                .get("bad")
                .await
                .unwrap_err()
                .message
                .contains("not valid")
        );
        *keychain.error.lock().unwrap() = Some(KeychainError::Failed("locked".into()));
        for error in [
            secrets.get("default").await.unwrap_err(),
            secrets.set("default", &key()).await.unwrap_err(),
            secrets.delete("default").await.unwrap_err(),
        ] {
            assert_eq!(error.message, "The keychain failed: locked");
        }
    }

    #[tokio::test]
    async fn the_file_only_store() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths {
            config_dir: dir.path().to_owned(),
            data_dir: dir.path().to_owned(),
        };
        let secrets = Secrets::new(&Env::with_vars(&[(STORE_VARIABLE, "file")]), &paths).unwrap();
        assert!(format!("{secrets:?}").contains("keychain: false"));
        secrets.set("default", &key()).await.unwrap();
        assert!(paths.credentials_file().exists());
        assert!(Secrets::new(&Env::with_vars(&[(STORE_VARIABLE, "auto")]), &paths).is_ok());
        let error =
            Secrets::new(&Env::with_vars(&[(STORE_VARIABLE, "vault")]), &paths).unwrap_err();
        assert_eq!(error.code, crate::error::EXIT_USAGE);
    }

    #[tokio::test]
    async fn a_forced_file_store_names_the_setting_not_a_missing_keychain() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("credentials.json");
        let secrets = Secrets::with_keychain(None, file.clone());
        secrets.set("default", &key()).await.unwrap();
        secrets.set("other", &key()).await.unwrap();
        assert_eq!(
            secrets.take_notices(),
            vec![format!(
                "The credentials are in {} (OSCAR_CREDENTIAL_STORE=file).",
                file.display()
            )]
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn refuses_a_file_others_may_read() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("credentials.json");
        fs::write(&file, r#"{"version": 1, "profiles": {}}"#).unwrap();
        fs::set_permissions(&file, fs::Permissions::from_mode(0o644)).unwrap();
        let secrets = Secrets::with_keychain(None, file.clone());
        let error = secrets.get("default").await.unwrap_err();
        assert!(error.message.contains("chmod 600"), "{}", error.message);
        fs::set_permissions(&file, fs::Permissions::from_mode(0o600)).unwrap();
        fs::write(&file, "{").unwrap();
        assert!(
            secrets
                .get("default")
                .await
                .unwrap_err()
                .message
                .contains("not valid")
        );
        fs::remove_file(&file).unwrap();
        fs::create_dir(&file).unwrap();
        assert!(
            secrets
                .get("default")
                .await
                .unwrap_err()
                .message
                .starts_with("Cannot read")
        );
    }

    #[test]
    fn maps_the_keyring_errors() {
        let unavailable = keychain_error(keyring::Error::NoStorageAccess("x".into()));
        assert!(matches!(unavailable, KeychainError::Unavailable(_)));
        let unavailable = keychain_error(keyring::Error::PlatformFailure("x".into()));
        assert!(matches!(unavailable, KeychainError::Unavailable(_)));
        let failed = keychain_error(keyring::Error::TooLong("x".into(), 1));
        assert!(matches!(failed, KeychainError::Failed(_)));
        assert!(restrict(Path::new("/nowhere")).is_ok() || cfg!(windows));
    }
}
