//! Persistent configuration: defaults, then the JSON file, then environment
//! variables. Environment values are never written back to the file.

use std::{
    path::{Path, PathBuf},
    sync::{Arc, RwLock},
};

use serde::{Deserialize, Serialize};

use crate::util;

const DEFAULT_API_BASE_URL: &str = "https://api.trello.com/1";
const DEFAULT_TIMEOUT_SECONDS: i64 = 30;
const DEFAULT_MAX_UPLOAD_BYTES: i64 = 10 << 20;
const CONFIG_ENVIRONMENT_VARIABLE: &str = "TRELLO_MCP_CONFIG";

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Invalid(String),
    #[error("{context}: {cause}")]
    Io {
        context: String,
        cause: std::io::Error,
    },
    #[error("parse config {path}: {cause}")]
    Parse {
        path: String,
        cause: serde_json::Error,
    },
}

/// Credentials live in a file created with mode 0600; still treat it as a secret.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    #[serde(skip_serializing_if = "String::is_empty")]
    pub api_key: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub token: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub default_board_id: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub workspace_id: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub allowed_workspace_ids: Vec<String>,
    pub api_base_url: String,
    pub request_timeout_seconds: i64,
    pub max_upload_bytes: i64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            api_key: String::new(),
            token: String::new(),
            default_board_id: String::new(),
            workspace_id: String::new(),
            allowed_workspace_ids: Vec::new(),
            api_base_url: DEFAULT_API_BASE_URL.into(),
            request_timeout_seconds: DEFAULT_TIMEOUT_SECONDS,
            max_upload_bytes: DEFAULT_MAX_UPLOAD_BYTES,
        }
    }
}

pub type Env = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;

#[cfg(not(test))]
pub fn process_env() -> Env {
    Arc::new(|name| std::env::var(name).ok())
}

/// Tests never read the real environment (CODEX_HOME and friends).
#[cfg(test)]
pub fn process_env() -> Env {
    no_env()
}

#[cfg(test)]
pub fn no_env() -> Env {
    Arc::new(|_| None)
}

/// Tests never see the real home: they get one shared temp dir instead.
#[cfg(test)]
pub(crate) fn test_home() -> &'static Path {
    static HOME: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    HOME.get_or_init(|| tempfile::tempdir().unwrap().keep())
        .as_path()
}

#[cfg(test)]
pub(crate) fn home_dir() -> Result<PathBuf, Error> {
    Ok(test_home().to_path_buf())
}

#[cfg(not(test))]
pub(crate) fn home_dir() -> Result<PathBuf, Error> {
    dirs::home_dir().ok_or_else(|| {
        Error::Invalid(
            "resolve home directory: neither $HOME nor a user profile is available".into(),
        )
    })
}

fn io_error(context: &str, cause: std::io::Error) -> Error {
    Error::Io {
        context: context.to_string(),
        cause,
    }
}

fn default_path() -> Result<PathBuf, Error> {
    if let Ok(value) = std::env::var(CONFIG_ENVIRONMENT_VARIABLE) {
        let value = value.trim();
        if !value.is_empty() {
            return expand_home(value);
        }
    }
    Ok(home_dir()?.join(".trello-mcp").join("config.json"))
}

fn expand_home(path: &str) -> Result<PathBuf, Error> {
    if path == "~" || path.starts_with("~/") {
        return expand_home_with(path, &home_dir()?);
    }
    expand_home_with(path, Path::new(""))
}

// Core of expand_home with a known home; `~` forms behave like Go filepath.Join (cleaned).
fn expand_home_with(path: &str, home: &Path) -> Result<PathBuf, Error> {
    let rest = if path == "~" {
        ""
    } else if let Some(rest) = path.strip_prefix("~/") {
        rest.trim_start_matches('/')
    } else {
        return util::absolute(Path::new(path)).map_err(|e| io_error("resolve path", e));
    };
    util::absolute(&home.join(rest)).map_err(|e| io_error("resolve path", e))
}

