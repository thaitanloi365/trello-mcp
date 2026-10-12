//! Renders MCP client configuration for trello-mcp.

use std::collections::BTreeMap;

use serde::Serialize;

pub mod project;

pub use project::setup_project;

const CODEX: &str = "codex";
const CLAUDE_CODE: &str = "claude-code";
const CLAUDE_DESKTOP: &str = "claude-desktop";
const ANTIGRAVITY: &str = "antigravity";
const OPENCODE: &str = "opencode";

pub const SUPPORTED_CLIENTS: [&str; 5] =
    [CODEX, CLAUDE_CODE, CLAUDE_DESKTOP, ANTIGRAVITY, OPENCODE];
pub const PROJECT_CLIENTS: [&str; 4] = [CODEX, CLAUDE_CODE, ANTIGRAVITY, OPENCODE];

/// Local paths a client uses to start trello-mcp. An empty `config_path`
/// means trello-mcp resolves its default config path at startup.
pub struct Options {
    pub command: String,
    pub config_path: String,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Invalid(String),
    #[error("{context}: {source}")]
    Io {
        context: String,
        source: std::io::Error,
    },
}

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
struct OpenCodeConfig {
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

/// Produces a complete MCP configuration document for a supported client.
/// Aliases "claude", "antigrav" and "open-code" are accepted.
pub fn render(client: &str, options: &Options) -> Result<String, Error> {
    let client = normalize_client(client)?;
    if options.command.trim().is_empty() {
        return Err(Error::Invalid("server command is required".into()));
    }

    let mut args = vec!["serve".to_string()];
    if !options.config_path.trim().is_empty() {
        args.push("--config".into());
        args.push(options.config_path.clone());
    }
    if client == CODEX {
        return Ok(render_codex(&options.command, &args));
    }
    if client == OPENCODE {
        let mut command = vec![options.command.clone()];
        command.extend(args);
        let payload = OpenCodeConfig {
            schema: "https://opencode.ai/config.json",
            mcp: BTreeMap::from([(
                "trello",
                OpenCodeServer {
                    kind: "local",
                    command,
                    enabled: true,
                },
            )]),
        };
        return render_json(client, &payload);
    }

    let payload = JsonConfig {
        mcp_servers: BTreeMap::from([(
            "trello",
            StdioServer {
                kind: if client == CLAUDE_CODE { "stdio" } else { "" },
                command: &options.command,
                args: &args,
            },
        )]),
    };
    render_json(client, &payload)
}

fn render_json(client: &str, payload: &impl Serialize) -> Result<String, Error> {
    let data = serde_json::to_string_pretty(payload)
        .map_err(|e| Error::Invalid(format!("encode {client} configuration: {e}")))?;
    Ok(go_escape(&data) + "\n")
}

/// Go's encoding/json escapes <, >, & and U+2028/2029 inside strings; those
/// characters never appear outside strings in JSON, so a blanket replace is safe.
pub(crate) fn go_escape(json: &str) -> String {
    let mut out = String::with_capacity(json.len());
    for c in json.chars() {
        match c {
            '<' => out.push_str("\\u003c"),
            '>' => out.push_str("\\u003e"),
            '&' => out.push_str("\\u0026"),
            '\u{2028}' => out.push_str("\\u2028"),
            '\u{2029}' => out.push_str("\\u2029"),
            c => out.push(c),
        }
    }
    out
}

fn normalize_client(client: &str) -> Result<&'static str, Error> {
    match client.trim().to_lowercase().as_str() {
        CODEX => Ok(CODEX),
        "claude" | CLAUDE_CODE => Ok(CLAUDE_CODE),
        CLAUDE_DESKTOP => Ok(CLAUDE_DESKTOP),
        "antigrav" | ANTIGRAVITY => Ok(ANTIGRAVITY),
        "open-code" | OPENCODE => Ok(OPENCODE),
        _ => Err(Error::Invalid(format!(
            "unsupported client {client:?} (choose {})",
            SUPPORTED_CLIENTS.join(", ")
        ))),
    }
}

