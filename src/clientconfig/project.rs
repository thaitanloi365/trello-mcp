//! Project-scoped client configuration: path lookup and merge into existing files.

use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::{Map, Value};

use super::{
    ANTIGRAVITY, CLAUDE_CODE, CLAUDE_DESKTOP, CODEX, Error, OPENCODE, Options, go_escape,
    normalize_client, render,
};

const CODEX_MANAGED_BEGIN: &str = "# BEGIN trello-mcp managed configuration";
const CODEX_MANAGED_END: &str = "# END trello-mcp managed configuration";

fn io_err(context: String) -> impl FnOnce(std::io::Error) -> Error {
    move |cause| Error::Io { context, cause }
}

/// Returns the client-recognized config path relative to a project root.
/// Claude Desktop does not support project-scoped MCP config.
pub fn project_config_path(client: &str) -> Result<&'static str, Error> {
    match normalize_client(client)? {
        CODEX => Ok(".codex/config.toml"),
        CLAUDE_CODE => Ok(".mcp.json"),
        ANTIGRAVITY => Ok(".agents/mcp_config.json"),
        OPENCODE => Ok("opencode.json"),
        CLAUDE_DESKTOP => Err(Error::Invalid(format!(
            "{CLAUDE_DESKTOP} does not support project-scoped MCP configuration; use client-config {CLAUDE_DESKTOP} and merge it into the desktop config"
        ))),
        other => Err(Error::Invalid(format!(
            "project configuration is not supported for {other}"
        ))),
    }
}

/// Writes or updates trello-mcp in a client's project-scoped MCP configuration
/// without replacing unrelated client settings or MCP servers.
pub fn setup_project(client: &str, project_dir: &str, options: &Options) -> Result<PathBuf, Error> {
    let client = normalize_client(client)?;
    let relative = project_config_path(client)?;
    let project_dir = if project_dir.trim().is_empty() {
        "."
    } else {
        project_dir
    };
    let root = crate::util::absolute(Path::new(project_dir))
        .map_err(io_err("resolve project directory".into()))?;
    let target = root.join(relative);
    let generated = render(client, options)?;
    let existing = match fs::read(&target) {
        Ok(data) => data,
        Err(e) if e.kind() == ErrorKind::NotFound => Vec::new(),
        Err(e) => {
            return Err(io_err(format!("read project config {}", target.display()))(
                e,
            ));
        }
    };

    let output = if client == CODEX {
        merge_codex(&existing, &generated, &target.display().to_string())?
    } else {
        merge_json(&existing, &generated, client)?
    };
    write_project_file(&target, &output)?;
    Ok(target)
}

fn merge_codex(existing: &[u8], generated: &str, target: &str) -> Result<Vec<u8>, Error> {
    let block = format!(
        "{CODEX_MANAGED_BEGIN}\n{}\n{CODEX_MANAGED_END}",
        generated.trim()
    );
    let content = String::from_utf8_lossy(existing);
    let start = content.find(CODEX_MANAGED_BEGIN);
    let end = content.find(CODEX_MANAGED_END);
    if start.is_some() != end.is_some() || matches!((start, end), (Some(s), Some(e)) if e < s) {
        return Err(Error::Invalid(
            "existing Codex trello-mcp managed block is incomplete".into(),
        ));
    }
    if let (Some(start), Some(end)) = (start, end) {
        let end = end + CODEX_MANAGED_END.len();
        return Ok(format!("{}{block}{}", &content[..start], &content[end..]).into_bytes());
    }
    const UNMANAGED: &str = "[mcp_servers.trello]";
    if let Some(offset) = content.find(UNMANAGED) {
        let line = content[..offset].matches('\n').count() + 1;
        return Err(Error::Invalid(format!(
            "{target}:{line}: existing un-managed {UNMANAGED} section found; remove it or use client-config codex to merge manually"
        )));
    }
    let content = content.trim_end_matches(['\r', '\n']);
    if content.is_empty() {
        return Ok(format!("{block}\n").into_bytes());
    }
    Ok(format!("{content}\n\n{block}\n").into_bytes())
}

// Like Go's Decoder.Decode: reads one value and ignores any trailing data.
fn decode_json(data: &[u8]) -> Result<Map<String, Value>, serde_json::Error> {
    Map::deserialize(&mut serde_json::Deserializer::from_slice(data))
}

fn merge_json(existing: &[u8], generated: &str, client: &str) -> Result<Vec<u8>, Error> {
    let mut document = Map::new();
    if !existing.trim_ascii().is_empty() {
        document = decode_json(existing)
            .map_err(|e| Error::Invalid(format!("parse existing {client} project config: {e}")))?;
    }
    let mut generated_document = decode_json(generated.as_bytes())
        .map_err(|e| Error::Invalid(format!("parse generated {client} project config: {e}")))?;

    let mut container_key = "mcpServers";
    if client == OPENCODE {
        container_key = "mcp";
        if !document.contains_key("$schema") {
            document.insert(
                "$schema".into(),
                generated_document
                    .get("$schema")
                    .cloned()
                    .unwrap_or(Value::Null),
            );
        }
    }
    let invalid = || {
        Error::Invalid(format!(
            "generated {client} config has invalid {container_key} object"
        ))
    };
    let Some(Value::Object(mut generated_container)) = generated_document.remove(container_key)
    else {
        return Err(invalid());
    };
    let trello = generated_container.remove("trello").unwrap_or(Value::Null);

    let container = document
        .entry(container_key)
        .or_insert_with(|| Value::Object(Map::new()));
    let Value::Object(servers) = container else {
        return Err(Error::Invalid(format!(
            "existing {client} config field {container_key:?} is not an object"
        )));
    };
    servers.insert("trello".into(), trello);

    let data = serde_json::to_string_pretty(&document)
        .map_err(|e| Error::Invalid(format!("encode {client} project config: {e}")))?;
    Ok((go_escape(&data) + "\n").into_bytes())
}