fn parse_int(func: &str, value: &str) -> Result<i64, String> {
    use std::num::IntErrorKind::{NegOverflow, PosOverflow};
    value.parse::<i64>().map_err(|e| {
        let reason = match e.kind() {
            PosOverflow | NegOverflow => "value out of range",
            _ => "invalid syntax",
        };
        format!("{func}: parsing {value:?}: {reason}")
    })
}

/// Uses the supplied path when non-empty, otherwise the default path.
pub fn resolve_path(path: &str) -> Result<PathBuf, Error> {
    if path.trim().is_empty() {
        return default_path();
    }
    expand_home(path)
}

/// A missing file is equivalent to an empty file and returns the defaults.
pub fn load(path: &Path) -> Result<Config, Error> {
    let data = match std::fs::read(path) {
        Ok(data) => data,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Config::default()),
        Err(e) => return Err(io_error(&format!("read config {}", path.display()), e)),
    };
    let mut cfg: Config = serde_json::from_slice(&data).map_err(|cause| Error::Parse {
        path: path.display().to_string(),
        cause,
    })?;
    cfg.normalize();
    Ok(cfg)
}

// Go MkdirAll(dir, 0o700): every directory it creates gets 0700; existing ones are untouched.
fn create_parent(dir: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)
    }
    #[cfg(not(unix))]
    {
        std::fs::create_dir_all(dir)
    }
}

/// Atomically persists cfg with restrictive permissions.
pub fn save(path: &Path, cfg: &Config) -> Result<(), Error> {
    let mut cfg = cfg.clone();
    cfg.normalize();
    cfg.validate(false)?;
    let dir = match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => Path::new("."),
    };
    create_parent(dir).map_err(|e| io_error("create config directory", e))?;
    let mut text = serde_json::to_string_pretty(&cfg)
        .map_err(|e| Error::Invalid(format!("encode config: {e}")))?;
    text.push('\n');
    util::write_atomic(path, text.as_bytes(), 0o600, ".config-")
        .map_err(|e| io_error("replace config", e))
}

impl Config {
    fn normalize(&mut self) {
        self.api_key = self.api_key.trim().to_string();
        self.token = self.token.trim().to_string();
        self.default_board_id = self.default_board_id.trim().to_string();
        self.workspace_id = self.workspace_id.trim().to_string();
        self.api_base_url = self.api_base_url.trim().trim_end_matches('/').to_string();
        if self.api_base_url.is_empty() {
            self.api_base_url = DEFAULT_API_BASE_URL.into();
        }
        if self.request_timeout_seconds <= 0 {
            self.request_timeout_seconds = DEFAULT_TIMEOUT_SECONDS;
        }
        if self.max_upload_bytes <= 0 {
            self.max_upload_bytes = DEFAULT_MAX_UPLOAD_BYTES;
        }
        self.allowed_workspace_ids = parse_workspace_ids(&self.allowed_workspace_ids.join(","));
    }

    /// Checks configuration. When `for_server` is true credentials are required too.
    pub fn validate(&self, for_server: bool) -> Result<(), Error> {
        let invalid = |m: &str| Err(Error::Invalid(m.to_string()));
        if for_server && (self.api_key.is_empty() || self.token.is_empty()) {
            return invalid(
                "Trello credentials are required; run `trello-mcp config set --api-key ... --token ...` or set TRELLO_API_KEY and TRELLO_TOKEN",
            );
        }
        if !self.api_base_url.starts_with("https://") && !self.api_base_url.starts_with("http://") {
            return invalid("api_base_url must start with http:// or https://");
        }
        if self.request_timeout_seconds <= 0 {
            return invalid("request_timeout_seconds must be greater than zero");
        }
        if self.max_upload_bytes <= 0 {
            return invalid("max_upload_bytes must be greater than zero");
        }
        Ok(())
    }
}

