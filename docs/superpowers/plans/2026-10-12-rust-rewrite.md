# trello-mcp Rust Rewrite Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the Go implementation of trello-mcp with a Rust binary that behaves the same for every MCP client, CLI user, and config file.

**Architecture:** One binary crate whose modules mirror the Go packages (`config`, `trello`, `mcp`, `clientconfig`, CLI in `main.rs`). Trello data stays dynamic `serde_json::Value`, as Go uses `map[string]any`. The 28 tools stay table-driven: one table builds the JSON Schemas by hand and validates arguments. `rmcp`'s `ServerHandler` is implemented by hand, so `tools/list` matches a fixture captured from the Go binary.

**Tech Stack:** Rust 2024 edition (rustc 1.95 installed), rmcp 3.5.1 (`server`, `transport-io`), tokio 1, clap 4.6 (derive), reqwest 0.13 (default rustls TLS, `multipart`), serde_json, thiserror 2, anyhow 1 (main only), tempfile 3, dirs 6, base64 0.22, wiremock 0.6 (dev).

**Spec:** `/Users/loithai/Projects/ai-tools/trello-mcp/docs/superpowers/specs/2026-10-12-rust-rewrite-design.md`

**How to read this plan:** this is a port. The Go source in `/Users/loithai/Projects/ai-tools/trello-mcp/internal/` and `/Users/loithai/Projects/ai-tools/trello-mcp/cmd/` is the behavioral reference until Task 11 deletes it. Where a step says "port `func` (file:lines)", translate that Go function line by line into the Rust signature given, keeping every string literal byte-identical. New code that is not a straight translation (Rust glue, harnesses, replacements for Go library calls) is written out in full here.

## Global Constraints

- Package `trello-mcp`, version `2.0.0`, edition `2024`, `rust-version = "1.88"`, binary `trello-mcp`. `Cargo.toml` must begin with exactly `[package]\nname = "trello-mcp"\n`; `update` checks that prefix.
- Dependencies: only those in Task 1's `Cargo.toml`. `base64` is added beyond the spec's list: `add_attachment` decodes base64 `data` (Go used `encoding/base64`).
- Do not enable serde_json's `preserve_order` feature. Go encodes `map[string]any` with sorted keys; serde_json's default `Map` is a `BTreeMap`, which matches.
- Every string the user or model sees keeps the Go text byte for byte: tool and field descriptions, the server instructions, the prompt text, validation errors, Trello errors, and Markdown output. OS-level I/O failure text (`std::io::Error` wording) may differ from Go.
- Intentional differences from Go (and only these):
  1. MCP implementation name is `trello-mcp`, not `trello-mcp-go`.
  2. HTTP `User-Agent` is `trello-mcp/2.0.0`, not `trello-mcp-go/1.0.0`.
  3. Network-error text never contains the request URL; Go leaked key and token there.
  4. `update` uses cargo; `scripts/trello-mcp-dev` uses `cargo run`.
  5. Cobra's auto-generated `completion` command is dropped.
  6. Help text layout follows clap.
- `tools/list` returns tools sorted by name, as go-sdk does.
- No `unwrap()`/`expect()` on any input-driven path outside tests. Mutex poisoning recovers with `unwrap_or_else(|p| p.into_inner())`.
- Unix mode bits (`0600`, `0700`) go behind `#[cfg(unix)]` so the crate still compiles on Windows. Windows and Linux builds are not verified locally; aws-lc-sys cross-builds need a C cross toolchain.
- In `serve`, nothing may write to stdout except rmcp. No `println!` on any path `serve` reaches.
- Commit messages: conventional style (`feat:`, `test:`, `chore:`, `docs:`), short, no AI attribution lines.
- Per-task checks: `cargo fmt --check` and `cargo test`. `cargo clippy --all-targets -- -D warnings` becomes required from Task 10 on, once every module is reachable from `main`. Before that, dead-code warnings are expected.

## Review Focus

1. **Credentials in network errors.** A connection failure, timeout or DNS error must not put `key=`/`token=` into the tool error text. reqwest's error `Display` includes the URL, so every reqwest error passes through `.without_url()`. Pinned by Task 3 test `transport_error_does_not_leak_credentials`.
2. **Multibyte text at truncation points.** `shorten` (120 runes), `response_message` (500 bytes) and `mask` must not panic on emoji or CJK text. Pinned by Task 3 test `response_message_truncates_on_char_boundary`, Task 7 test `shorten_counts_characters` and Task 10 test `mask_handles_multibyte`.
3. **`--config` placement.** Clients launch both `serve --config PATH` and `--config PATH serve`. Both must select PATH. Pinned by Task 10 test `config_flag_parses_before_and_after_subcommand`.
4. **Stdout cleanliness under `serve`.** Any stray stdout byte breaks JSON-RPC for every client. Pinned by Task 11 step "stdio smoke": every stdout line must parse as JSON.
5. **Upgrading from the Go binary.** The installed Go `bin/trello-mcp update` cannot build a Rust checkout: it looks for `go.mod`. The switch needs one manual build, and the docs must say so. Pinned by Task 11 steps "rebuild bin" and "changelog upgrade note".

## File Structure

```
Cargo.toml, Cargo.lock           crate manifest (Task 1)
src/main.rs                      clap CLI, run(), serve(), update() (Task 1 stub, Task 10 full)
src/version.rs                   CURRENT, USER_AGENT (Task 1)
src/util.rs                      path_escape, path_unescape, absolute, write_atomic (Task 1)
src/config.rs                    Config, Manager, load/save/env (Task 2)
src/trello/mod.rs                Client, Error, call/execute/uploads/download/allow-list (Task 3)
src/mcp/mod.rs                   Server, Error, Output, Args, arg helpers (Task 4)
src/mcp/tools.rs                 Kind, FieldDefinition, ToolDefinition, TOOLS (Task 4)
src/mcp/server.rs                schema builder, validation, ServerHandler impl, prompt (Task 4)
src/mcp/test_support.rs          #[cfg(test)] wiremock harness + board fixture (Task 4)
src/mcp/board.rs                 board index, Refs, match_item, resolve_list, list_cards (Task 5)
src/mcp/handlers.rs              handle() dispatch, write/acked, ensure_*_allowed (Task 6)
src/mcp/extras.rs                create/update cards, custom fields, attachments, set_active,
                                 boards, activity, checklists, link_attachments, health (Task 7)
src/mcp/card.rs                  get_card: ref normalization, Markdown, delivery files (Task 8)
src/clientconfig/mod.rs          clients, aliases, render (Task 9)
src/clientconfig/project.rs      project paths, JSON merge, Codex block merge (Task 9)
tests/fixtures/tools_list.json   Go tools/list + prompts/list + instructions (Task 1)
tests/fixtures/client-config/    Go client-config outputs, 10 files (Task 1)
```

`src/mcp/card.rs` splits get_card out of the 1013-line Go `extras.go`, so each Rust file stays one responsibility.

---

### Task 1: Capture Go parity fixtures and scaffold the crate

**Files:**
- Create: `Cargo.toml`, `src/main.rs`, `src/version.rs`, `src/util.rs`
- Create: `tests/fixtures/tools_list.json`, `tests/fixtures/client-config/*.out` (10 files)
- Modify: `.gitignore` (add `target/`)

**Interfaces:**
- Produces: `crate::version::{CURRENT, USER_AGENT}`; `crate::util::{path_escape(&str) -> String, path_unescape(&str) -> Result<String, String>, absolute(&Path) -> io::Result<PathBuf>, write_atomic(&Path, &[u8], u32, &str) -> io::Result<()>}`; fixture files later tasks compare against.

- [ ] **Step 1: Build the Go binary and capture the MCP fixture**

Run from `/Users/loithai/Projects/ai-tools/trello-mcp`:

```bash
mkdir -p tests/fixtures/client-config
GO_BIN=$(mktemp -d)/go-trello-mcp
go build -o "$GO_BIN" ./cmd/trello-mcp
{ printf '%s\n' \
  '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"fixture","version":"1"}}}' \
  '{"jsonrpc":"2.0","method":"notifications/initialized"}' \
  '{"jsonrpc":"2.0","id":2,"method":"tools/list"}' \
  '{"jsonrpc":"2.0","id":3,"method":"prompts/list"}'; sleep 1; } \
| TRELLO_MCP_CONFIG=/nonexistent/config.json TRELLO_API_KEY=k TRELLO_TOKEN=t "$GO_BIN" serve \
| jq -s '{instructions: (map(select(.id==1))[0].result.instructions),
          tools: (map(select(.id==2))[0].result.tools),
          prompts: (map(select(.id==3))[0].result.prompts)}' \
> tests/fixtures/tools_list.json
jq '.tools | length, (.prompts | length)' tests/fixtures/tools_list.json
```

Expected: `28` then `1`.

- [ ] **Step 2: Capture client-config fixtures**

```bash
for c in codex claude-code claude-desktop antigravity opencode; do
  "$GO_BIN" client-config "$c" --command /x/trello-mcp > "tests/fixtures/client-config/$c.out"
  "$GO_BIN" client-config "$c" --command /x/trello-mcp --config /x/config.json > "tests/fixtures/client-config/$c.config.out"
done
ls tests/fixtures/client-config | wc -l
```

Expected: `10`.

- [ ] **Step 3: Write `Cargo.toml`**

```toml
[package]
name = "trello-mcp"
version = "2.0.0"
edition = "2024"
rust-version = "1.88"
description = "Model Context Protocol server for Trello"
license = "MIT"
repository = "https://github.com/thaitanloi365/trello-mcp"

[dependencies]
anyhow = "1"
base64 = "0.22"
clap = { version = "4.6", features = ["derive"] }
dirs = "6"
reqwest = { version = "0.13", features = ["multipart"] }
rmcp = { version = "3.5.1", default-features = false, features = ["server", "transport-io"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
tempfile = "3"
thiserror = "2"
tokio = { version = "1", features = ["rt-multi-thread", "macros", "io-std", "signal", "time"] }

[dev-dependencies]
wiremock = "0.6"

[profile.release]
strip = true
lto = "thin"
```

- [ ] **Step 4: Write `src/version.rs`**

```rust
//! The single release version shared by the CLI, MCP metadata, health
//! output, and the Trello HTTP client.

pub const CURRENT: &str = env!("CARGO_PKG_VERSION");
pub const USER_AGENT: &str = concat!("trello-mcp/", env!("CARGO_PKG_VERSION"));
```

- [ ] **Step 5: Write the failing tests for `src/util.rs`**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_escape_matches_go_path_segment_rules() {
        assert_eq!(path_escape("a b/c;d,e?f$&+:=@~-_.%#"), "a%20b%2Fc%3Bd%2Ce%3Ff$&+:=@~-_.%25%23");
        assert_eq!(path_escape("é"), "%C3%A9");
    }

    #[test]
    fn path_unescape_decodes_and_rejects_bad_escapes() {
        assert_eq!(path_unescape("spec%20v2.pdf").unwrap(), "spec v2.pdf");
        assert_eq!(path_unescape("a+b").unwrap(), "a+b");
        assert_eq!(path_unescape("bad%zz").unwrap_err(), "invalid URL escape \"%zz\"");
        assert_eq!(path_unescape("end%4").unwrap_err(), "invalid URL escape \"%4\"");
    }

    #[test]
    fn absolute_cleans_dot_segments() {
        let cwd = std::env::current_dir().unwrap();
        assert_eq!(absolute(Path::new("a/./b/../c")).unwrap(), cwd.join("a/c"));
        assert_eq!(absolute(Path::new("/x/y/../z/")).unwrap(), PathBuf::from("/x/z"));
    }

    #[test]
    fn write_atomic_replaces_file_with_mode() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("out.json");
        write_atomic(&path, b"one", 0o600, ".test-").unwrap();
        write_atomic(&path, b"two", 0o600, ".test-").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"two");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        }
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1, "temporary file left behind");
    }
}
```

- [ ] **Step 6: Write a minimal `src/main.rs` so the crate compiles**

```rust
mod util;
mod version;

fn main() {
    println!("{}", version::CURRENT);
}
```

Run: `cargo test util` and expect a compile FAIL: `cannot find function path_escape`.

- [ ] **Step 7: Implement `src/util.rs` above the tests**

```rust
//! Small replacements for Go standard-library helpers the port relies on.

use std::{
    io::{self, Write},
    path::{Component, Path, PathBuf},
};

/// Go url.PathEscape: escapes everything except unreserved characters and
/// `$&+:=@`, so `/`, `;`, `,` and `?` are escaped inside one path segment.
pub fn path_escape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || b"-_.~$&+:=@".contains(&byte) {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// Go url.PathUnescape: decodes %XX and keeps `+`. The error text matches Go.
pub fn path_unescape(value: &str) -> Result<String, String> {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let hex = bytes.get(index + 1..index + 3).filter(|pair| pair.iter().all(u8::is_ascii_hexdigit));
            let Some(hex) = hex else {
                let end = (index + 3).min(bytes.len());
                return Err(format!("invalid URL escape {:?}", String::from_utf8_lossy(&bytes[index..end])));
            };
            let text = std::str::from_utf8(hex).map_err(|err| err.to_string())?;
            out.push(u8::from_str_radix(text, 16).map_err(|err| err.to_string())?);
            index += 3;
        } else {
            out.push(bytes[index]);
            index += 1;
        }
    }
    Ok(String::from_utf8_lossy(&out).into_owned())
}

/// Go filepath.Abs: joins the working directory and cleans `.` and `..`
/// lexically, without resolving symlinks.
pub fn absolute(path: &Path) -> io::Result<PathBuf> {
    let joined = if path.is_absolute() { path.to_path_buf() } else { std::env::current_dir()?.join(path) };
    let mut out = PathBuf::new();
    for component in joined.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    Ok(out)
}

/// Writes a temporary file beside `path`, syncs it, and renames it into place,
/// so readers never see a partial file. `mode` applies on Unix only.
pub fn write_atomic(path: &Path, data: &[u8], mode: u32, prefix: &str) -> io::Result<()> {
    let directory = path.parent().filter(|parent| !parent.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let mut file = tempfile::Builder::new().prefix(prefix).suffix(".tmp").tempfile_in(directory)?;
    set_mode(file.path(), mode)?;
    file.write_all(data)?;
    file.as_file().sync_all()?;
    file.persist(path).map_err(|err| err.error)?;
    set_mode(path, mode)
}

#[cfg(unix)]
pub fn set_mode(path: &Path, mode: u32) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
}

#[cfg(not(unix))]
pub fn set_mode(_path: &Path, _mode: u32) -> io::Result<()> {
    Ok(())
}
```

- [ ] **Step 8: Run tests**

Run: `cargo test util`
Expected: 4 passed.

- [ ] **Step 9: Ignore build output and commit**

Append `target/` to `/Users/loithai/Projects/ai-tools/trello-mcp/.gitignore`.

```bash
git add Cargo.toml Cargo.lock src tests .gitignore
git commit -m "chore: scaffold Rust crate and capture Go parity fixtures"
```

---

### Task 2: Config module

**Files:**
- Create: `src/config.rs`
- Modify: `src/main.rs` (add `mod config;`)
- Reference: `internal/config/config.go`, `internal/config/config_test.go`

**Interfaces:**
- Consumes: `util::{absolute, write_atomic}`.
- Produces:
  - `pub struct Config { pub api_key: String, pub token: String, pub default_board_id: String, pub workspace_id: String, pub allowed_workspace_ids: Vec<String>, pub api_base_url: String, pub request_timeout_seconds: i64, pub max_upload_bytes: i64 }` (Default = Go `Default()`), plus `impl Config { pub fn validate(&self, for_server: bool) -> Result<(), Error>; fn normalize(&mut self) }`
  - `pub enum Error` (thiserror)
  - `pub type Env = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>`; `pub fn process_env() -> Env`; `#[cfg(test)] pub fn no_env() -> Env`
  - `pub fn resolve_path(path: &str) -> Result<PathBuf, Error>`; `pub fn load(path: &Path) -> Result<Config, Error>`; `pub fn save(path: &Path, cfg: &Config) -> Result<(), Error>`; `pub fn apply_environment(cfg: Config, env: &Env) -> Result<Config, Error>`; `pub fn parse_workspace_ids(value: &str) -> Vec<String>`
  - `pub struct Manager` with `new(path: &str) -> Result<Self, Error>`, `with_env(path: &str, env: Env) -> Result<Self, Error>`, `path(&self) -> &Path`, `file(&self) -> Config`, `effective(&self) -> Result<Config, Error>`, `update(&self, f: impl FnOnce(&mut Config) -> Result<(), Error>) -> Result<(), Error>`

