//! `config.json`: the profiles, with no secret (`clients/spec/README.md`, section 2.1).

use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::env::Env;
use crate::error::{CliError, Result};

/// The name of the directory of the CLI under the config and data directories of the OS.
const APP_DIR: &str = "oscaraccountant";

/// Where the CLI keeps its files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    /// The directory of `config.json`.
    pub config_dir: PathBuf,
    /// The directory of the fallback `credentials.json`.
    pub data_dir: PathBuf,
}

impl Paths {
    /// `OSCAR_CONFIG_DIR` for both, else the config and the local data directory of the OS.
    pub fn resolve(env: &Env) -> Result<Self> {
        if let Some(dir) = env.var("OSCAR_CONFIG_DIR") {
            let dir = PathBuf::from(dir);
            return Ok(Self {
                config_dir: dir.clone(),
                data_dir: dir,
            });
        }
        let missing = || CliError::failed("Cannot find the home directory. Set OSCAR_CONFIG_DIR.");
        Ok(Self {
            config_dir: dirs::config_dir().ok_or_else(missing)?.join(APP_DIR),
            data_dir: dirs::data_local_dir().ok_or_else(missing)?.join(APP_DIR),
        })
    }

    /// The path of `config.json`.
    pub fn config_file(&self) -> PathBuf {
        self.config_dir.join("config.json")
    }

    /// The path of the fallback `credentials.json`.
    pub fn credentials_file(&self) -> PathBuf {
        self.data_dir.join("credentials.json")
    }
}

/// How a profile logged in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthKind {
    /// The OAuth login: a refresh token and an access token.
    Oauth,
    /// A company API key.
    ApiKey,
}

impl AuthKind {
    /// The name in `config.json` and in `oscar auth token --json`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Oauth => "oauth",
            Self::ApiKey => "api_key",
        }
    }
}

/// One login.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(from = "StoredProfile")]
pub struct Profile {
    /// The normalized base URL.
    pub base_url: String,
    /// How it logged in.
    pub auth: AuthKind,
    /// The default company of the commands.
    #[serde(default)]
    pub company: Option<String>,
    /// The companies of the credential, from the last `GET /api/me/`: one for a company API key,
    /// the companies chosen on the consent page for an OAuth login. The names that
    /// `--company <name>` resolves against. Read again when a name does not resolve.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub companies: Option<Vec<Company>>,
    /// Keys a newer CLI wrote.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// A profile as `config.json` stores it. A file of a CLI before 0.2.0 also has `organizations`
/// (or one `organization`) and an `organization_id` in each cached company: the server has no
/// organizations any more, so the CLI drops them when it reads the file, and the next save writes
/// the profile without them.
#[derive(Deserialize)]
struct StoredProfile {
    base_url: String,
    auth: AuthKind,
    #[serde(default)]
    company: Option<String>,
    #[serde(default)]
    companies: Option<Vec<Company>>,
    #[serde(flatten)]
    extra: Map<String, Value>,
}

/// The keys of a profile that a CLI before 0.2.0 wrote, for the organizations the server had.
const ORGANIZATION_KEYS: [&str; 2] = ["organizations", "organization"];

impl From<StoredProfile> for Profile {
    fn from(stored: StoredProfile) -> Self {
        let mut extra = stored.extra;
        for key in ORGANIZATION_KEYS {
            extra.remove(key);
        }
        Self {
            base_url: stored.base_url,
            auth: stored.auth,
            company: stored.company,
            companies: stored.companies,
            extra,
        }
    }
}

impl Profile {
    /// The companies of the cache, as `{id, name}`; empty when the profile has no cache.
    pub fn company_summaries(&self) -> Vec<Value> {
        self.companies
            .iter()
            .flatten()
            .map(|company| serde_json::json!({"id": company.id, "name": company.name}))
            .collect()
    }

    /// The names of the companies of the cache, with no empty one.
    pub fn company_names(&self) -> Vec<&str> {
        self.companies
            .iter()
            .flatten()
            .map(|company| company.name.as_str())
            .filter(|name| !name.is_empty())
            .collect()
    }
}

/// A company of the cache of a profile: the fields of `MeCompany` the CLI needs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Company {
    /// Its sqid.
    pub id: String,
    /// Its name.
    #[serde(default)]
    pub name: String,
    /// Its registry code.
    #[serde(default)]
    pub registry_code: String,
    /// `provisioning`, `ready` or `failed`.
    #[serde(default)]
    pub state: String,
}