/// Overlays environment variables without touching the file on disk.
pub fn apply_environment(mut cfg: Config, env: &Env) -> Result<Config, Error> {
    let set = |name: &str, dst: &mut String| {
        if let Some(value) = env(name) {
            *dst = value.trim().to_string();
        }
    };
    set("TRELLO_API_KEY", &mut cfg.api_key);
    set("TRELLO_TOKEN", &mut cfg.token);
    set("TRELLO_BOARD_ID", &mut cfg.default_board_id);
    set("TRELLO_WORKSPACE_ID", &mut cfg.workspace_id);
    set("TRELLO_API_BASE_URL", &mut cfg.api_base_url);
    if let Some(value) = env("TRELLO_ALLOWED_WORKSPACES") {
        cfg.allowed_workspace_ids = parse_workspace_ids(&value);
    }
    if let Some(value) = env("TRELLO_REQUEST_TIMEOUT_SECONDS") {
        cfg.request_timeout_seconds = parse_int("strconv.Atoi", value.trim())
            .map_err(|e| Error::Invalid(format!("parse TRELLO_REQUEST_TIMEOUT_SECONDS: {e}")))?;
    }
    if let Some(value) = env("TRELLO_MAX_UPLOAD_BYTES") {
        cfg.max_upload_bytes = parse_int("strconv.ParseInt", value.trim())
            .map_err(|e| Error::Invalid(format!("parse TRELLO_MAX_UPLOAD_BYTES: {e}")))?;
    }
    cfg.normalize();
    cfg.validate(false)?;
    Ok(cfg)
}

/// Comma-separated IDs, as the CLI and TRELLO_ALLOWED_WORKSPACES take them: trimmed, no blanks or repeats.
pub fn parse_workspace_ids(value: &str) -> Vec<String> {
    let mut result: Vec<String> = Vec::new();
    for item in value.split(',') {
        let item = item.trim();
        if !item.is_empty() && !result.iter().any(|r| r == item) {
            result.push(item.to_string());
        }
    }
    result
}

/// Synchronizes access to the persistent file and produces effective runtime
/// config with environment variables applied.
pub struct Manager {
    path: PathBuf,
    file: RwLock<Config>,
    env: Env,
}

impl Manager {
    pub fn new(path: &str) -> Result<Self, Error> {
        Self::with_env(path, process_env())
    }