- [ ] **Step 1: Write the failing tests** (ports of `config_test.go`; env goes through `Env`, so tests never mutate the process environment)

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn env_of(pairs: &[(&str, &str)]) -> Env {
        let map: HashMap<String, String> = pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        Arc::new(move |name| map.get(name).cloned())
    }

    #[test]
    fn save_load_and_permissions() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("config.json");
        let mut cfg = Config::default();
        cfg.api_key = " key ".into();
        cfg.token = "token".into();
        cfg.allowed_workspace_ids = vec!["a".into(), " b ".into(), "a".into()];
        save(&path, &cfg).unwrap();
        let loaded = load(&path).unwrap();
        assert_eq!(loaded.api_key, "key");
        assert_eq!(loaded.allowed_workspace_ids, vec!["a", "b"]);
        assert_eq!(loaded.api_base_url, "https://api.trello.com/1");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        }
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.starts_with("{\n  \"api_key\": \"key\",\n  \"token\": \"token\",\n"), "{text}");
        assert!(!text.contains("default_board_id"), "empty fields must be omitted: {text}");
        assert!(text.ends_with("}\n"));
    }

    #[test]
    fn missing_file_loads_defaults() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(load(&dir.path().join("none.json")).unwrap(), Config::default());
    }

    #[test]
    fn environment_overrides_file() {
        let mut cfg = Config::default();
        cfg.api_key = "file-key".into();
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
        let bad = apply_environment(Config::default(), &env_of(&[("TRELLO_MAX_UPLOAD_BYTES", "x")])).unwrap_err();
        assert!(bad.to_string().starts_with("parse TRELLO_MAX_UPLOAD_BYTES: "), "{bad}");
    }

    #[test]
    fn manager_update_does_not_persist_environment() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        save(&path, &Config::default()).unwrap();
        let manager = Manager::with_env(path.to_str().unwrap(), env_of(&[("TRELLO_TOKEN", "env-token")])).unwrap();
        manager.update(|cfg| {
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
    fn validate_messages_match_go() {
        let cfg = Config::default();
        assert_eq!(
            cfg.validate(true).unwrap_err().to_string(),
            "Trello credentials are required; run `trello-mcp config set --api-key ... --token ...` or set TRELLO_API_KEY and TRELLO_TOKEN"
        );
        let mut bad = Config::default();
        bad.api_base_url = "ftp://x".into();
        assert_eq!(bad.validate(false).unwrap_err().to_string(), "api_base_url must start with http:// or https://");
    }
}
```

- [ ] **Step 2: Run to see it fail**

Run: `cargo test config` (after adding `mod config;` to `src/main.rs`)
Expected: compile FAIL, unresolved items.

- [ ] **Step 3: Implement `src/config.rs`**

Port `config.go` with these Rust shapes:

```rust
//! Persistent configuration: defaults, then the JSON file, then environment
//! variables. Environment values are never written back to the file.

use std::{path::{Path, PathBuf}, sync::{Arc, RwLock}};

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
    #[error("{context}: {source}")]
    Io { context: String, source: std::io::Error },
    #[error("parse config {path}: {source}")]
    Parse { path: String, source: serde_json::Error },
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

pub fn process_env() -> Env {
    Arc::new(|name| std::env::var(name).ok())
}

#[cfg(test)]
pub fn no_env() -> Env {
    Arc::new(|_| None)
}

pub struct Manager {
    path: PathBuf,
    file: RwLock<Config>,
    env: Env,
}
```

Function-by-function port (keep every message):
- `default_path()` from `DefaultPath` (config.go:46-55): `TRELLO_MCP_CONFIG` (trimmed) via `std::env::var`, else `dirs::home_dir()` joined with `.trello-mcp/config.json`. A missing home dir gives `Error::Invalid("resolve home directory: ...")`.
- `expand_home` (config.go:57-70): `~` and `~/…` use `dirs::home_dir()`; everything else uses `util::absolute`.
- `resolve_path(path: &str)` (config.go:72-79).
- `load(path)` (config.go:81-96): `ErrorKind::NotFound` → `Ok(Config::default())`; read error → `Io { context: format!("read config {}", path.display()) }`; JSON error → `Parse`; then `normalize()`.
- `save(path, cfg)` (config.go:98-141): clone, normalize, `validate(false)`; `create_dir_all` parent and set its mode `0o700` on Unix (Go `MkdirAll(…, 0o700)`; only new dirs get that mode, so skip chmod if the dir existed); `serde_json::to_string_pretty` + `"\n"`; `util::write_atomic(path, bytes, 0o600, ".config-")`; map I/O errors to `Io { context: "replace config" }`.
- `normalize` (config.go:143-161), `validate` (config.go:163-179), `split_and_clean` (config.go:213-229), `parse_workspace_ids`.
- `apply_environment(cfg, env)` (config.go:181-211): call `env(name)` instead of `os.LookupEnv`. Timeout parse error is `Invalid(format!("parse TRELLO_REQUEST_TIMEOUT_SECONDS: {err}"))`, upload is `parse TRELLO_MAX_UPLOAD_BYTES: {err}`; parse `trim()`med text as `i64`. End with `normalize()` then `validate(false)`.
- `Manager::new(path)` = `with_env(path, process_env())`. `with_env` resolves the path and loads it. `file()` clones under read lock. `effective()` = `apply_environment(self.file(), &self.env)`. `update(f)` takes the write lock, clones, applies `f`, normalizes, `save`s, then stores. Recover poisoned locks with `unwrap_or_else(|p| p.into_inner())`.

- [ ] **Step 4: Run tests**

Run: `cargo test config`
Expected: 5 passed.

- [ ] **Step 5: Commit**

```bash
git add src/config.rs src/main.rs
git commit -m "feat: port config to Rust"
```

---

### Task 3: Trello HTTP client

**Files:**
- Create: `src/trello/mod.rs`
- Modify: `src/main.rs` (add `mod trello;`)
- Reference: `internal/trello/client.go`, `internal/trello/client_test.go`

**Interfaces:**
- Consumes: `config::{Config, Manager, Error}`, `util::{path_escape, write_atomic, set_mode, absolute}`, `version::USER_AGENT`.
- Produces:
  - `pub enum Error { Config(config::Error), Api { method: Method, path: String, status: u16, message: String }, Transport { method: Method, path: String, source: reqwest::Error }, Message(String) }`
  - `pub struct Client` with:
    - `pub fn new(config: Arc<Manager>) -> Self`
    - `pub fn manager(&self) -> &Manager`
    - `pub async fn call(&self, method: Method, path: &str, params: Value, body: Option<&Value>) -> Result<Value, Error>` (Go `Do`; `params` is a `json!` object or `Value::Null`)
    - `pub async fn upload_file(&self, card_id: &str, path: &str, name: &str) -> Result<Value, Error>`
    - `pub async fn upload_data(&self, card_id: &str, name: &str, data: &[u8]) -> Result<Value, Error>`
    - `pub async fn download_attachment(&self, card_id: &str, attachment_id: &str, destination: &str) -> Result<Value, Error>`
    - `pub fn board_id(&self, explicit: &str) -> Result<String, Error>`
    - `pub fn workspace_allowed(&self, id: &str) -> bool`
    - `pub fn require_workspace_allowed(&self, id: &str) -> Result<(), Error>`
    - `pub async fn ensure_board_allowed(&self, board_id: &str) -> Result<(), Error>`
  - `reqwest::Method` is the method type everywhere.

- [ ] **Step 1: Write the failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{self, Config};
    use std::collections::HashMap;
    use wiremock::{Mock, MockServer, Request, ResponseTemplate, matchers::any};

    struct Harness {
        client: Client,
        api: MockServer,
        _dir: tempfile::TempDir,
    }

    async fn test_client(handler: impl Fn(&Request) -> ResponseTemplate + Send + Sync + 'static) -> Harness {
        let api = MockServer::start().await;
        Mock::given(any()).respond_with(handler).mount(&api).await;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        let mut cfg = Config::default();
        cfg.api_key = "test-key".into();
        cfg.token = "test-token".into();
        cfg.api_base_url = api.uri();
        config::save(&path, &cfg).unwrap();
        let manager = Arc::new(Manager::with_env(path.to_str().unwrap(), config::no_env()).unwrap());
        Harness { client: Client::new(manager), api, _dir: dir }
    }

    fn query(request: &Request) -> HashMap<String, String> {
        request.url.query_pairs().into_owned().collect()
    }

    #[tokio::test]
    async fn call_adds_credentials_and_decodes_response() {
        let h = test_client(|_| ResponseTemplate::new(200).set_body_json(json!({"id": "card-1"}))).await;
        let result = h.client.call(Method::GET, "/cards/card-1", json!({"limit": 5}), None).await.unwrap();
        assert_eq!(result["id"], "card-1");
        let requests = h.api.received_requests().await.unwrap();
        let q = query(&requests[0]);
        assert_eq!(q["key"], "test-key");
        assert_eq!(q["token"], "test-token");
        assert_eq!(q["limit"], "5");
        assert_eq!(requests[0].headers["user-agent"], version::USER_AGENT);
    }

    #[tokio::test]
    async fn integral_float_params_have_no_decimal_point() {
        let h = test_client(|_| ResponseTemplate::new(200).set_body_string("")).await;
        let result = h.client.call(Method::PUT, "/cards/c", json!({"dueReminder": 1440.0, "closed": true}), None).await.unwrap();
        assert_eq!(result, json!({"ok": true}));
        let q = query(&h.api.received_requests().await.unwrap()[0]);
        assert_eq!(q["dueReminder"], "1440");
        assert_eq!(q["closed"], "true");
    }

    #[tokio::test]
    async fn api_error_does_not_leak_credentials() {
        let h = test_client(|_| ResponseTemplate::new(401).set_body_string("invalid key test-key")).await;
        let err = h.client.call(Method::GET, "/members/me", Value::Null, None).await.unwrap_err().to_string();
        assert!(err.starts_with("Trello GET /members/me returned HTTP 401: "), "{err}");
        assert!(!err.contains("test-token"), "{err}");
    }

    #[tokio::test]
    async fn transport_error_does_not_leak_credentials() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        let mut cfg = Config::default();
        cfg.api_key = "test-key".into();
        cfg.token = "test-token".into();
        cfg.api_base_url = "http://127.0.0.1:9".into(); // discard port: connection refused
        config::save(&path, &cfg).unwrap();
        let client = Client::new(Arc::new(Manager::with_env(path.to_str().unwrap(), config::no_env()).unwrap()));
        let err = client.call(Method::POST, "/cards", Value::Null, None).await.unwrap_err().to_string();
        assert!(err.starts_with("call Trello POST /cards: "), "{err}");
        assert!(!err.contains("test-key") && !err.contains("test-token"), "{err}");
    }

    #[tokio::test]
    async fn non_idempotent_request_is_not_retried_after_server_error() {
        let h = test_client(|_| ResponseTemplate::new(500).set_body_json(json!({"message": "boom"}))).await;
        let err = h.client.call(Method::POST, "/cards", Value::Null, None).await.unwrap_err().to_string();
        assert_eq!(err, "Trello POST /cards returned HTTP 500: boom");
        assert_eq!(h.api.received_requests().await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn download_attachment_uses_authenticated_download_endpoint() {
        let h = test_client(|request: &Request| match request.url.path() {
            "/cards/card-1/attachments/att-1" => ResponseTemplate::new(200).set_body_json(json!({"id": "att-1", "name": "spec v2.pdf"})),
            "/cards/card-1/attachments/att-1/download/spec%20v2.pdf" => ResponseTemplate::new(200).set_body_bytes(b"PDF".to_vec()),
            _ => ResponseTemplate::new(404),
        })
        .await;
        let dir = tempfile::tempdir().unwrap();
        let destination = dir.path().join("out.pdf");
        let result = h.client.download_attachment("card-1", "att-1", destination.to_str().unwrap()).await.unwrap();
        assert_eq!(result["bytes"], 3);
        assert_eq!(result["name"], "spec v2.pdf");
        assert_eq!(std::fs::read(&destination).unwrap(), b"PDF");
        let requests = h.api.received_requests().await.unwrap();
        let download = &requests[1];
        assert_eq!(download.headers["authorization"], "OAuth oauth_consumer_key=\"test-key\", oauth_token=\"test-token\"");
    }

    #[test]
    fn response_message_truncates_on_char_boundary() {
        let body = "é".repeat(600);
        let message = response_message(body.as_bytes(), StatusCode::BAD_REQUEST);
        assert!(message.ends_with("..."));
        assert!(message.len() <= 503);
        assert_eq!(response_message(b"", StatusCode::NOT_FOUND), "Not Found");
        assert_eq!(response_message(br#"{"error":"nope"}"#, StatusCode::BAD_REQUEST), "nope");
    }
}
```

- [ ] **Step 2: Run to see it fail**

Run: `cargo test trello`
Expected: compile FAIL.

- [ ] **Step 3: Implement the core of `src/trello/mod.rs`**

```rust
//! Trello REST client: credentials as query parameters, a conservative rate
//! limit, retries for idempotent calls, and errors that never carry the URL.

use std::{
    collections::BTreeMap,
    path::Path,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use reqwest::{Method, StatusCode, Url, header};
use serde_json::{Value, json};

use crate::{config::{self, Config, Manager}, util, version};

const MAX_RESPONSE_BYTES: usize = 16 << 20;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Config(#[from] config::Error),
    #[error("Trello {method} {path} returned HTTP {status}: {message}")]
    Api { method: Method, path: String, status: u16, message: String },
    #[error("call Trello {method} {path}: {source}")]
    Transport { method: Method, path: String, source: reqwest::Error },
    #[error("{0}")]
    Message(String),
}

fn message(text: impl Into<String>) -> Error {
    Error::Message(text.into())
}

enum Payload<'a> {
    Empty,
    Json(Vec<u8>),
    File { name: &'a str, data: &'a [u8] },
}

pub struct Client {
    config: Arc<Manager>,
    http: reqwest::Client,
    next: Mutex<Instant>,
}

impl Client {
    pub fn new(config: Arc<Manager>) -> Self {
        Self { config, http: reqwest::Client::new(), next: Mutex::new(Instant::now()) }
    }

    pub fn manager(&self) -> &Manager {
        &self.config
    }

    /// Spaces requests 105 ms apart to stay below Trello's per-token limit.
    async fn wait(&self) {
        let ready = {
            let mut next = self.next.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            let ready = (*next).max(Instant::now());
            *next = ready + Duration::from_millis(105);
            ready
        };
        tokio::time::sleep_until(ready.into()).await;
    }

    pub async fn call(&self, method: Method, path: &str, params: Value, body: Option<&Value>) -> Result<Value, Error> {
        let cfg = self.config.effective()?;
        cfg.validate(true)?;
        let url = build_url(&cfg, path, &params)?;
        let payload = match body {
            Some(body) => Payload::Json(
                serde_json::to_vec(body).map_err(|err| message(format!("encode Trello request body: {err}")))?,
            ),
            None => Payload::Empty,
        };
        self.execute(&cfg, method, path, url, payload).await
    }

    async fn execute(&self, cfg: &Config, method: Method, safe_path: &str, url: Url, payload: Payload<'_>) -> Result<Value, Error> {
        let timeout = Duration::from_secs(cfg.request_timeout_seconds.max(1) as u64);
        for attempt in 0..3 {
            self.wait().await;
            let mut request = self
                .http
                .request(method.clone(), url.clone())
                .timeout(timeout)
                .header(header::ACCEPT, "application/json")
                .header(header::USER_AGENT, version::USER_AGENT);
            request = match &payload {
                Payload::Empty => request,
                Payload::Json(bytes) => request.header(header::CONTENT_TYPE, "application/json").body(bytes.clone()),
                Payload::File { name, data } => {
                    let part = reqwest::multipart::Part::bytes(data.to_vec())
                        .file_name(name.to_string())
                        .mime_str("application/octet-stream")
                        .map_err(|err| message(format!("create attachment form: {}", err.without_url())))?;
                    request.multipart(reqwest::multipart::Form::new().part("file", part))
                }
            };
            let response = match request.send().await {
                Ok(response) => response,
                Err(err) => {
                    if attempt < 2 && idempotent(&method) {
                        continue;
                    }
                    return Err(Error::Transport { method, path: safe_path.into(), source: err.without_url() });
                }
            };
            let status = response.status();
            let retry_after = response
                .headers()
                .get(header::RETRY_AFTER)
                .and_then(|value| value.to_str().ok())
                .unwrap_or("")
                .to_string();
            let data = read_limited(response).await?;
            let retryable = status == StatusCode::TOO_MANY_REQUESTS || (status.is_server_error() && idempotent(&method));
            if retryable && attempt < 2 {
                tokio::time::sleep(retry_delay(&retry_after, attempt)).await;
                continue;
            }
            if !status.is_success() {
                return Err(Error::Api {
                    method,
                    path: safe_path.into(),
                    status: status.as_u16(),
                    message: response_message(&data, status),
                });
            }
            if data.trim_ascii().is_empty() {
                return Ok(json!({"ok": true}));
            }
            return Ok(serde_json::from_slice(&data)
                .unwrap_or_else(|_| json!({"ok": true, "response": String::from_utf8_lossy(&data)})));
        }
        unreachable!("the third attempt always returns")
    }
}

fn build_url(cfg: &Config, path: &str, params: &Value) -> Result<Url, Error> {
    let raw = format!("{}/{}", cfg.api_base_url.trim_end_matches('/'), path.trim_start_matches('/'));
    let mut url = Url::parse(&raw).map_err(|err| message(format!("build Trello URL: {err}")))?;
    let mut query = BTreeMap::new();
    query.insert("key".to_string(), cfg.api_key.clone());
    query.insert("token".to_string(), cfg.token.clone());
    if let Value::Object(params) = params {
        for (name, value) in params {
            if let Some(text) = query_value(value) {
                query.insert(name.clone(), text);
            }
        }
    }
    {
        let mut pairs = url.query_pairs_mut();
        for (name, value) in &query {
            pairs.append_pair(name, value);
        }
    }
    Ok(url)
}

/// Go appendQueryValue: integral numbers print without a decimal point.
fn query_value(value: &Value) -> Option<String> {
    match value {
        Value::Null => None,
        Value::String(text) => Some(text.clone()),
        Value::Bool(flag) => Some(flag.to_string()),
        Value::Number(number) => Some(match (number.as_i64(), number.as_f64()) {
            (Some(int), _) => int.to_string(),
            (None, Some(float)) => float.to_string(),
            _ => number.to_string(),
        }),
        Value::Array(items) => Some(
            items.iter().map(|item| item.as_str().map(str::to_string).unwrap_or_else(|| item.to_string())).collect::<Vec<_>>().join(","),
        ),
        Value::Object(_) => Some(value.to_string()),
    }
}

async fn read_limited(mut response: reqwest::Response) -> Result<Vec<u8>, Error> {
    let mut data = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|err| message(format!("read Trello response: {}", err.without_url())))?
    {
        data.extend_from_slice(&chunk);
        if data.len() > MAX_RESPONSE_BYTES {
            return Err(message("Trello response exceeded 16 MiB limit"));
        }
    }
    Ok(data)
}

fn idempotent(method: &Method) -> bool {
    [Method::GET, Method::HEAD, Method::OPTIONS, Method::PUT, Method::DELETE].contains(method)
}

fn retry_delay(value: &str, attempt: u32) -> Duration {
    match value.trim().parse::<u64>() {
        Ok(seconds) if seconds > 0 => Duration::from_secs(seconds),
        _ => Duration::from_millis(250 << attempt),
    }
}

fn response_message(data: &[u8], status: StatusCode) -> String {
    let text = String::from_utf8_lossy(data);
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return status.canonical_reason().unwrap_or("").to_string();
    }
    if let Ok(Value::Object(object)) = serde_json::from_slice::<Value>(data) {
        for key in ["message", "error"] {
            if let Some(Value::String(value)) = object.get(key)
                && !value.is_empty()
            {
                return value.clone();
            }
        }
    }
    if trimmed.len() > 500 {
        return format!("{}...", &trimmed[..trimmed.floor_char_boundary(500)]);
    }
    trimmed.to_string()
}
```

`str::floor_char_boundary` is stable since Rust 1.91. If clippy or the MSRV check objects, replace it with `(0..=500).rev().find(|i| trimmed.is_char_boundary(*i)).unwrap_or(0)`. Then set `rust-version = "1.91"` only if the method stays.

- [ ] **Step 4: Port the rest of `client.go` onto `Client`**
  - `upload_file` (client.go:260-286): `std::fs::metadata` (follows symlinks like `os.Stat`); `!is_file()` gives `attachment path must be a regular file`. Size over `cfg.max_upload_bytes` gives `attachment is {size} bytes; configured limit is {limit}`. Read with `std::fs::read`, then re-check the length, because the file can grow between the stat and the read. Default `name` is `Path::file_name`. Then `self.upload(...)`.
  - `upload_data` (client.go:288-300).
  - `upload` (client.go:390-413): `cfg.validate(true)?`; path `/cards/{path_escape(card_id)}/attachments`; `build_url(&cfg, &path, &Value::Null)`; `self.execute(&cfg, Method::POST, &path, url, Payload::File { name, data })`.
  - `download_attachment` (client.go:302-388): metadata via `call(GET, "/cards/{id}/attachments/{aid}")`; file name defaults to the attachment ID when blank. Build the download URL with `build_url(&cfg, &download_path, &Value::Null)`, which keeps key and token in the query as Go does. Send `Authorization: OAuth oauth_consumer_key="{key}", oauth_token="{token}"`, `User-Agent`, and the timeout. A non-2xx status gives `Error::Api { method: GET, path: "/attachments/download", status, message: canonical_reason }`. Resolve the destination with `util::absolute`. If the parent dir is missing, the error is `attachment destination directory does not exist: {parent}`. Stream chunks into `tempfile::Builder::new().prefix(".trello-attachment-").suffix(".tmp").tempfile_in(parent)`. Abort past `max_upload_bytes` with `attachment exceeds configured limit of {limit} bytes`. Sync, `persist`, `util::set_mode(0o600)`. Return `json!({"bytes": written, "name": file_name, "path": abs.display().to_string()})`.
  - `board_id` (client.go:415-427), `workspace_allowed` (429-440), `require_workspace_allowed` (442-450), `ensure_board_allowed` (452-474), with messages verbatim. Use `util::path_escape` wherever Go uses `url.PathEscape`.

- [ ] **Step 5: Run tests**

Run: `cargo test trello`
Expected: 7 passed. If `download_attachment_uses_authenticated_download_endpoint` fails on the encoded file name, confirm `path_escape("spec v2.pdf") == "spec%20v2.pdf"` and that `Url::parse` keeps the existing `%20`.

- [ ] **Step 6: Commit**

```bash
git add src/trello src/main.rs
git commit -m "feat: port Trello HTTP client to Rust"
```

---

### Task 4: MCP server core: tool table, schemas, validation, rmcp handler, prompt

**Files:**
- Create: `src/mcp/mod.rs`, `src/mcp/tools.rs`, `src/mcp/server.rs`, `src/mcp/test_support.rs`
- Modify: `src/main.rs` (add `mod mcp;`)
- Reference: `internal/mcpserver/server.go`, `internal/mcpserver/tools.go`, `internal/mcpserver/tools_test.go`

**Interfaces:**
- Consumes: `trello::{Client, Error}`, `config::Error`, `version::CURRENT`.
- Produces (used by Tasks 5-8):
  - `pub type Args = serde_json::Map<String, Value>;`
  - `pub enum Output { Json(Value), Text(String) }`
  - `#[derive(Debug, thiserror::Error)] pub enum Error { #[error(transparent)] Trello(#[from] trello::Error), #[error(transparent)] Config(#[from] config::Error), #[error("{0}")] Message(String) }` and `pub(crate) fn msg(text: impl Into<String>) -> Error`
  - `#[derive(Clone)] pub struct Server { pub(crate) client: Arc<trello::Client> }` with `pub fn new(client: Arc<trello::Client>) -> Self`
  - `pub(crate) async fn handle(&self, name: &str, args: &Args) -> Result<Output, Error>` (stub here: `Err(msg(format!("tool {name:?} is not implemented")))`; filled in by Task 6)
  - Argument helpers in `mod.rs`: `text(&Args, &str) -> String`, `text_default(&Args, &str, &str) -> String`, `integer(&Args, &str, i64) -> i64`, `boolean(&Args, &str, bool) -> bool`, `map_args(&Args, &[(&str, &str)]) -> Args` (pairs are (input, output)), `copy_arg(&mut Args, output: &str, &Args, input: &str)`, `nested_value<'a>(&'a Value, &[&str]) -> Option<&'a Value>`, `nested_string(&Value, &[&str]) -> String`, `object_slice(&Value) -> Vec<Args>` (owned clones), `string_slice(&Value) -> Vec<String>`, `copy_present(&mut Args, &Args, &[&str])`, `compact_objects(&Value, &[&str]) -> Vec<Value>`, `escape(&str) -> String` (= `util::path_escape`)
  - `tools::{Kind, FieldDefinition, ToolDefinition, TOOLS: LazyLock<Vec<ToolDefinition>>, REPLY_STYLE}`
  - `server::{input_schema, validate_arguments, encode_result, tool_list, prompt_list, INSTRUCTIONS}`
  - `test_support::{Harness, test_server, board_fixture, not_found, query, calls}`

- [ ] **Step 1: Write the failing tests** (in `src/mcp/server.rs`; ports of `tools_test.go` plus the fixture gate)

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp::tools::TOOLS;
    use serde_json::json;

    #[test]
    fn tools_and_prompts_match_go_fixture() {
        let fixture: Value = serde_json::from_str(include_str!("../../tests/fixtures/tools_list.json")).unwrap();
        let actual = json!({
            "instructions": INSTRUCTIONS,
            "tools": serde_json::to_value(tool_list()).unwrap(),
            "prompts": serde_json::to_value(prompt_list()).unwrap(),
        });
        assert_eq!(actual["instructions"], fixture["instructions"]);
        let (got, want) = (actual["tools"].as_array().unwrap(), fixture["tools"].as_array().unwrap());
        assert_eq!(got.len(), want.len());
        for (got, want) in got.iter().zip(want) {
            assert_eq!(got, want, "tool {} differs", want["name"]);
        }
        assert_eq!(actual["prompts"], fixture["prompts"]);
    }

    #[test]
    fn tool_definitions_are_complete_and_unique() {
        let mut names: Vec<_> = TOOLS.iter().map(|t| t.name).collect();
        assert_eq!(names.len(), 28);
        names.sort();
        names.dedup();
        assert_eq!(names.len(), 28);
        assert!(TOOLS.iter().all(|t| !t.description.is_empty()));
    }

    #[test]
    fn create_cards_schema_types_each_card() {
        let tool = TOOLS.iter().find(|t| t.name == "create_cards").unwrap();
        let schema = Value::Object(input_schema(&tool.fields));
        assert_eq!(schema["properties"]["cards"]["items"]["required"], json!(["name"]));
        assert_eq!(schema["properties"]["cards"]["items"]["properties"]["labels"]["items"]["type"], "string");
    }

    #[test]
    fn validation_messages_match_go() {
        let tool = TOOLS.iter().find(|t| t.name == "create_cards").unwrap();
        let args = |v: Value| v.as_object().unwrap().clone();
        assert_eq!(validate_arguments(&tool.fields, &args(json!({"cards": []}))).unwrap_err(), "argument \"list\" is required");
        assert_eq!(validate_arguments(&tool.fields, &args(json!({"list": " ", "cards": []}))).unwrap_err(), "argument \"list\" is required");
        assert_eq!(validate_arguments(&tool.fields, &args(json!({"list": "a", "cards": [], "x": 1}))).unwrap_err(), "unknown argument \"x\"");
        assert_eq!(validate_arguments(&tool.fields, &args(json!({"list": "a", "cards": "no"}))).unwrap_err(), "argument \"cards\" must be object_array");
        assert_eq!(validate_arguments(&tool.fields, &args(json!({"list": "a", "cards": [{}]}))).unwrap_err(), "cards[0]: argument \"name\" is required");
        let get_card = TOOLS.iter().find(|t| t.name == "get_card").unwrap();
        assert_eq!(validate_arguments(&get_card.fields, &args(json!({"cardId": "c", "commentsLimit": 1.5}))).unwrap_err(), "argument \"commentsLimit\" must be an integer");
        assert_eq!(validate_arguments(&get_card.fields, &args(json!({"cardId": "c", "delivery": "x"}))).unwrap_err(), "argument \"delivery\" must be one of auto, inline, file");
        assert!(validate_arguments(&get_card.fields, &args(json!({"cardId": "c", "commentsLimit": 3.0, "delivery": null}))).is_ok());
    }

    #[test]
    fn encode_result_keeps_ampersands() {
        assert_eq!(encode_result(&json!({"name": "Terms & <Privacy>"})), r#"{"name":"Terms & <Privacy>"}"#);
    }

    #[test]
    fn draft_reply_text_matches_go() {
        assert_eq!(
            draft_reply_text("abc", " say hi "),
            "Draft a reply comment for Trello card abc.\n1. Read the card with get_card.\n2. Write plain English: main point first, short '- ' bullets, no jargon. Link every attachment or URL as [name](url); never paste a bare long URL.\n3. Show me the draft. Post it with add_comment only after I approve.\n\nWhat to say: say hi\n"
        );
    }
}
```

- [ ] **Step 2: Run to see it fail**

Run: `cargo test mcp`
Expected: compile FAIL.

- [ ] **Step 3: Write `src/mcp/tools.rs`** (port of `tools.go`, complete)

```rust
//! The 28 tool definitions. Descriptions are the model's only documentation,
//! so their text must stay identical to the Go version.

use std::sync::LazyLock;

use serde_json::{Value, json};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Str,
    Number,
    Integer,
    Bool,
    StrArray,
    ObjectArray,
    Object,
}