fn write_project_file(path: &Path, data: &[u8]) -> Result<(), Error> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(io_err("create project config directory".into()))?;
    }
    let mut mode = 0o644;
    match fs::metadata(path) {
        #[cfg(unix)]
        Ok(info) => {
            use std::os::unix::fs::PermissionsExt;
            mode = info.permissions().mode() & 0o777;
        }
        #[cfg(not(unix))]
        Ok(_) => {}
        Err(e) if e.kind() == ErrorKind::NotFound => {}
        Err(e) => return Err(io_err(format!("inspect project config {}", path.display()))(e)),
    }
    crate::util::write_atomic(path, data, mode, ".trello-mcp-config-")
        .map_err(io_err(format!("write project config {}", path.display())))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options() -> Options {
        Options {
            command: "/opt/trello/bin/trello-mcp".into(),
            config_path: "/home/user/.trello-mcp/config.json".into(),
        }
    }

    fn setup(client: &str, root: &Path) -> Result<PathBuf, Error> {
        setup_project(client, root.to_str().unwrap(), &options())
    }

    #[test]
    fn project_config_paths() {
        for (client, want) in [
            ("codex", ".codex/config.toml"),
            ("claude-code", ".mcp.json"),
            ("antigravity", ".agents/mcp_config.json"),
            ("opencode", "opencode.json"),
        ] {
            assert_eq!(project_config_path(client).unwrap(), want);
        }
        assert!(project_config_path("claude-desktop").is_err());
    }

    #[test]
    fn setup_project_json_clients() {
        for client in ["claude-code", "antigravity", "opencode"] {
            let root = tempfile::tempdir().unwrap();
            let target = setup(client, root.path()).unwrap();
            let doc: Value = serde_json::from_slice(&fs::read(target).unwrap()).unwrap();
            let key = if client == "opencode" {
                "mcp"
            } else {
                "mcpServers"
            };
            assert!(doc[key]["trello"].is_object(), "{client}: {doc}");
        }
    }

    #[test]
    fn setup_project_preserves_existing_json() {
        let root = tempfile::tempdir().unwrap();
        let target = root.path().join(".mcp.json");
        fs::write(
            &target,
            r#"{"otherSetting":true,"mcpServers":{"other":{"command":"other"}}}"#,
        )
        .unwrap();
        crate::util::set_mode(&target, 0o640).unwrap();
        setup("claude-code", root.path()).unwrap();
        let doc: Value = serde_json::from_slice(&fs::read(&target).unwrap()).unwrap();
        assert_eq!(doc["otherSetting"], true);
        assert!(doc["mcpServers"]["other"].is_object());
        assert!(doc["mcpServers"]["trello"].is_object());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&target).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o640);
        }
    }

    #[test]
    fn setup_project_rejects_non_object_container() {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join(".mcp.json"), r#"{"mcpServers":[]}"#).unwrap();
        let err = setup("claude-code", root.path()).unwrap_err().to_string();
        assert_eq!(
            err,
            "existing claude-code config field \"mcpServers\" is not an object"
        );
    }

    #[test]
    fn setup_project_updates_managed_codex_block() {
        let root = tempfile::tempdir().unwrap();
        let target = root.path().join(".codex/config.toml");
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(&target, "model = \"example\"\n").unwrap();
        let mut o = options();
        setup_project("codex", root.path().to_str().unwrap(), &o).unwrap();
        o.command = "/new/trello-mcp".into();
        setup_project("codex", root.path().to_str().unwrap(), &o).unwrap();
        let content = fs::read_to_string(&target).unwrap();
        assert!(content.contains("model = \"example\"") && content.contains(&o.command));
        assert_eq!(content.matches("[mcp_servers.trello]").count(), 1);
        assert_eq!(content.matches(CODEX_MANAGED_BEGIN).count(), 1);
    }

    #[test]
    fn setup_project_rejects_unmanaged_codex_section() {
        let root = tempfile::tempdir().unwrap();
        let target = root.path().join(".codex/config.toml");
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(
            &target,
            "model = \"example\"\n\n[mcp_servers.trello]\ncommand = \"old\"\n",
        )
        .unwrap();
        let err = setup("codex", root.path()).unwrap_err().to_string();
        assert!(err.contains("un-managed"), "{err}");
        assert!(err.contains(&format!("{}:3:", target.display())), "{err}");
    }

    #[test]
    fn codex_incomplete_block() {
        let err = merge_codex(CODEX_MANAGED_END.as_bytes(), "x", "t").unwrap_err();
        assert_eq!(
            err.to_string(),
            "existing Codex trello-mcp managed block is incomplete"
        );
    }
}