    pub fn with_env(path: &str, env: Env) -> Result<Self, Error> {
        let path = resolve_path(path)?;
        let file = RwLock::new(load(&path)?);
        Ok(Self { path, file, env })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The persistent values without environment overrides.
    pub fn file(&self) -> Config {
        self.file.read().unwrap_or_else(|p| p.into_inner()).clone()
    }

    pub fn effective(&self) -> Result<Config, Error> {
        apply_environment(self.file(), &self.env)
    }

    pub fn update(&self, f: impl FnOnce(&mut Config) -> Result<(), Error>) -> Result<(), Error> {
        let mut guard = self.file.write().unwrap_or_else(|p| p.into_inner());
        // The file, not this process's startup copy: `config set` may have changed it since.
        let mut next = load(&self.path)?;
        f(&mut next)?;
        next.normalize();
        save(&self.path, &next)?;
        *guard = next;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn env_of(pairs: &[(&str, &str)]) -> Env {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        Arc::new(move |name| map.get(name).cloned())
    }

    #[test]
    fn save_load_and_permissions() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("config.json");
        let cfg = Config {
            api_key: " key ".into(),
            token: "token".into(),
            allowed_workspace_ids: vec!["a".into(), " b ".into(), "a".into()],
            ..Config::default()
        };
        save(&path, &cfg).unwrap();
        let loaded = load(&path).unwrap();
        assert_eq!(loaded.api_key, "key");
        assert_eq!(loaded.allowed_workspace_ids, vec!["a", "b"]);
        assert_eq!(loaded.api_base_url, "https://api.trello.com/1");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(
            text.starts_with("{\n  \"api_key\": \"key\",\n  \"token\": \"token\",\n"),
            "{text}"
        );
        assert!(
            !text.contains("default_board_id"),
            "empty fields must be omitted: {text}"
        );
        assert!(text.ends_with("}\n"));
    }

    #[test]
    fn missing_file_loads_defaults() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            load(&dir.path().join("none.json")).unwrap(),
            Config::default()
        );
    }

    #[test]
    fn environment_overrides_file() {
        let cfg = Config {
            api_key: "file-key".into(),
            ..Config::default()
        };
        let env = env_of(&[
            ("TRELLO_API_KEY", "env-key"),
            ("TRELLO_ALLOWED_WORKSPACES", "w1, w2,w1"),
            ("TRELLO_REQUEST_TIMEOUT_SECONDS", "45"),
            ("TRELLO_API_BASE_URL", "https://example.test/1/"),
        ]);
        let effective = apply_environment(cfg, &env).unwrap();
        assert_eq!(effective.api_key, "env-key");
        assert_eq!(effective.allowed_workspace_ids, vec!["w1", "w2"]);
        assert_eq!(effective.request_timeout_seconds, 45);
        assert_eq!(effective.api_base_url, "https://example.test/1");
        let bad = apply_environment(
            Config::default(),
            &env_of(&[("TRELLO_MAX_UPLOAD_BYTES", "x")]),
        )
        .unwrap_err();
        assert!(
            bad.to_string()
                .starts_with("parse TRELLO_MAX_UPLOAD_BYTES: "),
            "{bad}"
        );
    }

    #[test]
    fn manager_update_does_not_persist_environment() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        save(&path, &Config::default()).unwrap();
        let manager = Manager::with_env(
            path.to_str().unwrap(),
            env_of(&[("TRELLO_TOKEN", "env-token")]),
        )
        .unwrap();
        manager
            .update(|cfg| {
                cfg.default_board_id = "board".into();
                Ok(())
            })
            .unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("\"default_board_id\": \"board\""));
        assert!(!text.contains("env-token"));
        assert_eq!(manager.effective().unwrap().token, "env-token");
    }

    #[test]
    fn manager_update_keeps_changes_made_by_other_processes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        save(&path, &Config::default()).unwrap();
        let manager = Manager::with_env(path.to_str().unwrap(), no_env()).unwrap();
        // `config set` in another process, after this server loaded the file.
        let restricted = Config {
            allowed_workspace_ids: vec!["w1".into()],
            ..Config::default()
        };
        save(&path, &restricted).unwrap();
        manager
            .update(|cfg| {
                cfg.default_board_id = "board".into();
                Ok(())
            })
            .unwrap();
        let file = load(&path).unwrap();
        assert_eq!(file.allowed_workspace_ids, vec!["w1"]);
        assert_eq!(file.default_board_id, "board");
    }

    #[test]
    fn validate_messages_match_go() {
        let cfg = Config::default();
        assert_eq!(
            cfg.validate(true).unwrap_err().to_string(),
            "Trello credentials are required; run `trello-mcp config set --api-key ... --token ...` or set TRELLO_API_KEY and TRELLO_TOKEN"
        );
        let bad = Config {
            api_base_url: "ftp://x".into(),
            ..Config::default()
        };
        assert_eq!(
            bad.validate(false).unwrap_err().to_string(),
            "api_base_url must start with http:// or https://"
        );
    }

    #[test]
    fn expand_home_matches_go_join() {
        let home = Path::new("/h");
        for (input, want) in [
            ("~", "/h"),
            ("~/x", "/h/x"),
            ("~//x", "/h/x"),
            ("~/a/../b", "/h/b"),
            ("~/", "/h"),
        ] {
            assert_eq!(
                expand_home_with(input, home).unwrap(),
                Path::new(want),
                "{input}"
            );
        }
    }

    #[test]
    fn env_parse_errors_match_go_strconv() {
        let e = |k: &str, v: &str| {
            apply_environment(Config::default(), &env_of(&[(k, v)]))
                .unwrap_err()
                .to_string()
        };
        assert_eq!(
            e("TRELLO_REQUEST_TIMEOUT_SECONDS", "x"),
            "parse TRELLO_REQUEST_TIMEOUT_SECONDS: strconv.Atoi: parsing \"x\": invalid syntax"
        );
        assert_eq!(
            e("TRELLO_MAX_UPLOAD_BYTES", "99999999999999999999"),
            "parse TRELLO_MAX_UPLOAD_BYTES: strconv.ParseInt: parsing \"99999999999999999999\": value out of range"
        );
        assert_eq!(
            e("TRELLO_MAX_UPLOAD_BYTES", ""),
            "parse TRELLO_MAX_UPLOAD_BYTES: strconv.ParseInt: parsing \"\": invalid syntax"
        );
    }
}