impl Kind {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Kind::Str => "string",
            Kind::Number => "number",
            Kind::Integer => "integer",
            Kind::Bool => "boolean",
            Kind::StrArray => "string_array",
            Kind::ObjectArray => "object_array",
            Kind::Object => "object",
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct FieldDefinition {
    pub name: &'static str,
    pub kind: Kind,
    pub description: String,
    pub required: bool,
    pub enum_values: Vec<&'static str>,
    pub default: Option<Value>,
    pub items: Vec<FieldDefinition>,
}

#[derive(Debug, Clone)]
pub(crate) struct ToolDefinition {
    pub name: &'static str,
    pub description: String,
    pub fields: Vec<FieldDefinition>,
    pub read_only: bool,
}

/// House style for every comment posted to a card.
pub(crate) const REPLY_STYLE: &str = "Write plain English: main point first, short '- ' bullets, no jargon. Link every attachment or URL as [name](url); never paste a bare long URL.";
/// Keeps the preview rule on every tool whose text other people see.
const APPROVAL: &str = " Show the user the exact text and wait for approval first.";
const CARD_REF: &str = "Card ID, short link, or Trello card URL";
const LIST_REF: &str = "List name or ID; names resolve on boardId or the active board";
const POSITION: &str = "top, bottom, or a number";
const LABEL_COLORS: &[&str] = &["yellow", "purple", "blue", "red", "green", "orange", "black", "sky", "pink", "lime"];

fn field(name: &'static str, kind: Kind, description: impl Into<String>, required: bool) -> FieldDefinition {
    FieldDefinition { name, kind, description: description.into(), required, enum_values: Vec::new(), default: None, items: Vec::new() }
}
fn required(name: &'static str, kind: Kind, description: impl Into<String>) -> FieldDefinition {
    field(name, kind, description, true)
}
fn optional(name: &'static str, kind: Kind, description: impl Into<String>) -> FieldDefinition {
    field(name, kind, description, false)
}
fn defaulted(name: &'static str, kind: Kind, description: &str, value: Value) -> FieldDefinition {
    FieldDefinition { default: Some(value), ..optional(name, kind, description) }
}
fn defaulted_enum(name: &'static str, description: &str, value: &str, values: &[&'static str]) -> FieldDefinition {
    FieldDefinition { enum_values: values.to_vec(), default: Some(json!(value)), ..optional(name, Kind::Str, description) }
}
fn required_enum(name: &'static str, description: &str, values: &[&'static str]) -> FieldDefinition {
    FieldDefinition { enum_values: values.to_vec(), ..required(name, Kind::Str, description) }
}
fn enumerated(name: &'static str, description: &str, values: &[&'static str]) -> FieldDefinition {
    FieldDefinition { enum_values: values.to_vec(), ..optional(name, Kind::Str, description) }
}
fn board_field() -> FieldDefinition {
    optional("boardId", Kind::Str, "Board ID; defaults to the active board")
}
fn tool(name: &'static str, read_only: bool, description: impl Into<String>, fields: Vec<FieldDefinition>) -> ToolDefinition {
    ToolDefinition { name, description: description.into(), fields, read_only }
}

