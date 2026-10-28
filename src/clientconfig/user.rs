//! User-scoped (home directory) client configuration. Files are edited in
//! place: only our own `trello` entry is added or replaced.

use std::path::{Path, PathBuf};

use super::project::apply;
use super::{ANTIGRAVITY, CLAUDE_CODE, CODEX, Error, OPENCODE, Options, normalize_client};
use crate::config::Env;

/// Env var `name` as a directory, when set and non-empty.
fn env_dir(env: &Env, name: &str) -> Option<PathBuf> {
    env(name)
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

/// Returns the client's user-level MCP config path. Relocated config homes
/// win: `$CODEX_HOME`, `$CLAUDE_CONFIG_DIR`, `$XDG_CONFIG_HOME`.
/// Claude Desktop has no CLI-managed user config here; use `client-config`.
pub fn user_config_path(client: &str, home: &Path, env: &Env) -> Result<PathBuf, Error> {
    Ok(match normalize_client(client)? {
        CODEX => env_dir(env, "CODEX_HOME")
            .unwrap_or_else(|| home.join(".codex"))
            .join("config.toml"),
        CLAUDE_CODE => env_dir(env, "CLAUDE_CONFIG_DIR")
            .unwrap_or_else(|| home.to_path_buf())
            .join(".claude.json"),
        ANTIGRAVITY => home.join(".gemini").join("config").join("mcp_config.json"),
        OPENCODE => env_dir(env, "XDG_CONFIG_HOME")
            .unwrap_or_else(|| home.join(".config"))
            .join("opencode")
            .join("opencode.json"),
        other => {
            return Err(Error::Invalid(format!(
                "user-scoped setup is not supported for {other}; use client-config {other} and merge it into its config"
            )));
        }
    })
}

/// Writes or updates trello-mcp in a client's user-level MCP configuration
/// without touching anything else in the file.
pub fn setup_user(client: &str, options: &Options, env: &Env) -> Result<PathBuf, Error> {
    let home = crate::config::home_dir().map_err(|e| Error::Invalid(e.to_string()))?;
    setup_user_in(client, &home, env, options)
}

pub(crate) fn setup_user_in(
    client: &str,
    home: &Path,
    env: &Env,
    options: &Options,
) -> Result<PathBuf, Error> {
    let client = normalize_client(client)?;
    let target = user_config_path(client, home, env)?;
    apply(client, &target, options, 0o600)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Env, no_env};
    use serde_json::Value;
    use std::fs;
    use std::sync::Arc;

    fn options() -> Options {
        Options {
            command: "/opt/trello-mcp".into(),
            config_path: String::new(),
        }
    }

    fn setup(client: &str, home: &Path) -> Result<PathBuf, Error> {
        setup_user_in(client, home, &no_env(), &options())
    }

    fn env_with(pairs: &[(&str, &Path)]) -> Env {
        let map: Vec<(String, String)> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.display().to_string()))
            .collect();
        Arc::new(move |name| map.iter().find(|(k, _)| k == name).map(|(_, v)| v.clone()))
    }

    #[test]
    fn user_config_paths() {
        let home = Path::new("/h");
        for (client, want) in [
            ("codex", "/h/.codex/config.toml"),
            ("claude-code", "/h/.claude.json"),
            ("antigravity", "/h/.gemini/config/mcp_config.json"),
            ("opencode", "/h/.config/opencode/opencode.json"),
        ] {
            assert_eq!(
                user_config_path(client, home, &no_env()).unwrap(),
                Path::new(want)
            );
        }
        assert!(user_config_path("claude-desktop", home, &no_env()).is_err());
    }

    #[test]
    fn relocated_config_homes_win() {
        let home = Path::new("/h");
        let env = env_with(&[
            ("CODEX_HOME", Path::new("/x/codex")),
            ("CLAUDE_CONFIG_DIR", Path::new("/x/claude")),
            ("XDG_CONFIG_HOME", Path::new("/x/xdg")),
        ]);
        for (client, want) in [
            ("codex", "/x/codex/config.toml"),
            ("claude-code", "/x/claude/.claude.json"),
            ("opencode", "/x/xdg/opencode/opencode.json"),
            ("antigravity", "/h/.gemini/config/mcp_config.json"),
        ] {
            assert_eq!(
                user_config_path(client, home, &env).unwrap(),
                Path::new(want)
            );
        }
        // Empty values count as unset.
        let empty: Env = Arc::new(|_| Some("  ".into()));
        assert_eq!(
            user_config_path("codex", home, &empty).unwrap(),
            Path::new("/h/.codex/config.toml")
        );
    }

    #[test]
    fn setup_writes_to_relocated_home_not_default() {
        let (home, other) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let env = env_with(&[("CODEX_HOME", other.path())]);
        let target = setup_user_in("codex", home.path(), &env, &options()).unwrap();
        assert_eq!(target, other.path().join("config.toml"));
        assert!(!home.path().join(".codex").exists());
    }

    #[test]
    fn creates_missing_files_for_every_client_with_mode_0600() {
        for client in ["codex", "claude-code", "antigravity", "opencode"] {
            let home = tempfile::tempdir().unwrap();
            let target = setup(client, home.path()).unwrap();
            assert!(
                fs::read_to_string(&target).unwrap().contains("trello"),
                "{client}"
            );
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mode = fs::metadata(&target).unwrap().permissions().mode() & 0o777;
                assert_eq!(mode, 0o600, "{client}");
            }
        }
    }

    #[test]
    fn claude_code_entry_has_stdio_type() {
        let home = tempfile::tempdir().unwrap();
        let target = setup("claude-code", home.path()).unwrap();
        let doc: Value = serde_json::from_slice(&fs::read(target).unwrap()).unwrap();
        assert_eq!(doc["mcpServers"]["trello"]["type"], "stdio");
    }

    #[test]
    fn foreign_content_survives_byte_for_byte() {
        let foreign = "{\n\t// keep me\n\t\"zeta\": 1.50,\n\t\"mcpServers\": {\n\t\t\"other\": {\"command\": \"x\"},\n\t},\n\t\"alpha\": [1, 2]\n}\n";
        let home = tempfile::tempdir().unwrap();
        let target = home.path().join(".claude.json");
        fs::write(&target, foreign).unwrap();
        crate::util::set_mode(&target, 0o640).unwrap();
        setup("claude-code", home.path()).unwrap();
        let got = fs::read_to_string(&target).unwrap();
        let start = got.find("\n\t\t\"trello\"").unwrap();
        let end = start + got[start..].find("\n\t},").unwrap();
        assert_eq!(format!("{}{}", &got[..start], &got[end..]), foreign);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&target).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o640);
        }
        setup("claude-code", home.path()).unwrap();
        assert_eq!(fs::read_to_string(&target).unwrap(), got);
    }

    #[test]
    fn replacing_entry_keeps_users_own_fields() {
        let home = tempfile::tempdir().unwrap();
        let target = home.path().join(".config/opencode/opencode.json");
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(
            &target,
            "{\"mcp\":{\"trello\":{\"command\":[\"old\"],\"enabled\":false,\"environment\":{\"K\":\"v\"},\"type\":\"local\"}}}",
        )
        .unwrap();
        setup("opencode", home.path()).unwrap();
        let doc: Value = serde_json::from_slice(&fs::read(&target).unwrap()).unwrap();
        let e = &doc["mcp"]["trello"];
        assert_eq!(e["enabled"], false);
        assert_eq!(e["environment"]["K"], "v");
        assert_eq!(e["command"][0], "/opt/trello-mcp");
    }

    #[test]
    fn non_object_entry_is_replaced() {
        let home = tempfile::tempdir().unwrap();
        let target = home.path().join(".claude.json");
        fs::write(&target, "{\"mcpServers\":{\"trello\":1}}").unwrap();
        setup("claude-code", home.path()).unwrap();
        let doc: Value = serde_json::from_slice(&fs::read(&target).unwrap()).unwrap();
        assert_eq!(doc["mcpServers"]["trello"]["type"], "stdio");
    }

    #[test]
    fn adds_container_to_file_without_it() {
        let home = tempfile::tempdir().unwrap();
        let target = home.path().join(".claude.json");
        fs::write(&target, "{\n  \"a\": {\"b\": 1}\n}\n").unwrap();
        setup("claude-code", home.path()).unwrap();
        let got = fs::read_to_string(&target).unwrap();
        assert!(
            got.starts_with("{\n  \"a\": {\"b\": 1},\n  \"mcpServers\": {"),
            "{got}"
        );
        let doc: Value = serde_json::from_str(&got).unwrap();
        assert!(doc["mcpServers"]["trello"].is_object());
    }

    #[test]
    fn fills_empty_container_and_top_level() {
        for original in ["{}", "{\"mcpServers\": {}}", "{\n  \"mcpServers\": { }\n}"] {
            let home = tempfile::tempdir().unwrap();
            let target = home.path().join(".claude.json");
            fs::write(&target, original).unwrap();
            setup("claude-code", home.path()).unwrap();
            let doc: Value = serde_json::from_slice(&fs::read(&target).unwrap()).unwrap();
            assert!(doc["mcpServers"]["trello"].is_object(), "{original}");
        }
    }

    #[test]
    fn opencode_keeps_existing_servers_and_adds_no_schema() {
        let home = tempfile::tempdir().unwrap();
        let target = home.path().join(".config/opencode/opencode.json");
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        let original = "{\"theme\":\"x\",\"mcp\":{\"o\":{\"type\":\"remote\"}}}";
        fs::write(&target, original).unwrap();
        setup("opencode", home.path()).unwrap();
        let got = fs::read_to_string(&target).unwrap();
        assert!(
            got.starts_with("{\"theme\":\"x\",\"mcp\":{\"o\":{\"type\":\"remote\"}"),
            "{got}"
        );
        let doc: Value = serde_json::from_str(&got).unwrap();
        assert_eq!(doc["mcp"]["trello"]["type"], "local");
        assert!(doc.get("$schema").is_none());
    }

    #[test]
    fn invalid_json_is_refused_and_untouched() {
        let deep = format!("{}{}", "[".repeat(200), "]".repeat(200));
        for original in [
            "[1]",
            "{\"a\": }",
            "{\"mcpServers\": []}",
            "{} trailing",
            "{\"a\": tru}",
            "{\"a\": NaN}",
            "{\"a\": 1e}",
            &format!("{{\"a\": {deep}}}"),
        ] {
            let home = tempfile::tempdir().unwrap();
            let target = home.path().join(".claude.json");
            fs::write(&target, original).unwrap();
            let err = setup("claude-code", home.path()).unwrap_err().to_string();
            assert!(
                err.contains("claude-code config") && err.contains(".claude.json"),
                "{err}"
            );
            assert_eq!(fs::read_to_string(&target).unwrap(), original);
        }
    }

    #[test]
    fn bom_and_crlf_are_kept() {
        let home = tempfile::tempdir().unwrap();
        let target = home.path().join(".claude.json");
        fs::write(&target, "\u{feff}{\r\n  \"a\": 1\r\n}\r\n").unwrap();
        setup("claude-code", home.path()).unwrap();
        let got = fs::read_to_string(&target).unwrap();
        assert!(
            got.starts_with("\u{feff}{\r\n  \"a\": 1,\r\n  \"mcpServers\""),
            "{got:?}"
        );
        assert!(!got.replace("\r\n", "").contains('\n'), "{got:?}");
        serde_json::from_str::<Value>(got.trim_start_matches('\u{feff}')).unwrap();
    }

    #[test]
    fn codex_keeps_foreign_toml_and_refuses_unmanaged_table() {
        let home = tempfile::tempdir().unwrap();
        let target = home.path().join(".codex/config.toml");
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        let original = "model = \"x\"\n# note\n[other]\na = 1\n";
        fs::write(&target, original).unwrap();
        setup("codex", home.path()).unwrap();
        assert!(
            fs::read_to_string(&target)
                .unwrap()
                .starts_with(original.trim_end())
        );
        for unmanaged in [
            "[mcp_servers.trello]\ncommand = \"old\"\n",
            "mcp_servers = { trello = { command = \"old\" } }\n",
            "[mcp_servers]\n\"trello\" = { command = \"old\" }\n",
            "mcp_servers.trello.command = \"old\"\n",
        ] {
            fs::write(&target, unmanaged).unwrap();
            let err = setup("codex", home.path()).unwrap_err().to_string();
            assert!(
                err.contains("un-managed") && err.contains("config.toml"),
                "{err}"
            );
            assert_eq!(fs::read_to_string(&target).unwrap(), unmanaged);
        }
        // A commented-out table is not an existing one.
        fs::write(&target, "# [mcp_servers.trello]\nmodel = \"x\"\n").unwrap();
        setup("codex", home.path()).unwrap();
        // Unparseable TOML fails closed.
        fs::write(&target, "a = = 1\n").unwrap();
        assert!(setup("codex", home.path()).is_err());
        assert_eq!(fs::read_to_string(&target).unwrap(), "a = = 1\n");
    }

    #[test]
    fn codex_managed_block_rules() {
        let home = tempfile::tempdir().unwrap();
        let target = home.path().join(".codex/config.toml");
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        let (b, e) = (
            "# BEGIN trello-mcp managed configuration",
            "# END trello-mcp managed configuration",
        );
        let run = |text: &str| {
            fs::write(&target, text).unwrap();
            let r = setup("codex", home.path());
            (r, fs::read_to_string(&target).unwrap())
        };
        // Codex adds tables between our table and END: kept, after the fresh block.
        let (r, got) = run(&format!(
            "x = 1\n\n{b}\n[mcp_servers.trello]\ncommand = \"/old/trello-mcp\"\n\n[projects.\"/x\"]\ntrust_level = \"trusted\"\n\n[features]\nf = true\n{e}\n"
        ));
        r.unwrap();
        assert_eq!(got.matches(e).count(), 1, "{got}");
        assert!(
            got.contains("/opt/trello-mcp") && !got.contains("/old/"),
            "{got}"
        );
        assert!(
            got.find(e).unwrap() < got.find("[projects.\"/x\"]").unwrap(),
            "{got}"
        );
        assert!(
            got.contains("[projects.\"/x\"]\ntrust_level = \"trusted\"\n\n[features]\nf = true\n"),
            "{got}"
        );
        assert!(got.starts_with("x = 1\n\n"), "{got}");
        // Foreign settings before our table: refused, file untouched.
        let foreign = format!("{b}\nfoo = 1\n[mcp_servers.trello]\ncommand = \"x\"\n{e}\n");
        let (r, got) = run(&foreign);
        assert!(
            r.unwrap_err()
                .to_string()
                .contains("other than mcp_servers.trello")
        );
        assert_eq!(got, foreign);
        // A foreign table between our trello tables: refused.
        let between = format!(
            "{b}\n[mcp_servers.trello]\ncommand = \"x\"\n[other]\na = 1\n[mcp_servers.trello.env]\nK = \"v\"\n{e}\n"
        );
        let (r, got) = run(&between);
        assert!(r.unwrap_err().to_string().contains("between"));
        assert_eq!(got, between);
        // Orphan END, our table far from it: replaced wherever it sits.
        let (r, got) = run(&format!(
            "[mcp_servers.trello]\ncommand = \"/old/trello-mcp\"\nargs = [\"serve\"]\n\n[mcp_servers.zed]\ncommand = \"z\"\n{e}\n"
        ));
        r.unwrap();
        assert_eq!(got.matches("[mcp_servers.trello]").count(), 1, "{got}");
        assert_eq!(got.matches(e).count(), 1, "{got}");
        assert!(
            got.starts_with(b) && got.contains("[mcp_servers.zed]\ncommand = \"z\"\n"),
            "{got}"
        );
        assert!(!got.contains("/old/"), "{got}");
        // The user's own keys and subtables on the old table survive.
        let (r, got) = run(&format!(
            "{b}\n[mcp_servers.trello]\ncommand = \"old\"\nenabled = false\ntool_timeout_sec = 9\n[mcp_servers.trello.env]\nK = \"v\"\n{e}\n"
        ));
        r.unwrap();
        let doc: toml::Table = got.parse().unwrap();
        let t = &doc["mcp_servers"]["trello"];
        assert_eq!(t["command"].as_str(), Some("/opt/trello-mcp"));
        assert_eq!(t["enabled"].as_bool(), Some(false));
        assert_eq!(t["tool_timeout_sec"].as_integer(), Some(9));
        assert_eq!(t["env"]["K"].as_str(), Some("v"));
        // User comments after END survive, at EOF and before a foreign header.
        let tail = "# ===== section =====\n\n# [mcp_servers.figma]\n# command = \"npx\"\n";
        for (head, with_begin) in [("", true), ("", false)] {
            let begin = if with_begin {
                format!("{b}\n")
            } else {
                String::new()
            };
            let eof = format!(
                "{head}model = \"m\"\n\n{begin}[mcp_servers.trello]\ncommand = \"old\"\nargs = []\n{e}\n\n{tail}"
            );
            let (r, got) = run(&eof);
            r.unwrap();
            assert!(got.ends_with(&format!("\n{e}\n\n{tail}")), "{got}");
            let before = format!(
                "{begin}[mcp_servers.trello]\ncommand = \"old\"\n{e}\n\n{tail}\n[projects.\"/x\"]\ntrust_level = \"trusted\"\n"
            );
            let (r, got) = run(&before);
            r.unwrap();
            assert!(
                got.contains(&format!("{e}\n\n{tail}\n[projects.\"/x\"]\n")),
                "{got}"
            );
            assert_eq!(got.matches(e).count(), 1, "{got}");
            let again = got.clone();
            let (r, got) = run(&again);
            r.unwrap();
            assert_eq!(got, again);
        }
        // Orphan END after another table, no trello table: the marker line is dropped.
        let (r, got) = run(&format!("[mcp_servers.canvashand]\ncommand = \"c\"\n{e}\n"));
        r.unwrap();
        assert_eq!(got.matches(e).count(), 1);
        assert!(
            got.starts_with("[mcp_servers.canvashand]\ncommand = \"c\"\n\n"),
            "{got}"
        );
        // Orca/Codex rewrites leave a lone BEGIN on another table, the
        // markers out of order, or none at all: stray markers dropped, our
        // table replaced.
        for drifted in [
            "[mcp_servers.trello]\ncommand = \"/old/bin/trello-mcp\"\nargs = [\"serve\"]\n\n[x]\na = 1\n"
                .to_string(),
            format!(
                "[mcp_servers.trello]\ncommand = \"/old/trello-mcp\"\n\n[x]\na = 1\n\n{b}\n\n[features]\nf = false\n"
            ),
            format!(
                "[mcp_servers.trello]\ncommand = \"/old/trello-mcp\"\n{e}\n\n[x]\na = 1\n\n{b}\n\n[features]\nf = false\n"
            ),
            format!(
                "{b}\n{b}\n[mcp_servers.trello]\ncommand = \"/old/trello-mcp\"\n\n[x]\na = 1\n"
            ),
        ] {
            let (r, got) = run(&drifted);
            r.unwrap();
            assert_eq!(got.matches(b).count(), 1, "{got}");
            assert_eq!(got.matches(e).count(), 1, "{got}");
            assert!(got.find(b).unwrap() < got.find(e).unwrap(), "{got}");
            assert!(
                !got.contains("/old/") && got.contains("[x]\na = 1\n"),
                "{got}"
            );
            let again = got.clone();
            let (r, got) = run(&again);
            r.unwrap();
            assert_eq!(got, again);
        }
        // Lone BEGIN, no trello table: the marker line is dropped.
        let (r, got) = run(&format!("{b}\n"));
        r.unwrap();
        assert_eq!(got.matches(b).count(), 1, "{got}");
        assert!(got.starts_with(b), "{got}");
        // CRLF is kept.
        let (r, got) = run("model = \"x\"\r\n");
        r.unwrap();
        assert!(!got.replace("\r\n", "").contains('\n'), "{got:?}");
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_config_is_edited_through_the_link() {
        let home = tempfile::tempdir().unwrap();
        let real = home.path().join("dotfiles-claude.json");
        let link = home.path().join(".claude.json");
        fs::write(&real, "{\"a\":1}").unwrap();
        std::os::unix::fs::symlink(&real, &link).unwrap();
        setup("claude-code", home.path()).unwrap();
        assert!(
            fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert!(fs::read_to_string(&real).unwrap().contains("trello"));
        // A dangling link is an error, never replaced by a regular file.
        let dangling = tempfile::tempdir().unwrap();
        let link = dangling.path().join(".claude.json");
        std::os::unix::fs::symlink(dangling.path().join("nope"), &link).unwrap();
        assert!(setup("claude-code", dangling.path()).is_err());
        assert!(
            fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
    }
}