fn render_codex(command: &str, args: &[String]) -> String {
    let list: Vec<String> = args.iter().map(|a| quote(a)).collect();
    format!(
        "[mcp_servers.trello]\ncommand = {}\nargs = [{}]\n",
        quote(command),
        list.join(", ")
    )
}

// JSON string quoting is compatible with TOML basic strings for local paths
// and command arguments while correctly escaping quotes and backslashes.
fn quote(value: &str) -> String {
    go_escape(&serde_json::to_string(value).unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    const COMMAND: &str = "/opt/trello/bin/trello-mcp";
    const CONFIG: &str = "/home/user/.trello-mcp/config.json";

    fn opts(config: &str) -> Options {
        Options {
            command: COMMAND.into(),
            config_path: config.into(),
        }
    }

    #[test]
    fn render_codex() {
        let got = render("codex", &opts(CONFIG)).unwrap();
        let want = "[mcp_servers.trello]\n\
            command = \"/opt/trello/bin/trello-mcp\"\n\
            args = [\"serve\", \"--config\", \"/home/user/.trello-mcp/config.json\"]\n";
        assert_eq!(got, want);
    }

    #[test]
    fn render_uses_default_config_without_argument() {
        for client in SUPPORTED_CLIENTS {
            let out = render(client, &opts("")).unwrap();
            assert!(!out.contains("--config") && !out.contains(CONFIG), "{out}");
            assert!(out.contains("serve"), "{out}");
        }
    }

    #[test]
    fn render_json_clients() {
        for (client, want_type) in [
            ("claude-code", Some("stdio")),
            ("claude-desktop", None),
            ("antigravity", None),
        ] {
            let out = render(client, &opts(CONFIG)).unwrap();
            let v: serde_json::Value = serde_json::from_str(&out).unwrap();
            let server = &v["mcpServers"]["trello"];
            assert_eq!(server.get("type").and_then(|t| t.as_str()), want_type);
            assert_eq!(server["command"], COMMAND);
            assert_eq!(
                server["args"],
                serde_json::json!(["serve", "--config", CONFIG])
            );
        }
    }

    #[test]
    fn render_aliases() {
        let o = opts(CONFIG);
        for (alias, canonical) in [
            ("claude", "claude-code"),
            ("antigrav", "antigravity"),
            ("open-code", "opencode"),
        ] {
            assert_eq!(render(alias, &o).unwrap(), render(canonical, &o).unwrap());
        }
    }

    #[test]
    fn render_open_code() {
        let out = render("opencode", &opts(CONFIG)).unwrap();
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["$schema"], "https://opencode.ai/config.json");
        let server = &v["mcp"]["trello"];
        assert_eq!(server["type"], "local");
        assert_eq!(server["enabled"], true);
        assert_eq!(
            server["command"],
            serde_json::json!([COMMAND, "serve", "--config", CONFIG])
        );
    }

    #[test]
    fn render_rejects_unsupported_client() {
        let err = render("other", &opts(CONFIG)).unwrap_err().to_string();
        assert_eq!(
            err,
            "unsupported client \"other\" (choose codex, claude-code, claude-desktop, antigravity, opencode)"
        );
    }

    #[test]
    fn render_requires_command() {
        let o = Options {
            command: " ".into(),
            config_path: String::new(),
        };
        assert_eq!(
            render("codex", &o).unwrap_err().to_string(),
            "server command is required"
        );
    }

    #[test]
    fn render_escapes_like_go() {
        let o = Options {
            command: "/a&b<c>".into(),
            config_path: String::new(),
        };
        assert!(
            render("claude-desktop", &o)
                .unwrap()
                .contains("/a\\u0026b\\u003cc\\u003e")
        );
    }

    #[test]
    fn render_matches_go_fixtures() {
        for client in SUPPORTED_CLIENTS {
            for (suffix, config_path) in [("", ""), (".config", "/x/config.json")] {
                let want = std::fs::read_to_string(format!(
                    "{}/tests/fixtures/client-config/{client}{suffix}.out",
                    env!("CARGO_MANIFEST_DIR")
                ))
                .unwrap();
                let options = Options {
                    command: "/x/trello-mcp".into(),
                    config_path: config_path.into(),
                };
                assert_eq!(render(client, &options).unwrap(), want, "{client}{suffix}");
            }
        }
    }
}