pub(crate) static TOOLS: LazyLock<Vec<ToolDefinition>> = LazyLock::new(tool_definitions);

fn tool_definitions() -> Vec<ToolDefinition> {
    use Kind::*;
    let approved = |text: &str| format!("{text}{APPROVAL}");
    let update_label_colors = [LABEL_COLORS, &["null"][..]].concat();
    vec![
        tool("get_card", true, "Read a card as Markdown: details, custom fields, checklists with IDs, attachment links, and recent comments", vec![
            required("cardId", Str, CARD_REF),
            defaulted("commentsLimit", Integer, "Latest comments to include, 0 to 100", json!(10)),
            defaulted_enum("delivery", "auto writes output over 16 KiB to a private file and returns its path", "auto", &["auto", "inline", "file"]),
        ]),
        tool("list_cards", true, "List open cards in a list, on a board, or, with neither, assigned to me", vec![
            optional("list", Str, LIST_REF), optional("boardId", Str, "Board ID to list all its open cards"),
            defaulted("limit", Integer, "Maximum cards", json!(100)),
        ]),
        tool("get_activity", true, "Recent actions on a card, or on a board when cardId is omitted", vec![
            optional("cardId", Str, CARD_REF), board_field(), defaulted("limit", Integer, "Maximum actions", json!(50)),
        ]),
        tool("get_board", true, "Get a board's lists, labels, members, and custom fields with their IDs", vec![board_field()]),
        tool("list_boards", true, "List open boards, optionally in one workspace", vec![optional("workspaceId", Str, "Workspace ID")]),
        tool("list_workspaces", true, "List workspaces", vec![]),
        tool("find_checklist_items", true, "Find checklist items by text, checklist name, or both, on a card or board; results are grouped by checklist", vec![
            optional("query", Str, "Case-insensitive text in the item"),
            optional("checklist", Str, "Checklist name, such as Acceptance Criteria"),
            optional("cardId", Str, format!("{CARD_REF}; omit to search the board")), board_field(),
        ]),
        tool("get_health", true, "Check configuration and Trello API reachability", vec![]),
        tool("download_attachment", false, "Download a card attachment to a local file", vec![
            required("cardId", Str, CARD_REF), required("attachmentId", Str, "Attachment ID"),
            required("destinationPath", Str, "Local destination path"),
        ]),
        tool("create_cards", false, approved("Create one or more cards in a list"), vec![
            required("list", Str, LIST_REF), board_field(),
            FieldDefinition {
                items: vec![
                    required("name", Str, "Card title"), optional("description", Str, "Markdown description"),
                    optional("due", Str, "ISO 8601 due date"), optional("start", Str, "ISO 8601 start date"),
                    optional("labels", StrArray, "Label names or IDs"), optional("members", StrArray, "Member names, usernames, IDs, or me"),
                    optional("position", Str, POSITION),
                ],
                ..required("cards", ObjectArray, "Cards to create, in order")
            },
        ]),
        tool("update_card", false, approved("Change a card's fields, dates, list, archive or watch state, labels, or members; omitted fields stay unchanged"), vec![
            required("cardId", Str, CARD_REF),
            optional("name", Str, "New title"), optional("description", Str, "New Markdown description"),
            optional("due", Str, "ISO 8601 due date; empty string clears"), optional("start", Str, "ISO 8601 start date; empty string clears"),
            optional("dueComplete", Bool, "Mark the due date done"), optional("dueReminder", Integer, "Reminder minutes before due; -1 for none"),
            optional("list", Str, "Move to this list, by name or ID on the card's board"), optional("position", Str, POSITION),
            optional("closed", Bool, "Archive (true) or restore (false)"), optional("subscribed", Bool, "Watch the card"),
            optional("addLabels", StrArray, "Label names or IDs to add"), optional("removeLabels", StrArray, "Label names or IDs to remove"),
            optional("addMembers", StrArray, "Member names, usernames, IDs, or me to add"), optional("removeMembers", StrArray, "Members to remove"),
        ]),
        tool("set_custom_field", false, "Set or clear a custom field on a card", vec![
            required("cardId", Str, CARD_REF), required("field", Str, "Custom field name or ID"),
            optional("value", Str, "Text, number, true or false, ISO date, or list option"),
            optional("clear", Bool, "Clear the field instead of setting a value"),
        ]),
        tool("copy_card", false, "Copy a card into a list", vec![
            required("sourceCardId", Str, CARD_REF), required("list", Str, LIST_REF), board_field(),
            optional("name", Str, "Name for the copy"), optional("position", Str, POSITION),
            optional("keepFromSource", StrArray, "Parts to copy: attachments, checklists, comments, due, labels, members, stickers"),
        ]),
        tool("create_list", false, "Create a list on a board", vec![
            board_field(), required("name", Str, "List name"), optional("position", Str, POSITION),
        ]),
        tool("update_list", false, "Rename, move, archive, or watch a list", vec![
            required("list", Str, LIST_REF), board_field(), optional("name", Str, "New name"),
            optional("position", Str, POSITION), optional("closed", Bool, "Archive (true) or restore (false)"),
            optional("subscribed", Bool, "Watch the list"),
        ]),
        tool("create_board", false, "Create a board in a workspace", vec![
            required("name", Str, "Board name"), optional("workspaceId", Str, "Workspace ID; defaults to the active workspace"),
            optional("description", Str, "Board description"), defaulted("defaultLists", Bool, "Create Trello's default lists", json!(true)),
        ]),
        tool("set_active", false, "Save the default board, workspace, or both for later calls", vec![
            optional("boardId", Str, "Board ID"), optional("workspaceId", Str, "Workspace ID"),
        ]),
        tool("add_comment", false, format!("Post a comment on a card. {REPLY_STYLE}{APPROVAL}"), vec![
            required("cardId", Str, CARD_REF), required("text", Str, "Comment text"),
        ]),
        tool("update_comment", false, format!("Edit a comment. {REPLY_STYLE}{APPROVAL}"), vec![
            required("commentId", Str, "Comment ID"), required("text", Str, "New comment text"),
        ]),
        tool("delete_comment", false, "Delete a comment", vec![required("commentId", Str, "Comment ID")]),
        tool("create_checklist", false, approved("Create a checklist on a card with its items"), vec![
            required("cardId", Str, CARD_REF), required("name", Str, "Checklist name"),
            optional("items", StrArray, "Item texts, in order"), optional("position", Str, POSITION),
        ]),
        tool("add_checklist_item", false, approved("Add an item to a checklist"), vec![
            required("checklistId", Str, "Checklist ID"), required("name", Str, "Item text"),
            optional("checked", Bool, "Start as done"), optional("position", Str, POSITION),
            optional("due", Str, "ISO 8601 due date"), optional("dueReminder", Integer, "Reminder minutes before due; -1 for none"),
            optional("member", Str, "Assignee name, username, ID, or me"),
        ]),
        tool("update_checklist_item", false, approved("Change a checklist item's text, done state, position, due date, or assignee"), vec![
            required("cardId", Str, CARD_REF), required("checkItemId", Str, "Checklist item ID"),
            optional("name", Str, "New text"), optional("checked", Bool, "Done (true) or not done (false)"),
            optional("position", Str, POSITION), optional("due", Str, "ISO 8601 due date; empty string clears"),
            optional("dueReminder", Integer, "Reminder minutes before due; -1 for none"),
            optional("member", Str, "Assignee name, username, ID, or me; empty string clears"),
        ]),
        tool("delete_checklist_item", false, "Delete a checklist item", vec![
            required("checklistId", Str, "Checklist ID"), required("checkItemId", Str, "Checklist item ID"),
        ]),
        tool("create_label", false, "Create a board label", vec![
            board_field(), required("name", Str, "Label name"), required_enum("color", "Label color", LABEL_COLORS),
        ]),
        tool("update_label", false, "Rename or recolor a board label", vec![
            required("labelId", Str, "Label ID"), optional("name", Str, "New name"),
            enumerated("color", "New color; null removes it", &update_label_colors),
        ]),
        tool("delete_label", false, "Delete a board label", vec![required("labelId", Str, "Label ID")]),
        tool("add_attachment", false, "Attach a link, a local file, or base64 data to a card; give exactly one of url, filePath, or data", vec![
            required("cardId", Str, CARD_REF), optional("url", Str, "Link or image URL"),
            optional("filePath", Str, "Local file path, subject to max_upload_bytes"), optional("data", Str, "Base64 file content"),
            optional("name", Str, "Attachment name; required with data"), optional("setCover", Bool, "Use the url image as the card cover"),
        ]),
    ]
}
```

- [ ] **Step 4: Write `src/mcp/server.rs`** (schema, validation, rmcp glue; APIs verified against rmcp 3.5.1)

```rust
//! MCP protocol glue: JSON Schemas from the tool table, argument validation,
//! and the rmcp ServerHandler.

use std::sync::Arc;

use rmcp::{
    ErrorData as McpError, RoleServer, ServerHandler,
    model::{
        CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, GetPromptRequestParams, GetPromptResponse,
        GetPromptResult, Implementation, ListPromptsResult, ListToolsResult, PaginatedRequestParams, Prompt, PromptArgument,
        PromptMessage, Role, ServerCapabilities, ServerConfig, Tool, ToolAnnotations,
    },
    service::RequestContext,
};
use serde_json::{Map, Value, json};

use super::{Args, Output, Server, tools::{FieldDefinition, Kind, REPLY_STYLE, TOOLS}};
use crate::version;

pub(crate) const INSTRUCTIONS: &str = "Trello tools. Cards take an ID, short link, or URL; lists, labels, members, and custom fields take a name or ID. Board tools default to the active board. Before any change other people see (comments, new or updated cards, checklists), show the user a preview and wait for approval. Keep text short: main point first.";

pub(crate) fn input_schema(fields: &[FieldDefinition]) -> Map<String, Value> {
    let mut properties = Map::new();
    let mut required = Vec::new();
    for field in fields {
        let mut property = Map::new();
        property.insert("description".into(), json!(field.description));
        match field.kind {
            Kind::StrArray => {
                property.insert("type".into(), json!("array"));
                property.insert("items".into(), json!({"type": "string"}));
            }
            Kind::ObjectArray => {
                property.insert("type".into(), json!("array"));
                let items = if field.items.is_empty() { json!({"type": "object"}) } else { Value::Object(input_schema(&field.items)) };
                property.insert("items".into(), items);
            }
            Kind::Object => {
                property.insert("type".into(), json!("object"));
                property.insert("additionalProperties".into(), json!(true));
            }
            kind => {
                property.insert("type".into(), json!(kind.as_str()));
            }
        }
        if !field.enum_values.is_empty() {
            property.insert("enum".into(), json!(field.enum_values));
        }
        if let Some(default) = &field.default {
            property.insert("default".into(), default.clone());
        }
        properties.insert(field.name.into(), Value::Object(property));
        if field.required {
            required.push(field.name);
        }
    }
    let mut schema = Map::new();
    schema.insert("type".into(), json!("object"));
    schema.insert("properties".into(), Value::Object(properties));
    schema.insert("additionalProperties".into(), json!(false));
    if !required.is_empty() {
        schema.insert("required".into(), json!(required));
    }
    schema
}

pub(crate) fn validate_arguments(fields: &[FieldDefinition], args: &Args) -> Result<(), String> {
    for field in fields {
        let missing = match args.get(field.name) {
            None | Some(Value::Null) => true,
            Some(Value::String(text)) => text.trim().is_empty(),
            Some(_) => false,
        };
        if field.required && missing {
            return Err(format!("argument {:?} is required", field.name));
        }
    }
    for (name, value) in args {
        let Some(field) = fields.iter().find(|field| field.name == name) else {
            return Err(format!("unknown argument {name:?}"));
        };
        if value.is_null() {
            continue;
        }
        validate_kind(field, value)?;
        if !field.items.is_empty()
            && let Value::Array(items) = value
        {
            for (index, item) in items.iter().enumerate() {
                let object = item.as_object().cloned().unwrap_or_default();
                validate_arguments(&field.items, &object).map_err(|err| format!("{}[{index}]: {err}", field.name))?;
            }
        }
    }
    Ok(())
}

fn validate_kind(field: &FieldDefinition, value: &Value) -> Result<(), String> {
    let valid = match field.kind {
        Kind::Str => value.is_string(),
        Kind::Bool => value.is_boolean(),
        Kind::Number | Kind::Integer => match value.as_f64() {
            Some(number) if field.kind == Kind::Integer && number.trunc() != number => {
                return Err(format!("argument {:?} must be an integer", field.name));
            }
            Some(_) => true,
            None => false,
        },
        Kind::StrArray | Kind::ObjectArray => value.is_array(),
        Kind::Object => value.is_object(),
    };
    if !valid {
        return Err(format!("argument {:?} must be {}", field.name, field.kind.as_str()));
    }
    if !field.enum_values.is_empty() && !field.enum_values.contains(&value.as_str().unwrap_or("")) {
        return Err(format!("argument {:?} must be one of {}", field.name, field.enum_values.join(", ")));
    }
    Ok(())
}

/// Compact JSON. serde_json never HTML-escapes, so `&` stays `&` for the model.
pub(crate) fn encode_result(value: &Value) -> String {
    value.to_string()
}

/// Sorted by name: go-sdk lists tools that way.
pub(crate) fn tool_list() -> Vec<Tool> {
    let mut tools: Vec<Tool> = TOOLS
        .iter()
        .map(|definition| {
            let tool = Tool::new(definition.name, definition.description.clone(), Arc::new(input_schema(&definition.fields)));
            if definition.read_only { tool.with_annotations(ToolAnnotations::new().read_only(true)) } else { tool }
        })
        .collect();
    tools.sort_by(|a, b| a.name.cmp(&b.name));
    tools
}

pub(crate) fn prompt_list() -> Vec<Prompt> {
    vec![Prompt::new(
        "draft_reply",
        Some("Draft a reply comment for a Trello card and post it only after approval"),
        Some(vec![
            PromptArgument::new("card").with_description("Card ID, short link, or Trello card URL").with_required(true),
            PromptArgument::new("notes").with_description("What the reply should say"),
        ]),
    )]
}

pub(crate) fn draft_reply_text(card: &str, notes: &str) -> String {
    let mut text = format!(
        "Draft a reply comment for Trello card {card}.\n1. Read the card with get_card.\n2. {REPLY_STYLE}\n3. Show me the draft. Post it with add_comment only after I approve.\n"
    );
    let notes = notes.trim();
    if !notes.is_empty() {
        text.push_str(&format!("\nWhat to say: {notes}\n"));
    }
    text
}

fn tool_error(message: impl Into<String>) -> CallToolResponse {
    CallToolResult::error(vec![ContentBlock::text(message.into())]).into()
}