/// The whole of `config.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Config {
    /// The format, 1.
    pub version: u32,
    /// The profile of a command with no `--profile`.
    #[serde(default)]
    pub default_profile: Option<String>,
    /// The profiles by name.
    #[serde(default)]
    pub profiles: BTreeMap<String, Profile>,
    /// Keys a newer CLI wrote.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            version: 1,
            default_profile: None,
            profiles: BTreeMap::new(),
            extra: Map::new(),
        }
    }
}

impl Config {
    /// Read `config.json`. A missing file is an empty configuration.
    pub fn load(paths: &Paths) -> Result<Self> {
        let path = paths.config_file();
        match fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(|error| {
                CliError::failed(format!("{} is not valid: {error}", path.display()))
            }),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(CliError::io("read", &path, &error)),
        }
    }

    /// Write `config.json` atomically.
    pub fn save(&self, paths: &Paths) -> Result<()> {
        let mut text = serde_json::to_string_pretty(self)
            .map_err(|error| CliError::failed(error.to_string()))?;
        text.push('\n');
        write_atomic(&paths.config_file(), text.as_bytes(), false)
    }

    /// The name of the profile a command uses when none is given: the default profile, else
    /// the only profile.
    pub fn default_name(&self) -> Option<&str> {
        match &self.default_profile {
            Some(name) if self.profiles.contains_key(name) => Some(name),
            _ if self.profiles.len() == 1 => self.profiles.keys().next().map(String::as_str),
            _ => None,
        }
    }

    /// The profile for a base URL, when the caller gave no profile (spec section 1.3): the
    /// default profile when it has that base URL, else the only profile with that base URL.
    pub fn name_for_base_url(&self, base_url: &str) -> Option<&str> {
        if let Some(name) = self.default_name()
            && self.profiles[name].base_url == base_url
        {
            return Some(name);
        }
        let mut matching = self
            .profiles
            .iter()
            .filter(|(_, profile)| profile.base_url == base_url);
        match (matching.next(), matching.next()) {
            (Some((name, _)), None) => Some(name),
            _ => None,
        }
    }

    /// Forget a profile. When it was the default, the only other profile becomes the default.
    pub fn remove(&mut self, name: &str) {
        self.profiles.remove(name);
        if self.default_profile.as_deref() == Some(name) {
            self.default_profile = if self.profiles.len() == 1 {
                self.profiles.keys().next().cloned()
            } else {
                None
            };
        }
    }
}

/// Make a directory of the CLI, owner-only on Unix.
pub fn create_dir(dir: &Path) -> Result<()> {
    if dir.is_dir() {
        return Ok(());
    }
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    builder
        .create(dir)
        .map_err(|error| CliError::io("create", dir, &error))
}