impl ServerHandler for Server {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().enable_prompts().build())
            .with_server_info(Implementation::new("trello-mcp", version::CURRENT))
            .with_instructions(INSTRUCTIONS)
    }

    async fn list_tools(&self, _: Option<PaginatedRequestParams>, _: RequestContext<RoleServer>) -> Result<ListToolsResult, McpError> {
        Ok(ListToolsResult::with_all_items(tool_list()))
    }

    async fn call_tool(&self, request: CallToolRequestParams, _: RequestContext<RoleServer>) -> Result<CallToolResponse, McpError> {
        let Some(definition) = TOOLS.iter().find(|tool| tool.name == request.name) else {
            return Err(McpError::invalid_params(format!("unknown tool {:?}", request.name), None));
        };
        let args = request.arguments.unwrap_or_default();
        if let Err(message) = validate_arguments(&definition.fields, &args) {
            return Ok(tool_error(message));
        }
        Ok(match self.handle(definition.name, &args).await {
            Ok(Output::Text(text)) => CallToolResult::success(vec![ContentBlock::text(text)]).into(),
            Ok(Output::Json(value)) => CallToolResult::success(vec![ContentBlock::text(encode_result(&value))]).into(),
            Err(err) => tool_error(err.to_string()),
        })
    }

    async fn list_prompts(&self, _: Option<PaginatedRequestParams>, _: RequestContext<RoleServer>) -> Result<ListPromptsResult, McpError> {
        Ok(ListPromptsResult::with_all_items(prompt_list()))
    }

    async fn get_prompt(&self, request: GetPromptRequestParams, _: RequestContext<RoleServer>) -> Result<GetPromptResponse, McpError> {
        let argument = |name: &str| {
            request.arguments.as_ref().and_then(|args| args.get(name)).and_then(Value::as_str).unwrap_or("").trim().to_string()
        };
        let card = argument("card");
        if card.is_empty() {
            return Err(McpError::invalid_params(format!("argument {:?} is required", "card"), None));
        }
        Ok(GetPromptResult::new(vec![PromptMessage::new_text(Role::User, draft_reply_text(&card, &argument("notes")))])
            .with_description("Draft a Trello reply")
            .into())
    }
}
```

If a model type's name or constructor has moved, check `~/.cargo/registry/src/*/rmcp-3.5.1/src/model.rs` and `src/model/{tool,prompt}.rs`; this code compiled against 3.5.1 in a probe.

- [ ] **Step 5: Write `src/mcp/mod.rs`** with the `Server`, `Error`, `Output`, `Args`, `msg` items from Interfaces, `mod tools; mod server; #[cfg(test)] pub(crate) mod test_support;` (Tasks 5-8 add `mod board; mod handlers; mod extras; mod card;`), the `handle` stub, and the argument helpers. Port the helpers from handlers.go:345-393, extras.go:767-790 and extras.go:860-871, extras.go:992-1013, board.go:222-231:
  - `text`: string value, `trim()`med; anything else is `""`.
  - `integer`: `as_f64()` then `as i64` (Go `int(float64)` truncates).
  - `copy_arg` skips absent keys and `null`.
  - `copy_present` skips absent, `null`, and `""` strings.
  - `object_slice` keeps only object elements of an array; `string_slice` only string elements.
  - `nested_value` walks object keys; any non-object step gives `None`.

- [ ] **Step 6: Write `src/mcp/test_support.rs`** (shared by Tasks 5-8; port of `testServer` and `boardFixture` from handlers_test.go:17-50)

```rust
//! Test harness: a wiremock stand-in for Trello and a Server pointed at it.

use std::{collections::HashMap, sync::Arc};

use serde_json::{Value, json};
use wiremock::{Mock, MockServer, Request, ResponseTemplate, matchers::any};

use super::Server;
use crate::{config::{self, Config, Manager}, trello::Client};

pub(crate) struct Harness {
    pub server: Server,
    pub manager: Arc<Manager>,
    pub api: MockServer,
    pub dir: tempfile::TempDir,
}

/// Assert on recorded requests after the call, not inside the responder: a
/// panic there only surfaces as a connection error.
pub(crate) async fn test_server(allowed: &[&str], handler: impl Fn(&Request) -> ResponseTemplate + Send + Sync + 'static) -> Harness {
    let api = MockServer::start().await;
    Mock::given(any()).respond_with(handler).mount(&api).await;
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.json");
    let mut cfg = Config::default();
    cfg.api_key = "key".into();
    cfg.token = "token".into();
    cfg.api_base_url = api.uri();
    cfg.allowed_workspace_ids = allowed.iter().map(|id| id.to_string()).collect();
    config::save(&path, &cfg).expect("save config");
    let manager = Arc::new(Manager::with_env(path.to_str().expect("utf-8 path"), config::no_env()).expect("manager"));
    Harness { server: Server::new(Arc::new(Client::new(manager.clone()))), manager, api, dir }
}

pub(crate) fn json_response(value: Value) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(value)
}

pub(crate) fn not_found() -> ResponseTemplate {
    ResponseTemplate::new(404).set_body_string("404 page not found\n")
}

pub(crate) fn query(request: &Request) -> HashMap<String, String> {
    request.url.query_pairs().into_owned().collect()
}

pub(crate) async fn calls(api: &MockServer) -> Vec<String> {
    api.received_requests().await.unwrap_or_default().iter().map(|r| format!("{} {}", r.method, r.url.path())).collect()
}

pub(crate) fn board_fixture() -> Value {
    json!({
        "id": "board-1", "name": "Board",
        "lists": [
            {"id": "list-todo", "name": "To Do"},
            {"id": "list-done", "name": "Done"},
            {"id": "list-old", "name": "Done", "closed": true}
        ],
        "labels": [
            {"id": "label-bug", "name": "Bug", "color": "red"},
            {"id": "label-blue", "name": "", "color": "blue"}
        ],
        "members": [{"id": "member-joe", "fullName": "Joe Thai", "username": "joe"}]
    })
}
```

- [ ] **Step 7: Run tests**

Run: `cargo test mcp`
Expected: 6 passed. When `tools_and_prompts_match_go_fixture` fails, the assertion message names the tool; diff its description and schema against `tools.go`.

- [ ] **Step 8: Commit**

```bash
git add src/mcp src/main.rs
git commit -m "feat: port MCP tool table, schemas, and rmcp handler"
```

---

### Task 5: Board resolution and list_cards

**Files:**
- Create: `src/mcp/board.rs`
- Modify: `src/mcp/mod.rs` (`mod board;`)
- Reference: `internal/mcpserver/board.go`; tests `TestMatchPrefersNamesAndOpenItems` (handlers_test.go:328-344), `TestListCardsUsesNames` (handlers_test.go:140-162)

**Interfaces:**
- Consumes: Task 4 helpers, `Server`, `trello::Client::{call, board_id, ensure_board_allowed}`.
- Produces:
  - `impl Server { pub(crate) async fn board_index(&self, board_id: &str) -> Result<Args, Error>; pub(crate) async fn board(&self, args: &Args) -> Result<String, Error>; pub(crate) fn board_refs(&self, args: &Args) -> Refs; pub(crate) fn board_id_refs(&self, board_id: &str) -> Refs; pub(crate) fn card_refs(&self, card_id: &str) -> Refs; pub(crate) fn checklist_refs(&self, checklist_id: &str) -> Refs; pub(crate) async fn resolve_list(&self, args: &Args, fallback: Option<Refs>) -> Result<(String, Refs), Error>; pub(crate) async fn list_cards(&self, args: &Args) -> Result<Value, Error> }`
  - `pub(crate) fn board_summary(index: &Args) -> Value`
  - `pub(crate) enum BoardSource { Args(Args), Id(String), Parent(String) }` and `pub(crate) struct Refs { source: BoardSource, pub(crate) index: Option<Args> }` with `pub(crate) async fn load(&mut self, server: &Server) -> Result<&Args, Error>`, `pub(crate) async fn id(&mut self, server: &Server, kind: &str, value: &str) -> Result<String, Error>`, `pub(crate) async fn ids(&mut self, server: &Server, kind: &str, values: &[Value]) -> Result<Vec<String>, Error>`
  - `pub(crate) fn match_item(items: &[Args], kind: &str, value: &str) -> Result<Args, Error>`, `pub(crate) fn display_name(kind: &str, item: &Args) -> String`, `pub(crate) fn names_of(index: &Args, kind: &str, ids: &Value) -> Vec<String>`, `pub(crate) fn is_trello_id(value: &str) -> bool`

Go's `refs.board` is a closure. Here it becomes `BoardSource`, resolved by `async fn board_id(&self, server: &Server) -> Result<String, Error>`:
- `Args(args)` → `server.board(args)`
- `Id(id)` → `id`
- `Parent(path)` → `GET path?fields=idBoard`; a missing `idBoard` gives `Trello response did not include idBoard`

`Server::board` (handlers.go:203-212) also lives here, since every board lookup uses it.

- [ ] **Step 1: Write the failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp::test_support::*;
    use serde_json::json;

    fn objects(value: Value) -> Vec<Args> {
        crate::mcp::object_slice(&value)
    }

    #[test]
    fn match_prefers_names_and_open_items() {
        let lists = objects(board_fixture()["lists"].clone());
        assert_eq!(match_item(&lists, "lists", "done").unwrap()["id"], "list-done");
        let labels = objects(board_fixture()["labels"].clone());
        assert_eq!(match_item(&labels, "labels", "blue").unwrap()["id"], "label-blue");
        let members = objects(board_fixture()["members"].clone());
        assert_eq!(match_item(&members, "members", "@JOE").unwrap()["id"], "member-joe");
        assert_eq!(
            match_item(&lists, "lists", "Backlog").unwrap_err().to_string(),
            "list \"Backlog\" not found; options: To Do, Done"
        );
        let twins = objects(json!([{"id": "a", "name": "Same"}, {"id": "b", "name": "same"}]));
        assert_eq!(
            match_item(&twins, "lists", "same").unwrap_err().to_string(),
            "list \"same\" matches 2 entries; use an ID from get_board"
        );
    }

    #[test]
    fn trello_ids_are_24_lowercase_hex() {
        assert!(is_trello_id("0123456789abcdef01234567"));
        assert!(!is_trello_id("0123456789ABCDEF01234567"));
        assert!(!is_trello_id("list-todo"));
    }

    #[tokio::test]
    async fn list_cards_uses_names() {
        let h = test_server(&[], |request| match request.url.path() {
            "/boards/board-1" => json_response(board_fixture()),
            "/lists/list-todo/cards" => json_response(json!([{
                "shortLink": "abc12345", "name": "Card", "idBoard": "board-1", "idList": "list-todo",
                "idLabels": ["label-bug", "label-blue"], "idMembers": ["member-joe"], "dueComplete": true, "due": "2026-11-01"
            }])),
            _ => not_found(),
        })
        .await;
        let args = json!({"list": "To Do", "boardId": "board-1"}).as_object().unwrap().clone();
        let result = h.server.list_cards(&args).await.unwrap();
        assert_eq!(
            result,
            json!([{"id": "abc12345", "name": "Card", "due": "2026-11-01", "dueComplete": true, "labels": ["Bug", "blue"], "members": ["Joe Thai"]}])
        );
        assert_eq!(calls(&h.api).await, vec!["GET /boards/board-1", "GET /lists/list-todo/cards"]);
    }
}
```

Read the Go `TestListCardsUsesNames` (handlers_test.go:140-162) and copy any extra assertion it makes into the Rust test.

- [ ] **Step 2: Run to see it fail**

Run: `cargo test board`
Expected: compile FAIL.

- [ ] **Step 3: Implement `src/mcp/board.rs`** by porting board.go function by function:
  - `board_index` (13-32): params `{"fields": "name,url", "lists": "all", "list_fields": "name,closed", "labels": "all", "label_fields": "name,color", "labels_limit": 1000, "members": "all", "member_fields": "fullName,username", "customFields": true}`. A non-object result gives `Trello returned an invalid board response`.
  - `board_summary` (34-67).
  - `Refs::load` (105-116), `Refs::id` (118-133): the `me` check is `value.trim_start_matches('@')` compared case-insensitively to `"me"`, but strip only one `@`; Go uses `TrimPrefix`. Use `value.strip_prefix('@').unwrap_or(value).eq_ignore_ascii_case("me")`.
  - `Refs::ids` (135-146): string elements as-is, others via `Value::to_string()`.
  - `match_item` (150-187): `singular` = kind without the trailing `s`. The Go `[2][]map` buckets become `[[Vec<Args>; 2]; 2]` or four `Vec`s, searched in the same order (open names, open fallback, archived names, archived fallback). Lowercase with `to_lowercase()`.
  - `display_name` (189-202), `names_of` (204-220), `is_trello_id` (233-243), `resolve_list` (249-270; error `list is required`), `list_cards` (272-338). The `indexes` map is a `HashMap<String, Args>`; when `resolve_list` returned a loaded `Refs`, seed it with `refs.index` keyed by its `id`.

- [ ] **Step 4: Run tests**

Run: `cargo test board`
Expected: 3 passed.

- [ ] **Step 5: Commit**

```bash
git add src/mcp/board.rs src/mcp/mod.rs
git commit -m "feat: port board name resolution and list_cards"
```

---

### Task 6: Tool dispatch, write acknowledgements, allow-list checks

**Files:**
- Create: `src/mcp/handlers.rs`
- Modify: `src/mcp/mod.rs` (`mod handlers;`, delete the Task 4 `handle` stub)
- Reference: `internal/mcpserver/handlers.go`; tests `TestWorkspaceRestrictionBlocksCardMutation` (handlers_test.go:210-232), `TestCopyCardUsesFullSourceIDAndSourceBoard` (306-326)

**Interfaces:**
- Consumes: Tasks 3-5.
- Produces:
  - `impl Server { pub(crate) async fn handle(&self, name: &str, args: &Args) -> Result<Output, Error>; pub(crate) async fn allowed_card(&self, args: &Args, key: &str) -> Result<String, Error>; pub(crate) async fn workspace_id(&self, args: &Args) -> Result<String, Error>; pub(crate) async fn write(&self, method: Method, path: &str, params: Value, body: Option<Value>) -> Result<Value, Error>; pub(crate) fn workspace_restrictions_enabled(&self) -> Result<bool, Error>; pub(crate) async fn ensure_card_allowed(&self, id: &str) -> Result<(), Error>; ensure_list_allowed; ensure_checklist_allowed; ensure_action_allowed; ensure_label_allowed (same signature) }`
  - `pub(crate) fn acked(value: Value) -> Value`
  - `pub(crate) fn join_values(values: &[Value]) -> String`
- Calls into Tasks 7-8, which must exist with these names: `create_cards`, `update_card`, `set_custom_field`, `add_attachment`, `set_active(board_id: &str, workspace_id: &str)`, `list_boards(workspace_id: &str)`, `activity`, `find_checklist_items`, `create_checklist`, `health()`, `filter_objects_by_workspace(Value, &str) -> Value`, `link_attachments(&str) -> String` (free fn in extras), `normalize_card_ref(&str) -> Result<String, Error>` and `get_card(&str, GetCardOptions) -> Result<Output, Error>` (card.rs). All are `async fn(&self, args: &Args) -> Result<Value, Error>` unless a signature is given. To keep this task compiling on its own, add those as `todo!()` stubs in `extras.rs` and `card.rs`, then replace them in Tasks 7-8.

The five Go `ensure*Allowed` functions differ only in path, fields and messages. Port them as one helper plus five one-liners:

```rust
async fn ensure_allowed(&self, path: String, fields: &str, board_path: &[&str], missing: &str) -> Result<(), Error> {
    if !self.workspace_restrictions_enabled()? {
        return Ok(());
    }
    let object = self.client.call(Method::GET, &path, json!({"fields": fields}), None).await?;
    let board_id = nested_string(&object, board_path);
    if board_id.is_empty() {
        return Err(msg(missing));
    }
    Ok(self.client.ensure_board_allowed(&board_id).await?)
}

pub(crate) async fn ensure_card_allowed(&self, id: &str) -> Result<(), Error> {
    self.ensure_allowed(format!("/cards/{}", escape(id)), "id,idBoard", &["idBoard"], "Trello card response did not include idBoard").await
}
// lists: "/lists/{}", "id,idBoard", "Trello list response did not include idBoard"
// checklists: "/checklists/{}", "id,idBoard", "Trello checklist response did not include idBoard"
// actions: "/actions/{}", "data", &["data", "board", "id"], "Trello action response did not include a board ID"
// labels: "/labels/{}", "id,idBoard", "Trello label response did not include idBoard"
```

`acked` (handlers.go:235-255), in full:

```rust
/// A short ack instead of the full Trello object, which can run to several
/// KiB; get_card has the full view.
pub(crate) fn acked(value: Value) -> Value {
    let Value::Object(object) = &value else { return value };
    let mut result = Args::new();
    copy_present(&mut result, object, &["id", "name", "shortUrl", "idList", "due", "start", "dueComplete", "dueReminder", "closed", "state", "color", "value", "date"]);
    if !result.contains_key("shortUrl") {
        copy_present(&mut result, object, &["url"]);
    }
    if let Some(items) = object.get("checkItems") {
        result.insert("checkItems".into(), Value::Array(compact_objects(items, &["id", "name", "state"])));
    }
    if result.is_empty() {
        return json!({"ok": true});
    }
    Value::Object(result)
}
```

- [ ] **Step 1: Write the failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp::test_support::*;
    use serde_json::json;

    fn args(value: Value) -> Args {
        value.as_object().unwrap().clone()
    }

    #[tokio::test]
    async fn workspace_restriction_blocks_card_mutation() {
        let h = test_server(&["allowed-workspace"], |request| match request.url.path() {
            "/cards/card-1" => json_response(json!({"id": "card-1", "idBoard": "board-1"})),
            "/boards/board-1" => json_response(json!({"id": "board-1", "idOrganization": "other-workspace", "name": "B"})),
            _ => not_found(),
        })
        .await;
        let err = h.server.handle("add_comment", &args(json!({"cardId": "card-1", "text": "hi"}))).await.unwrap_err();
        assert_eq!(err.to_string(), "board \"board-1\" belongs to workspace \"other-workspace\", which is not allowed");
        assert!(!calls(&h.api).await.iter().any(|call| call.starts_with("POST")));
    }

    #[tokio::test]
    async fn copy_card_uses_full_source_id_and_source_board() {
        let h = test_server(&[], |request| match (request.method.as_str(), request.url.path()) {
            ("GET", "/cards/abc12345") => json_response(json!({"id": "0123456789abcdef01234567", "idBoard": "board-1"})),
            ("GET", "/boards/board-1") => json_response(board_fixture()),
            ("POST", "/cards") => json_response(json!({"id": "copy-1", "name": "Copy"})),
            _ => not_found(),
        })
        .await;
        let result = h.server.handle("copy_card", &args(json!({"sourceCardId": "abc12345", "list": "Done", "keepFromSource": ["labels", "due"]}))).await.unwrap();
        assert!(matches!(result, Output::Json(ref v) if *v == json!({"id": "copy-1", "name": "Copy"})));
        let requests = h.api.received_requests().await.unwrap();
        let post = requests.iter().find(|r| r.method.as_str() == "POST").unwrap();
        let q = query(post);
        assert_eq!(q["idCardSource"], "0123456789abcdef01234567");
        assert_eq!(q["idList"], "list-done");
        assert_eq!(q["keepFromSource"], "labels,due");
    }

    #[test]
    fn acked_keeps_short_fields() {
        assert_eq!(acked(json!({"id": "x", "url": "u", "badges": {}})), json!({"id": "x", "url": "u"}));
        assert_eq!(acked(json!({"badges": {}})), json!({"ok": true}));
        assert_eq!(acked(json!([1])), json!([1]));
    }

    #[tokio::test]
    async fn unknown_tool_name_reports_not_implemented() {
        let h = test_server(&[], |_| not_found()).await;
        assert_eq!(h.server.handle("nope", &Args::new()).await.unwrap_err().to_string(), "tool \"nope\" is not implemented");
    }
}
```

Check both Go tests (handlers_test.go:210-232 and 306-326) for any assertion not copied here, and add it.

- [ ] **Step 2: Run to see it fail**

Run: `cargo test handlers`
Expected: compile FAIL.

- [ ] **Step 3: Implement `src/mcp/handlers.rs`**: port `handle` (handlers.go:12-184) arm by arm into `match name { ... }`. Every arm returns `Output::Json(...)` except `get_card`, which returns its `Output` directly. Notes:
  - `get_card`: `self.get_card(&text(args, "cardId"), GetCardOptions { comments_limit: integer(args, "commentsLimit", 10), delivery: text_default(args, "delivery", "auto"), ..Default::default() })`.
  - `list_workspaces`: `GET /members/me/organizations` `{"fields": "id,displayName"}`, then `filter_objects_by_workspace(result, "id")`.
  - `download_attachment`: `allowed_card(args, "cardId")`, then `client.download_attachment(&card_id, &text(args, "attachmentId"), &text(args, "destinationPath"))`.
  - `copy_card`: as Go. `map_args(args, &[("name", "name"), ("position", "pos")])`; `keepFromSource` only when it is an array; `join_values` uses strings as-is and `to_string()` otherwise.
  - `update_checklist_item`: `checked` maps to `state` `complete`/`incomplete`. A `member` key that is present (even empty) sets `idMember` to `""`, or to the resolved ID when non-empty.
  - Methods are `Method::POST/PUT/DELETE/GET`; paths use `escape`.
  - Port `allowed_card` (187-193), `join_values` (195-201), `workspace_id` (214-229, error from `client.require_workspace_allowed`), `write` (231-233 → `Ok(acked(self.client.call(method, path, params, body.as_ref()).await?))`) and `workspace_restrictions_enabled` (257-263).

- [ ] **Step 4: Run tests**

Run: `cargo test handlers`
Expected: 4 passed.

- [ ] **Step 5: Commit**

```bash
git add src/mcp
git commit -m "feat: port tool dispatch, acks, and workspace checks"
```

---

### Task 7: Card writes, custom fields, attachments, activity, checklists, health

**Files:**
- Create or replace stubs: `src/mcp/extras.rs`
- Reference: `internal/mcpserver/extras.go:21-500` and `:701-870`; handler tests at handlers_test.go:52-138, 164-208, 234-304, 346-427

**Interfaces:**
- Consumes: Tasks 3-6, `base64::engine::general_purpose::STANDARD`.
- Produces (the names Task 6 calls): `create_cards`, `update_card`, `set_custom_field`, `add_attachment`, `set_active(&self, board_id: &str, workspace_id: &str)`, `list_boards(&self, workspace_id: &str)`, `activity`, `find_checklist_items`, `create_checklist`, `health(&self) -> Result<Value, Error>`, `filter_objects_by_workspace(&self, value: Value, field: &str) -> Value`, `filter_by_workspace(&self, value: Value) -> Result<Value, Error>`; free fns `apply_set(&[String], &[String], &[String]) -> Vec<String>`, `custom_field_body(field: &Args, value: &str) -> Result<Value, Error>`, `link_attachments(&str) -> String`, `compact_actions(&Value) -> Vec<Value>`, `shorten(&Value) -> Value`.

- [ ] **Step 1: Write the failing tests**: port each Go test named below to `#[tokio::test]` (or `#[test]` for pure functions). Use the Task 4 harness. Keep every fake response and expected string from the Go test. Two are shown in full; the rest follow the same pattern. Each assertion that Go makes inside its handler becomes an assertion on `h.api.received_requests()` after the call.

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp::test_support::*;
    use serde_json::json;

    fn args(value: Value) -> Args {
        value.as_object().unwrap().clone()
    }

    #[tokio::test]
    async fn create_cards_resolves_names() {
        let h = test_server(&[], |request| match (request.method.as_str(), request.url.path()) {
            ("GET", "/boards/board-1") => json_response(board_fixture()),
            ("POST", "/cards") => json_response(json!({"id": "card-1", "name": "New card", "badges": {}})),
            _ => not_found(),
        })
        .await;
        let result = h.server.create_cards(&args(json!({
            "list": "to do", "boardId": "board-1",
            "cards": [{"name": "New card", "labels": ["bug", "blue"], "members": ["@joe"]}]
        })))
        .await
        .unwrap();
        assert_eq!(encode(&result), r#"[{"id":"card-1","name":"New card"}]"#);
        let requests = h.api.received_requests().await.unwrap();
        let q = query(requests.iter().find(|r| r.method.as_str() == "POST").unwrap());
        assert_eq!((q["idList"].as_str(), q["idLabels"].as_str(), q["idMembers"].as_str()), ("list-todo", "label-bug,label-blue", "member-joe"));
    }

    #[tokio::test]
    async fn update_card_moves_labels_and_sets_reminder() {
        let h = test_server(&[], |request| match (request.method.as_str(), request.url.path()) {
            ("GET", "/cards/card-1") => json_response(json!({"idBoard": "board-1", "idLabels": ["label-blue"], "idMembers": []})),
            ("GET", "/boards/board-1") => json_response(board_fixture()),
            ("PUT", "/cards/card-1") => json_response(json!({
                "id": "card-1", "name": "Card", "desc": "long description", "due": "2026-11-01T00:00:00.000Z",
                "dueReminder": 1440, "idList": "list-done", "shortUrl": "https://trello.com/c/abc", "url": "https://trello.com/c/abc/1-card"
            })),
            _ => not_found(),
        })
        .await;
        let result = h.server.update_card(&args(json!({
            "cardId": "card-1", "list": "Done", "due": "2026-11-01", "dueReminder": 1440.0, "addLabels": ["Bug", "blue"]
        })))
        .await
        .unwrap();
        assert_eq!(
            encode(&result),
            r#"{"due":"2026-11-01T00:00:00.000Z","dueReminder":1440,"id":"card-1","idList":"list-done","name":"Card","shortUrl":"https://trello.com/c/abc"}"#
        );
        assert_eq!(calls(&h.api).await, vec!["GET /cards/card-1", "GET /boards/board-1", "PUT /cards/card-1"]);
        let requests = h.api.received_requests().await.unwrap();
        let q = query(&requests[2]);
        assert_eq!(q["idList"], "list-done");
        assert_eq!(q["dueReminder"], "1440");
        assert_eq!(q["due"], "2026-11-01");
        assert_eq!(q["idLabels"], "label-blue,label-bug");
    }

    fn encode(value: &Value) -> String {
        crate::mcp::server::encode_result(value)
    }

    #[test]
    fn shorten_counts_characters() {
        let long = "é".repeat(130);
        let short = shorten(&json!(long));
        assert_eq!(short.as_str().unwrap().chars().count(), 121);
        assert!(short.as_str().unwrap().ends_with('…'));
        assert_eq!(shorten(&json!(5)), json!(5));
    }

    // Port the remaining Go tests by name, same fixtures and expectations:
    // update_card_unknown_name_writes_nothing      (handlers_test.go:120-138)
    // custom_field_body                            (164-192)
    // set_active_board_persists_selection          (194-208)  reads h.manager.file()
    // create_checklist_adds_items_and_returns_ack  (234-262)
    // link_attachments                             (264-287)
    // compact_actions                              (289-304)
    // set_custom_field_needs_value_or_clear        (346-362)
    // update_card_removing_last_label_uses_delete  (364-391)
    // compact_actions_reads_the_updated_entity     (393-407)
    // set_active_rejects_board_without_saving_workspace (409-427)
}
```

Write the ten listed tests in full before Step 2.

- [ ] **Step 2: Run to see it fail**

Run: `cargo test extras`
Expected: FAIL (stubs `todo!()`, or missing functions).

- [ ] **Step 3: Port extras.go into `src/mcp/extras.rs`**, function by function, keeping each message:
  - `create_cards` (21-62): resolve every card first, so a bad name creates nothing. Go ranges over two maps whose order is random; iterate deterministically with `[("name","name"),("description","desc"),("due","due"),("start","start"),("position","pos")]`, then labels before members. Errors: `cards must include at least one card`, `cards[{i}].{input}: {err}` and `cards[{i}] failed; cards already created: [{ids joined ", "}]: {err}`.
  - `update_card` (64-143) and `apply_set` (145-158): the clears are `DELETE /cards/{id}/{field}/{id}`. `nothing to update` when there are no params and no clears. With no params, the result is `{"id": card_id}`.
  - `set_custom_field` (160-187), `custom_field_body` (189-220): number check `value.parse::<f64>()`; error `{name} needs a number: {err}`. Rust's `ParseFloatError` text differs from Go's `strconv.ParseFloat` text; accepted. Checkbox accepts Go `ParseBool` inputs exactly: `1 t T TRUE true True 0 f F FALSE false False`, then sends `"true"`/`"false"`.
  - `add_attachment` (222-255): base64 via `STANDARD.decode`; the error is `decode data: {err}`.
  - `set_active` (257-292): validates both before saving either; saves through `self.client.manager().update(...)`.
  - `list_boards` (294-316), `activity` (318-348), `all_checklists` (350-385), `find_checklist_items` (387-444), `checklist_board_refs` (446-451), `create_checklist` (453-491), `health` (701-721; `latency_ms` from `Instant::elapsed().as_millis()`, `version` = `version::CURRENT`, `config_path` = manager path display), `filter_objects_by_workspace` (723-740), `filter_by_workspace` (742-765), `compact_actions` (816-850; the key/path list is fixed, so iteration order doesn't matter), `shorten` (852-858; count `chars()`, keep 120, append `…`).
  - `link_attachments` (792-814) without a regex crate:

```rust
/// Turns bare attachment URLs into [file name](url) links. Only whole words
/// outside code spans change, so URLs already wrapped in Markdown stay as written.
pub(crate) fn link_attachments(text: &str) -> String {
    text.split('`')
        .enumerate()
        .map(|(index, part)| if index % 2 == 0 { link_words(part) } else { part.to_string() })
        .collect::<Vec<_>>()
        .join("`")
}

/// Applies link_attachment to each run of non-space characters. RE2's \s is
/// exactly [\t\n\f\r ].
fn link_words(part: &str) -> String {
    let mut out = String::with_capacity(part.len());
    let mut word = String::new();
    for ch in part.chars() {
        if matches!(ch, ' ' | '\t' | '\n' | '\x0C' | '\r') {
            if !word.is_empty() {
                out.push_str(&link_attachment(&word));
                word.clear();
            }
            out.push(ch);
        } else {
            word.push(ch);
        }
    }
    if !word.is_empty() {
        out.push_str(&link_attachment(&word));
    }
    out
}

fn link_attachment(word: &str) -> String {
    let link = word.trim_end_matches(['.', ',', ';', ':', '!', '?', ')']);
    let Some(file) = attachment_file(link) else { return word.to_string() };
    if file.contains("://") {
        return word.to_string();
    }
    let name = crate::util::path_unescape(file).unwrap_or_else(|_| file.to_string());
    let target = link.replace('(', "%28").replace(')', "%29");
    format!("[{name}]({target}){}", &word[link.len()..])
}

/// Matches ^https://trello\.com/1/cards/\w+/attachments/\w+/download/(\S+)$
/// and returns the captured file part.
fn attachment_file(link: &str) -> Option<&str> {
    let rest = link.strip_prefix("https://trello.com/1/cards/")?;
    let (card, rest) = rest.split_once("/attachments/")?;
    let (attachment, file) = rest.split_once("/download/")?;
    let word = |part: &str| !part.is_empty() && part.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    (word(card) && word(attachment) && !file.is_empty()).then_some(file)
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test extras`
Expected: 13 passed.

- [ ] **Step 5: Commit**

```bash
git add src/mcp/extras.rs
git commit -m "feat: port card writes, checklists, attachments, and activity"
```

---

### Task 8: get_card: reference parsing, Markdown, file delivery

**Files:**
- Create or replace stubs: `src/mcp/card.rs`
- Reference: `internal/mcpserver/extras.go:493-700, 873-990`; `internal/mcpserver/get_card_test.go`

**Interfaces:**
- Consumes: Tasks 3-6.
- Produces: `#[derive(Default)] pub(crate) struct GetCardOptions { pub comments_limit: i64, pub delivery: String, pub output_dir: Option<PathBuf>, pub file_threshold: usize }`; `impl Server { pub(crate) async fn get_card(&self, card_ref: &str, options: GetCardOptions) -> Result<Output, Error> }`; `pub(crate) fn normalize_card_ref(value: &str) -> Result<String, Error>`; `pub(crate) fn render_card_markdown(card: &Args, field_definitions: &[Args], comments_limit: i64) -> String`; `pub(crate) fn deliver_card(card_id: &str, markdown: &str, options: &GetCardOptions) -> Result<Output, Error>`; `fn write_card_output(card_id: &str, format: &str, payload: &[u8], output_dir: Option<&Path>) -> Result<PathBuf, Error>`; `fn safe_file_part(&str) -> String`; `fn utc_stamp(SystemTime) -> String`; `fn format_minutes(f64) -> String`.

- [ ] **Step 1: Write the failing tests**: port all eight tests from `get_card_test.go` by name (`normalize_card_ref`, `normalize_card_ref_rejects_non_card_urls`, `get_card_compact_uses_one_direct_request`, `get_card_with_workspace_restriction_keeps_authorization_preflight`, `get_card_defaults_to_markdown_auto_inline`, `deliver_card_modes`, `get_card_rejects_invalid_options_before_calling_trello`, `get_card_names_checklist_assignees_from_the_board`). Keep their fixtures, wanted substrings, dropped substrings, and request counts. `rawTextResult` becomes `Output::Text`; a file result becomes `Output::Json` with `path`, `bytes`, `instruction`. Add these new tests:

```rust
#[test]
fn utc_stamp_formats_like_go() {
    use std::time::{Duration, UNIX_EPOCH};
    assert_eq!(utc_stamp(UNIX_EPOCH), "19700101T000000Z");
    // 2000-03-01 is day 11017; 13:14:15 is 47655 s.
    assert_eq!(utc_stamp(UNIX_EPOCH + Duration::from_secs(11017 * 86_400 + 47_655)), "20000301T131415Z");
    assert_eq!(utc_stamp(UNIX_EPOCH + Duration::from_secs(19_723 * 86_400)), "20240101T000000Z");
}

#[test]
fn format_minutes_matches_go_percent_g() {
    assert_eq!(format_minutes(1440.0), "1440");
    assert_eq!(format_minutes(0.0), "0");
    assert_eq!(format_minutes(90.5), "90.5");
    assert_eq!(format_minutes(1_000_000.0), "1e+06");
}

#[test]
fn safe_file_part_replaces_unsafe_characters() {
    assert_eq!(safe_file_part("abc/../x y"), "abc----x-y");
    assert_eq!(safe_file_part("///"), "card");
}

#[cfg(unix)]
#[test]
fn delivered_file_is_private() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let options = GetCardOptions { delivery: "file".into(), output_dir: Some(dir.path().join("outputs")), file_threshold: 1, comments_limit: 10 };
    let Output::Json(result) = deliver_card("card-1", "# Card", &options).unwrap() else { panic!("expected a file result") };
    let path = std::path::Path::new(result["path"].as_str().unwrap());
    assert_eq!(std::fs::metadata(path).unwrap().permissions().mode() & 0o777, 0o600);
    assert_eq!(std::fs::metadata(path.parent().unwrap()).unwrap().permissions().mode() & 0o777, 0o700);
    assert_eq!(std::fs::read_to_string(path).unwrap(), "# Card\n");
}
```

- [ ] **Step 2: Run to see it fail**

Run: `cargo test card`
Expected: FAIL.

- [ ] **Step 3: Implement `src/mcp/card.rs`**
  - `GetCardOptions::normalized` (493-508): `commentsLimit must be between 0 and 100`; delivery trimmed and lowercased, empty means `auto`; `delivery must be inline, file, or auto`; `file_threshold == 0` means `16 * 1024`.
  - `get_card` (510-552), `card_request_params` (554-570), `deliver_card` (572-587: inline when `inline`, or `auto` and `markdown.len() <= threshold`; else `{"bytes", "instruction": "The card is too large to return inline; read the Markdown file at path.", "path"}`), `render_card_markdown` (589-682), `custom_field_text` (684-699).
  - The due line uses `format_minutes(reminder)` for Go `%g`:

```rust
/// Go's %g for the reminder minutes: plain digits below 1e6, else Go's
/// exponent form. shortcut: exotic fractions may differ in the last digit from
/// Go; reminders are whole minutes, so this has not mattered.
fn format_minutes(value: f64) -> String {
    if value.abs() < 1e6 {
        return value.to_string();
    }
    let formatted = format!("{value:e}"); // "1e6"
    match formatted.split_once('e') {
        Some((mantissa, exponent)) => {
            let exponent: i32 = exponent.parse().unwrap_or(0);
            format!("{mantissa}e{}{:02}", if exponent < 0 { '-' } else { '+' }, exponent.abs())
        }
        None => formatted,
    }
}
```

  - `normalize_card_ref` (873-904), using `reqwest::Url`:

```rust
pub(crate) fn normalize_card_ref(value: &str) -> Result<String, Error> {
    let mut value = value.trim().to_string();
    if value.is_empty() {
        return Err(msg("card reference is required"));
    }
    let lower = value.to_lowercase();
    if lower.starts_with("trello.com/") || lower.starts_with("www.trello.com/") {
        value = format!("https://{value}");
    }
    if let Ok(parsed) = reqwest::Url::parse(&value)
        && let Some(host) = parsed.host_str().filter(|host| !host.is_empty())
    {
        if parsed.scheme() != "http" && parsed.scheme() != "https" {
            return Err(msg("Trello card URL must use http or https"));
        }
        let host = host.to_lowercase();
        if host != "trello.com" && host != "www.trello.com" {
            return Err(msg(format!("unsupported Trello card URL host {host:?}")));
        }
        let parts: Vec<&str> = parsed.path().trim_matches('/').split('/').collect();
        if parts.len() < 2 || !parts[0].eq_ignore_ascii_case("c") {
            return Err(msg("Trello card URL must have the form trello.com/c/{shortLink}"));
        }
        value = crate::util::path_unescape(parts[1]).map_err(|err| msg(format!("decode Trello short link: {err}")))?;
    }
    if value.contains(['/', '?', '#']) {
        return Err(msg("card reference must be a card ID, short link, or Trello card URL"));
    }
    Ok(value)
}
```

  - `write_card_output` (906-967): default dir is `dirs::cache_dir()` + `trello-mcp/outputs`; a missing cache dir gives `resolve Trello MCP cache directory`. `create_dir_all`, then `util::set_mode(dir, 0o700)`. Temp file `tempfile::Builder::new().prefix(".card-").suffix(".tmp").tempfile_in(dir)`, mode `0o600`, write payload + `\n`, `sync_all`. The random part is the temp file name without the `.card-` prefix and `.tmp` suffix. Final name: `card-{safe_file_part(card_id)}-{utc_stamp(now)}-{random}.{md|json}`; `persist`, `set_mode(0o600)`, return `util::absolute(path)`.
  - `safe_file_part` (969-990): keep `[A-Za-z0-9_-]`, map every other character to `-`, trim `-`, empty means `card`. Go's 48-character cap never fires (its `break` only leaves the `switch`), so do not truncate.
  - `utc_stamp`:

```rust
/// Go's "20060102T150405Z" layout, in UTC, without a date crate.
fn utc_stamp(time: std::time::SystemTime) -> String {
    let seconds = time.duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    let (days, rest) = (seconds.div_euclid(86_400), seconds.rem_euclid(86_400));
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}{month:02}{day:02}T{:02}{:02}{:02}Z", rest / 3600, rest % 3600 / 60, rest % 60)
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test card`
Expected: 12 passed.

- [ ] **Step 5: Commit**

```bash
git add src/mcp/card.rs
git commit -m "feat: port get_card Markdown rendering and file delivery"
```

---

### Task 9: Client config rendering and project setup

**Files:**
- Create: `src/clientconfig/mod.rs`, `src/clientconfig/project.rs`
- Modify: `src/main.rs` (`mod clientconfig;`)
- Reference: `internal/clientconfig/*.go` and their tests

**Interfaces:**
- Consumes: `util::{absolute, write_atomic}`.
- Produces: `pub const SUPPORTED_CLIENTS: [&str; 5]`, `pub const PROJECT_CLIENTS: [&str; 4]`; `pub struct Options { pub command: String, pub config_path: String }`; `#[derive(Debug, thiserror::Error)] pub enum Error { #[error("{0}")] Invalid(String), #[error("{context}: {source}")] Io { context: String, source: std::io::Error } }`; `pub fn render(client: &str, options: &Options) -> Result<String, Error>`; `pub fn project_config_path(client: &str) -> Result<&'static str, Error>`; `pub fn setup_project(client: &str, project_dir: &str, options: &Options) -> Result<PathBuf, Error>`.

JSON output keeps Go's struct field order, so render with `#[derive(Serialize)]` structs, not `json!` (which sorts keys):

```rust
#[derive(Serialize)]
struct JsonConfig<'a> {
    #[serde(rename = "mcpServers")]
    mcp_servers: BTreeMap<&'static str, StdioServer<'a>>,
}

#[derive(Serialize)]
struct StdioServer<'a> {
    #[serde(rename = "type", skip_serializing_if = "str::is_empty")]
    kind: &'a str,
    command: &'a str,
    args: &'a [String],
}

#[derive(Serialize)]
struct OpenCodeConfig<'a> {
    #[serde(rename = "$schema")]
    schema: &'static str,
    mcp: BTreeMap<&'static str, OpenCodeServer>,
}

#[derive(Serialize)]
struct OpenCodeServer {
    #[serde(rename = "type")]
    kind: &'static str,
    command: Vec<String>,
    enabled: bool,
}
```

Use `serde_json::to_string_pretty` (2-space indent, as Go `MarshalIndent`) + `"\n"`. Codex `quote(value)` is `serde_json::to_string(value)`.

- [ ] **Step 1: Write the failing tests**: port all 11 Go tests by name (`render_codex`, `render_uses_default_config_without_argument`, `render_json_clients`, `render_aliases`, `render_open_code`, `render_rejects_unsupported_client`, `project_config_paths`, `setup_project_json_clients`, `setup_project_preserves_existing_json`, `setup_project_updates_managed_codex_block`, `setup_project_rejects_unmanaged_codex_section`). Add the fixture gate:

```rust
#[test]
fn render_matches_go_fixtures() {
    for client in SUPPORTED_CLIENTS {
        for (suffix, config_path) in [("", ""), (".config", "/x/config.json")] {
            let want = std::fs::read_to_string(format!("{}/tests/fixtures/client-config/{client}{suffix}.out", env!("CARGO_MANIFEST_DIR"))).unwrap();
            let options = Options { command: "/x/trello-mcp".into(), config_path: config_path.into() };
            assert_eq!(render(client, &options).unwrap(), want, "{client}{suffix}");
        }
    }
}
```

- [ ] **Step 2: Run to see it fail**

Run: `cargo test clientconfig`
Expected: compile FAIL.

- [ ] **Step 3: Implement**: port clientconfig.go and project.go function by function, keeping every message, including `unsupported client {client:?} (choose codex, claude-code, claude-desktop, antigravity, opencode)`, `server command is required`, the Claude Desktop project error, `existing Codex trello-mcp managed block is incomplete`, the `{target}:{line}: existing un-managed [mcp_servers.trello] section found; remove it or use client-config codex to merge manually` message, `parse existing {client} project config: {err}`, and `existing {client} config field "{key}" is not an object`.
  - `merge_json` parses into `serde_json::Map` (sorted keys, as Go maps). Output is `to_string_pretty` + `"\n"`.
  - Project file writes use `util::write_atomic(path, data, mode, ".trello-mcp-config-")`: `mode` is the existing file's permission bits on Unix, else `0o644`. Parent dirs come from `create_dir_all`.
  - `setup_project` resolves `project_dir` (empty means `"."`) with `util::absolute`.

- [ ] **Step 4: Run tests**

Run: `cargo test clientconfig`
Expected: 12 passed.

- [ ] **Step 5: Commit**

```bash
git add src/clientconfig src/main.rs
git commit -m "feat: port client config rendering and project setup"
```

---

### Task 10: CLI, serve, update

**Files:**
- Replace: `src/main.rs`
- Reference: `cmd/trello-mcp/main.go`, `cmd/trello-mcp/main_test.go`

**Interfaces:**
- Consumes: everything above.
- Produces: `fn run(args: Vec<String>, stdout: &mut dyn Write, stderr: &mut dyn Write) -> anyhow::Result<()>`, `fn source_root(executable: &Path) -> anyhow::Result<PathBuf>`, `fn mask(value: &str) -> String`, clap types `Cli`, `Command`, `ConfigCommand`, `SetArgs`.

- [ ] **Step 1: Write the failing tests** (ports of main_test.go plus the Review Focus tests)

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn run_ok(args: &[&str]) -> (String, String) {
        let (mut out, mut err) = (Vec::new(), Vec::new());
        run(args.iter().map(|a| a.to_string()).collect(), &mut out, &mut err).unwrap_or_else(|e| panic!("run {args:?}: {e:#}"));
        (String::from_utf8(out).unwrap(), String::from_utf8(err).unwrap())
    }

    fn run_err(args: &[&str]) -> String {
        let (mut out, mut err) = (Vec::new(), Vec::new());
        format!("{:#}", run(args.iter().map(|a| a.to_string()).collect(), &mut out, &mut err).unwrap_err())
    }

    #[test]
    fn help_lists_commands() {
        let (out, err) = run_ok(&["--help"]);
        for want in ["client-config", "setup", "--config", "Run the Trello MCP server"] {
            assert!(out.contains(want), "help missing {want:?}:\n{out}");
        }
        assert!(err.is_empty());
    }

    #[test]
    fn version_forms() {
        for args in [&["version"][..], &["--version"], &["-version"], &["-v"]] {
            assert_eq!(run_ok(args), (format!("{}\n", version::CURRENT), String::new()), "{args:?}");
        }
    }

    #[test]
    fn config_flag_parses_before_and_after_subcommand() {
        for args in [&["trello-mcp", "serve", "--config", "/p.json"][..], &["trello-mcp", "--config", "/p.json", "serve"]] {
            let cli = Cli::try_parse_from(args).unwrap();
            assert_eq!(cli.config.as_deref(), Some("/p.json"), "{args:?}");
            assert!(matches!(cli.command, Some(Command::Serve)));
        }
    }

    #[test]
    fn persistent_config_flag_before_subcommands() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        let (out, _) = run_ok(&["--config", path.to_str().unwrap(), "config", "init"]);
        assert_eq!(out.trim(), path.to_str().unwrap());
        config::load(&path).unwrap();
    }

    #[test]
    fn config_set_persists_all_values() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        run_ok(&[
            "config", "set", "--config", path.to_str().unwrap(),
            "--api-key", "api-key", "--token", "api-token", "--board-id", "board", "--workspace-id", "workspace",
            "--allowed-workspaces", "workspace,second", "--api-base-url", "https://example.test/1/",
            "--timeout-seconds", "45", "--max-upload-bytes", "2048",
        ]);
        let cfg = config::load(&path).unwrap();
        assert_eq!((cfg.api_key.as_str(), cfg.token.as_str(), cfg.default_board_id.as_str(), cfg.workspace_id.as_str()), ("api-key", "api-token", "board", "workspace"));
        assert_eq!((cfg.api_base_url.as_str(), cfg.request_timeout_seconds, cfg.max_upload_bytes), ("https://example.test/1", 45, 2048));
        assert_eq!(cfg.allowed_workspace_ids, vec!["workspace", "second"]);
    }

    #[test]
    fn config_show_masks_secrets() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        let mut cfg = config::Config::default();
        cfg.api_key = "abcdefghijk".into();
        cfg.token = "super-secret-token".into();
        config::save(&path, &cfg).unwrap();
        let (out, _) = run_ok(&["config", "show", "--config", path.to_str().unwrap()]);
        assert!(!out.contains("abcdefghijk") && !out.contains("super-secret-token"), "{out}");
        assert!(out.contains("\"api_key\": \"abcd...hijk\""), "{out}");
    }

    #[test]
    fn mask_handles_multibyte() {
        assert_eq!(mask(""), "");
        assert_eq!(mask("short"), "********");
        assert_eq!(mask("ééééééééé"), "éééé...éééé");
    }

    #[test]
    fn config_without_subcommand_errors() {
        assert_eq!(run_err(&["config"]), "config subcommand is required");
    }

    #[test]
    fn setup_all_rejects_client_argument() {
        assert!(run_err(&["setup", "codex", "--all"]).contains("--all cannot be combined with a client"));
    }

    #[test]
    fn source_root_requires_rust_checkout() {
        let root = tempfile::tempdir().unwrap();
        let executable = root.path().join("bin").join("trello-mcp");
        assert!(source_root(&executable).is_err());
        std::fs::write(root.path().join("Cargo.toml"), "[package]\nname = \"trello-mcp\"\n").unwrap();
        assert_eq!(source_root(&executable).unwrap(), root.path());
    }

    // Port by name from main_test.go, same arguments and assertions:
    // client_config_command (30-119), client_config_command_uses_server_default_config (121-136),
    // setup_project_command (138-164), setup_all_project_clients_command (166-204)
}
```

- [ ] **Step 2: Run to see it fail**

Run: `cargo test --bin trello-mcp tests::`
Expected: compile FAIL.

- [ ] **Step 3: Implement `src/main.rs`**

```rust
mod clientconfig;
mod config;
mod mcp;
mod trello;
mod util;
mod version;

use std::{
    io::Write,
    path::{Path, PathBuf},
    process::ExitCode,
    sync::Arc,
};

use anyhow::{Context, bail};
use clap::{CommandFactory, Parser, Subcommand, error::ErrorKind};

#[derive(Parser)]
#[command(name = "trello-mcp", about = "Run the Trello MCP server", disable_version_flag = true)]
struct Cli {
    /// path to config file
    #[arg(long, global = true, value_name = "string")]
    config: Option<String>,
    /// version for trello-mcp
    #[arg(short = 'v', long)]
    version: bool,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Run the MCP server over stdio
    Serve,
    /// Create, edit, show, and validate persistent configuration
    Config {
        #[command(subcommand)]
        command: Option<ConfigCommand>,
    },
    /// Generate MCP configuration for an AI coding client
    ClientConfig {
        client: String,
        /// path clients should use to start trello-mcp
        #[arg(long)]
        command: Option<String>,
    },
    /// Configure trello-mcp in an AI coding client
    Setup {
        client: Option<String>,
        /// configure every project-scoped AI coding client
        #[arg(long)]
        all: bool,
        /// configuration scope (project)
        #[arg(long, default_value = "project")]
        scope: String,
        /// project root to configure
        #[arg(long, default_value = ".")]
        project_dir: String,
        /// path clients should use to start trello-mcp
        #[arg(long)]
        command: Option<String>,
    },
    /// Pull the latest source and rebuild this binary
    Update {
        /// rebuild the current checkout without pulling
        #[arg(long)]
        no_pull: bool,
    },
    /// Print the version
    Version,
}

#[derive(Subcommand)]
enum ConfigCommand {
    /// Create the config file with defaults
    Init,
    /// Persist one or more configuration values
    Set(SetArgs),
    /// Print configuration (credentials are masked by default)
    Show {
        /// show API key and token
        #[arg(long)]
        reveal_secrets: bool,
        /// include environment overrides
        #[arg(long)]
        effective: bool,
    },
    /// Validate effective file and environment configuration
    Validate {
        /// validate structure without requiring credentials
        #[arg(long)]
        without_credentials: bool,
    },
    /// Print the resolved config path
    Path,
}

#[derive(clap::Args)]
struct SetArgs {
    /// Trello API key
    #[arg(long)]
    api_key: Option<String>,
    /// Trello API token
    #[arg(long)]
    token: Option<String>,
    /// default/active board ID
    #[arg(long)]
    board_id: Option<String>,
    /// active workspace ID
    #[arg(long)]
    workspace_id: Option<String>,
    /// comma-separated allowed workspace IDs
    #[arg(long)]
    allowed_workspaces: Option<String>,
    /// Trello API base URL
    #[arg(long)]
    api_base_url: Option<String>,
    /// HTTP request timeout in seconds
    #[arg(long)]
    timeout_seconds: Option<i64>,
    /// maximum upload/download size
    #[arg(long)]
    max_upload_bytes: Option<i64>,
    /// remove API key and token from the config file
    #[arg(long)]
    clear_credentials: bool,
    /// clear the active board
    #[arg(long)]
    clear_board: bool,
    /// clear the active workspace
    #[arg(long)]
    clear_workspace: bool,
    /// remove the workspace allow-list
    #[arg(long)]
    clear_allowed_workspaces: bool,
}

fn main() -> ExitCode {
    let args = std::env::args().skip(1).collect();
    match run(args, &mut std::io::stdout(), &mut std::io::stderr()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("trello-mcp: {err:#}");
            ExitCode::from(1)
        }
    }
}

fn run(mut args: Vec<String>, stdout: &mut dyn Write, stderr: &mut dyn Write) -> anyhow::Result<()> {
    // Keep the legacy single-dash version flag.
    if args.first().is_some_and(|arg| arg == "-version") {
        args[0] = "--version".into();
    }
    let cli = match Cli::try_parse_from(std::iter::once("trello-mcp".to_string()).chain(args)) {
        Ok(cli) => cli,
        Err(err) if matches!(err.kind(), ErrorKind::DisplayHelp | ErrorKind::DisplayVersion) => {
            write!(stdout, "{}", err.render())?;
            return Ok(());
        }
        Err(err) => bail!("{}", err.render().to_string().trim_end()),
    };
    if cli.version {
        writeln!(stdout, "{}", version::CURRENT)?;
        return Ok(());
    }
    let config_path = cli.config.unwrap_or_default();
    match cli.command {
        None | Some(Command::Serve) => serve(&config_path),
        Some(Command::Version) => Ok(writeln!(stdout, "{}", version::CURRENT)?),
        Some(Command::Config { command }) => run_config(command, &config_path, stdout, stderr),
        Some(Command::ClientConfig { client, command }) => {
            let options = resolve_client_options(command.as_deref(), &config_path)?;
            Ok(write!(stdout, "{}", clientconfig::render(&client, &options)?)?)
        }
        Some(Command::Setup { client, all, scope, project_dir, command }) => {
            if all && client.is_some() {
                bail!("--all cannot be combined with a client");
            }
            if scope != "project" {
                bail!("unsupported setup scope {scope:?} (choose project)");
            }
            let clients: Vec<String> = match (all, client) {
                (true, _) => clientconfig::PROJECT_CLIENTS.iter().map(|c| c.to_string()).collect(),
                (false, Some(client)) => vec![client],
                (false, None) => bail!("setup requires a client or --all"),
            };
            let options = resolve_client_options(command.as_deref(), &config_path)?;
            for client in clients {
                let target = clientconfig::setup_project(&client, &project_dir, &options)?;
                writeln!(stdout, "{}", target.display())?;
            }
            Ok(())
        }
        Some(Command::Update { no_pull }) => update(no_pull, stdout),
    }
}
```

Then port the remaining pieces:
- `run_config`. When `command` is `None`, write `Cli::command().find_subcommand_mut("config")` help (`render_help()`) to `stderr` and `bail!("config subcommand is required")`.
- `init`: `config::save(manager.path(), &manager.file())` and print the path.
- `set`: apply the clears first, then every `Some(...)` flag (`allowed_workspaces` through `config::parse_workspace_ids`), then `cfg.validate(false)`, all inside `manager.update`; print the path.
- `show`: `file()`, or `effective()` with `--effective`. Mask unless `--reveal-secrets`. Print `to_string_pretty` + newline.
- `validate`: print `ok`.
- `path`: print `config::resolve_path(&config_path)`.
- `resolve_client_options` (main.go:390-410): `std::env::current_exe()`, `util::absolute`, and the config resolved only when non-empty.
- `mask` (main.go:477-485), counted in `chars()`.
- `serve`: build a `tokio::runtime::Runtime`. Load the manager and validate the effective config with credentials, before printing anything. Then `mcp::Server::new(Arc::new(trello::Client::new(manager))).serve(rmcp::transport::stdio()).await?` (bring `rmcp::ServiceExt` into scope). `tokio::select!` between `service.waiting()` and a shutdown future that waits for Ctrl-C or Unix SIGTERM (`tokio::signal::unix::signal(SignalKind::terminate())` under `#[cfg(unix)]`; Ctrl-C only on other platforms).
- `update(no_pull, stdout)`:
  1. Resolve the executable with `std::env::current_exe()` and canonicalize it.
  2. Find the checkout with `source_root`.
  3. Unless `--no-pull`, run `git pull --ff-only` with `Stdio::inherit()`. On failure, `bail!("git pull --ff-only in {}: {status}; use --no-pull to rebuild the current checkout", root.display())`.
  4. Run `cargo build --release --locked` with inherited stdio. On failure, `bail!("cargo build: {status}")`.
  5. Read `root/target/release/trello-mcp{EXE_SUFFIX}` and `util::write_atomic(&executable, &bytes, 0o755, ".trello-mcp-")`. The rename gives a new inode, so the running binary and macOS code signing stay intact.
  6. Run `executable version`. Print `updated {executable} to {version_output}`.
- `source_root(executable)`: grandparent dir. Accept it only if `root/Cargo.toml` starts with `[package]\nname = "trello-mcp"\n`. Otherwise `bail!("{} is not in a trello-mcp source checkout's bin/; reinstall with `cargo install --git https://github.com/thaitanloi365/trello-mcp` or `brew upgrade trello-mcp`", executable.display())`.

- [ ] **Step 4: Run every check**

Run: `cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test`
Expected: all pass, no warnings. Fix any clippy findings in the module that raised them.

- [ ] **Step 5: Commit**

```bash
git add src/main.rs
git commit -m "feat: port CLI, serve, and update to Rust"
```

---

### Task 11: Remove Go, update build, release and docs, switch the installed binary

**Files:**
- Delete: `cmd/`, `internal/`, `go.mod`, `go.sum`
- Modify: `scripts/build-release`, `scripts/trello-mcp-dev`, `Formula/trello-mcp.rb`, `README.md`, `docs/installation.md`, `CHANGELOG.md`

**Interfaces:**
- Consumes: the finished binary.
- Produces: the release-ready repo and a Rust `bin/trello-mcp`.

- [ ] **Step 1: Delete the Go code**

```bash
git rm -r -q cmd internal go.mod go.sum
cargo test
```

Expected: all tests still pass; nothing depended on the Go files.

- [ ] **Step 2: Rewrite `scripts/trello-mcp-dev`**

```sh
#!/bin/sh
set -eu

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
project_dir=$(dirname "$script_dir")

cd "$project_dir"
exec cargo run --quiet --release -- "$@"
```

- [ ] **Step 3: Rewrite `scripts/build-release`**: keep the argument parsing, version-character check, output-dir handling and "already exists" guard (lines 1-31 of the current script). Replace the rest with:

```sh
actual_version=$(cargo run --quiet --release -- version)
if [ "$actual_version" != "$release_version" ]; then
  echo "binary version is $actual_version, expected $release_version" >&2
  exit 1
fi

temporary_dir=$(mktemp -d "${TMPDIR:-/tmp}/trello-mcp-release.XXXXXX")
trap 'rm -rf "$temporary_dir"' EXIT HUP INT TERM

mkdir -p "$output_dir"

# macOS only: Linux and Windows install from source with cargo install --git.
build_archive() {
  rust_target=$1
  archive_arch=$2
  base_name="trello-mcp_${release_version}_darwin_${archive_arch}"
  package_dir="$temporary_dir/$base_name"
  rustup target add "$rust_target" >/dev/null
  cargo build --release --locked --target "$rust_target"
  mkdir -p "$package_dir"
  cp "target/$rust_target/release/trello-mcp" LICENSE README.md "$package_dir/"
  tar -C "$temporary_dir" -czf "$output_dir/$base_name.tar.gz" "$base_name"
}

build_archive aarch64-apple-darwin arm64
build_archive x86_64-apple-darwin amd64

(
  cd "$output_dir"
  shasum -a 256 trello-mcp_"$release_version"_* > checksums.txt
)

echo "$output_dir"
```

- [ ] **Step 4: Rewrite `Formula/trello-mcp.rb`**

```ruby
class TrelloMcp < Formula
  desc "Model Context Protocol server for Trello"
  homepage "https://github.com/thaitanloi365/trello-mcp"
  license "MIT"
  head "https://github.com/thaitanloi365/trello-mcp.git", branch: "master"

  depends_on "rust" => :build

  def install
    system "cargo", "install", *std_cargo_args
  end

  test do
    assert_match "2.0.0", shell_output("#{bin}/trello-mcp version")
    assert_match "Run the Trello MCP server", shell_output("#{bin}/trello-mcp --help")
  end
end
```

- [ ] **Step 5: Update `CHANGELOG.md`**: rename `## [Unreleased]` to `## [2.0.0] - 2026-10-12`. Insert this at the top of that section, above the existing tool-redesign notes:

```markdown
Rewritten in Rust. Tools, the `draft_reply` prompt, CLI commands and flags,
and the config file are unchanged.

Upgrading from 1.x: the Go binary's `update` cannot build the Rust source.
Run once from the checkout:
`cargo build --release --locked && cp target/release/trello-mcp bin/trello-mcp`.

### Rust rewrite

- Built with Rust (rmcp, tokio, clap, reqwest). Install with
  `cargo install --git https://github.com/thaitanloi365/trello-mcp`;
  `go install` no longer works.
- `trello-mcp update` rebuilds with `cargo build --release`.
- The MCP server name is `trello-mcp` (was `trello-mcp-go`), and the HTTP
  User-Agent is `trello-mcp/2.0.0`.
- Release archives cover macOS arm64 and amd64. Linux and Windows build
  from source.
- Removed the `completion` command that Cobra generated.
- Network errors no longer include the request URL, which carried the API
  key and token.
```

- [ ] **Step 6: Update `README.md` and `docs/installation.md`**
  - README title: `# Trello MCP Server in Rust`. The first paragraph says "A Rust implementation".
  - README Install: `cargo install --git https://github.com/thaitanloi365/trello-mcp --locked` (Rust 1.88 or newer) replaces `go install`.
  - README zsh section: "Always run the latest source; no manual cargo build is needed." The update paragraph says it rebuilds `bin/trello-mcp` with `cargo build --release`.
  - README Development: replace the Go commands with `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test`, `cargo build --release`.
  - README Security notes: fix the stale tool name `attach_file_to_card` to `add_attachment`.
  - installation.md: Go 1.26.5 becomes Rust 1.88. The release table lists only the two macOS archives. Replace the `go install` section with `cargo install --git`. The source checkout builds with `cargo build --release --locked && cp target/release/trello-mcp bin/trello-mcp`. The Homebrew text says it builds with Rust.
  - Run `grep -rn -i '\bgo \|golang\|go install\|go build\|go 1\.' README.md docs/installation.md`. Expected: no remaining Go references, except the history in CHANGELOG.md.

- [ ] **Step 7: Rebuild `bin/trello-mcp` from Rust** (the "rebuild bin" Review Focus step)

```bash
cargo build --release --locked && cp target/release/trello-mcp bin/trello-mcp
bin/trello-mcp version
```

Expected: `2.0.0`.

- [ ] **Step 8: Stdio smoke test** (the "stdio smoke" Review Focus step: every stdout line must be JSON)

```bash
{ printf '%s\n' \
  '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"smoke","version":"1"}}}' \
  '{"jsonrpc":"2.0","method":"notifications/initialized"}' \
  '{"jsonrpc":"2.0","id":2,"method":"tools/list"}' \
  '{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"get_health","arguments":{}}}'; sleep 3; } \
| bin/trello-mcp serve > /tmp/trello-smoke.jsonl
jq -c 'select(.id==1).result.serverInfo, (select(.id==2).result.tools|length), (select(.id==3).result.content[0].text|fromjson|.ok)' /tmp/trello-smoke.jsonl
```

Expected: `{"name":"trello-mcp","version":"2.0.0"}`, then `28`, then `true`. This uses the real `/Users/loithai/.trello-mcp/config.json`, and `get_health` is read-only. If jq reports a parse error, something wrote non-JSON to stdout; find it and fix it.

- [ ] **Step 9: Manual client check**: ask the user to restart one Claude Code project that uses trello-mcp, then run `get_card` and `list_cards` against a real card. Read-only.

- [ ] **Step 10: Final checks and commit**

```bash
cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test
sh -n scripts/build-release scripts/trello-mcp-dev
git add -A
git commit -m "chore!: replace Go implementation with Rust 2.0.0"
```

`bin/` is gitignored, so the rebuilt binary is not committed.