/// Write a file through a temporary file in the same directory and a rename, so a reader never
/// sees half of it. `private` makes it readable by the owner only (mode 0600 on Unix, an
/// owner-only access list on Windows).
pub fn write_atomic(path: &Path, bytes: &[u8], private: bool) -> Result<()> {
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    create_dir(dir)?;
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("file");
    // A new file with a random name: `create_new` never follows a link that sits at the path,
    // and a name that exists already is tried again with another one.
    let mut attempts = 0;
    let (temporary, mut file) = loop {
        let mut suffix = [0_u8; 8];
        getrandom::fill(&mut suffix)
            .map_err(|error| CliError::failed(format!("No random source: {error}")))?;
        let suffix: String = suffix.iter().map(|byte| format!("{byte:02x}")).collect();
        let temporary = dir.join(format!(".{name}.{suffix}.tmp"));
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, if private { 0o600 } else { 0o644 });
        match options.open(&temporary) {
            Ok(file) => break (temporary, file),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists && attempts < 16 => {
                attempts += 1;
            }
            Err(error) => return Err(CliError::io("write", &temporary, &error)),
        }
    };
    let result = (|| {
        if private {
            crate::secrets::restrict(&temporary)?;
        }
        file.write_all(bytes)
            .and_then(|()| file.sync_all())
            .map_err(|error| CliError::io("write", &temporary, &error))?;
        drop(file);
        fs::rename(&temporary, path).map_err(|error| CliError::io("write", path, &error))
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn paths(dir: &Path) -> Paths {
        Paths {
            config_dir: dir.join("config"),
            data_dir: dir.join("data"),
        }
    }

    fn profile(base_url: &str) -> Profile {
        Profile {
            base_url: base_url.into(),
            auth: AuthKind::Oauth,
            company: None,
            companies: None,
            extra: Map::new(),
        }
    }

    #[test]
    fn resolves_the_directories() {
        let env = Env::with_vars(&[("OSCAR_CONFIG_DIR", "/tmp/oscar-test")]);
        let paths = Paths::resolve(&env).unwrap();
        assert_eq!(
            paths.config_file(),
            Path::new("/tmp/oscar-test/config.json")
        );
        assert_eq!(
            paths.credentials_file(),
            Path::new("/tmp/oscar-test/credentials.json")
        );
        let paths = Paths::resolve(&Env::with_vars(&[])).unwrap();
        assert!(paths.config_dir.ends_with("oscaraccountant"));
        assert!(paths.data_dir.ends_with("oscaraccountant"));
    }

    #[test]
    fn a_missing_file_is_empty_and_a_saved_one_reads_back() {
        let dir = tempfile::tempdir().unwrap();
        let paths = paths(dir.path());
        assert_eq!(Config::load(&paths).unwrap(), Config::default());
        let mut config = Config {
            default_profile: Some("default".into()),
            ..Config::default()
        };
        config
            .profiles
            .insert("default".into(), profile("https://a.test"));
        config.save(&paths).unwrap();
        assert_eq!(Config::load(&paths).unwrap(), config);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&paths.config_dir)
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o700);
        }
    }

    #[test]
    fn keeps_unknown_keys() {
        let dir = tempfile::tempdir().unwrap();
        let paths = paths(dir.path());
        fs::create_dir_all(&paths.config_dir).unwrap();
        let text = json!({
            "version": 1,
            "future": {"a": 1},
            "profiles": {"p": {
                "base_url": "https://a.test",
                "auth": "api_key",
                "company": "c1",
                "colour": "blue",
            }},
        });
        fs::write(paths.config_file(), text.to_string()).unwrap();
        let config = Config::load(&paths).unwrap();
        assert_eq!(config.profiles["p"].auth, AuthKind::ApiKey);
        config.save(&paths).unwrap();
        let saved: Value = serde_json::from_slice(&fs::read(paths.config_file()).unwrap()).unwrap();
        assert_eq!(saved["future"], json!({"a": 1}));
        assert_eq!(saved["profiles"]["p"]["colour"], "blue");
    }

    #[test]
    fn drops_the_organizations_of_an_older_file() {
        let dir = tempfile::tempdir().unwrap();
        let paths = paths(dir.path());
        fs::create_dir_all(&paths.config_dir).unwrap();
        // The file of a CLI before 0.2.0: the organizations of the login, one `organization` of a
        // still older CLI, and an `organization_id` in each cached company.
        let text = json!({
            "version": 1,
            "default_profile": "p",
            "profiles": {
                "p": {
                    "base_url": "https://a.test",
                    "auth": "oauth",
                    "organizations": [{"id": "o1", "name": "Org A"}, {"id": "o2", "name": "Org B"}],
                    "company": "c1",
                    "companies": [{"id": "c1", "name": "A", "registry_code": "1",
                                   "state": "ready", "organization_id": "o1"},
                                  {"id": "c2", "name": "", "registry_code": "2",
                                   "state": "ready", "organization_id": "o2"}],
                    "colour": "blue",
                },
                "old": {"base_url": "https://b.test", "auth": "api_key",
                        "organization": {"id": "o1", "name": "Org"}},
                "none": {"base_url": "https://c.test", "auth": "api_key", "organization": null},
            },
        });
        fs::write(paths.config_file(), text.to_string()).unwrap();
        let config = Config::load(&paths).unwrap();
        let profile = &config.profiles["p"];
        assert_eq!(profile.company.as_deref(), Some("c1"));
        assert_eq!(profile.companies.as_ref().map(Vec::len), Some(2));
        assert_eq!(profile.company_names(), ["A"]);
        assert_eq!(
            profile.company_summaries(),
            [
                json!({"id": "c1", "name": "A"}),
                json!({"id": "c2", "name": ""})
            ]
        );
        assert_eq!(profile.extra.keys().collect::<Vec<_>>(), ["colour"]);
        assert!(config.profiles["old"].extra.is_empty());
        assert!(config.profiles["none"].extra.is_empty());
        assert!(config.profiles["none"].company_summaries().is_empty());
        // Saved again: no organization key, no `organization_id`, and the rest kept.
        config.save(&paths).unwrap();
        let saved: Value = serde_json::from_slice(&fs::read(paths.config_file()).unwrap()).unwrap();
        for name in ["p", "old", "none"] {
            let saved = &saved["profiles"][name];
            assert!(saved.get("organizations").is_none(), "{saved}");
            assert!(saved.get("organization").is_none(), "{saved}");
        }
        let saved = &saved["profiles"]["p"];
        assert_eq!(saved["colour"], "blue");
        assert_eq!(
            saved["companies"][0],
            json!({"id": "c1", "name": "A", "registry_code": "1", "state": "ready"})
        );
        assert_eq!(Config::load(&paths).unwrap(), config);
    }

    #[test]
    fn a_broken_file_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let paths = paths(dir.path());
        fs::create_dir_all(&paths.config_dir).unwrap();
        fs::write(paths.config_file(), "{").unwrap();
        assert!(
            Config::load(&paths)
                .unwrap_err()
                .message
                .contains("is not valid")
        );
        fs::remove_file(paths.config_file()).unwrap();
        fs::create_dir(paths.config_file()).unwrap();
        assert!(
            Config::load(&paths)
                .unwrap_err()
                .message
                .starts_with("Cannot read")
        );
        assert!(
            Config::default()
                .save(&paths)
                .unwrap_err()
                .message
                .starts_with("Cannot write")
        );
    }

    #[cfg(unix)]
    #[test]
    fn never_writes_through_a_link_at_the_temporary_path() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target");
        fs::write(&target, "untouched").unwrap();
        let path = dir.path().join("credentials.json");
        // The name of the temporary file before it was random.
        let predictable = dir
            .path()
            .join(format!(".credentials.json.{}.tmp", std::process::id()));
        std::os::unix::fs::symlink(&target, &predictable).unwrap();
        write_atomic(&path, b"{}", true).unwrap();
        assert_eq!(fs::read_to_string(&target).unwrap(), "untouched");
        let metadata = fs::symlink_metadata(&path).unwrap();
        assert!(metadata.file_type().is_file());
        assert_eq!(fs::read_to_string(&path).unwrap(), "{}");
        let leftovers: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(leftovers.len(), 3, "{leftovers:?}");
    }

    #[test]
    fn chooses_the_profile() {
        let mut config = Config::default();
        assert_eq!(config.default_name(), None);
        config
            .profiles
            .insert("a".into(), profile("https://a.test"));
        assert_eq!(config.default_name(), Some("a"));
        config
            .profiles
            .insert("b".into(), profile("https://b.test"));
        config
            .profiles
            .insert("b2".into(), profile("https://b.test"));
        assert_eq!(config.default_name(), None);
        config.default_profile = Some("b".into());
        assert_eq!(config.default_name(), Some("b"));
        assert_eq!(config.name_for_base_url("https://b.test"), Some("b"));
        assert_eq!(config.name_for_base_url("https://a.test"), Some("a"));
        assert_eq!(config.name_for_base_url("https://c.test"), None);
        config.default_profile = Some("a".into());
        assert_eq!(config.name_for_base_url("https://b.test"), None);
        config.default_profile = Some("gone".into());
        assert_eq!(config.default_name(), None);
    }

    #[test]
    fn removing_the_default_moves_it() {
        let mut config = Config::default();
        config
            .profiles
            .insert("a".into(), profile("https://a.test"));
        config
            .profiles
            .insert("b".into(), profile("https://b.test"));
        config.default_profile = Some("a".into());
        config.remove("a");
        assert_eq!(config.default_profile.as_deref(), Some("b"));
        config.remove("b");
        assert_eq!(config.default_profile, None);
        config
            .profiles
            .insert("a".into(), profile("https://a.test"));
        config
            .profiles
            .insert("b".into(), profile("https://b.test"));
        config
            .profiles
            .insert("c".into(), profile("https://c.test"));
        config.default_profile = Some("a".into());
        config.remove("a");
        assert_eq!(config.default_profile, None);
        assert_eq!(AuthKind::Oauth.as_str(), "oauth");
        assert_eq!(AuthKind::ApiKey.as_str(), "api_key");
    }
}
