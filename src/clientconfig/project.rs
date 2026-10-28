//! Project-scoped client configuration: path lookup and merge into existing files.

use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use serde_json::Value;

use super::{
    ANTIGRAVITY, CLAUDE_CODE, CLAUDE_DESKTOP, CODEX, Error, OPENCODE, Options, PROJECT_CLIENTS,
    jsonpatch, normalize_client, render,
};

const CODEX_MANAGED_BEGIN: &str = "# BEGIN trello-mcp managed configuration";
const CODEX_MANAGED_END: &str = "# END trello-mcp managed configuration";

pub(super) fn io_err(context: String) -> impl FnOnce(std::io::Error) -> Error {
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
    apply(client, &root.join(relative), options, 0o644)
}

/// Project config files in `dir` that already start trello-mcp for one of
/// `clients`. Used to warn that a user-scope setup would make clients load it
/// twice. Nothing is reported when `dir` is `home`, and a file that is one of
/// the user `written` targets is never a project file.
pub fn project_setups(
    dir: &Path,
    home: &Path,
    written: &[PathBuf],
    clients: &[String],
) -> Vec<PathBuf> {
    let same = |a: &Path, b: &Path| matches!((fs::canonicalize(a), fs::canonicalize(b)), (Ok(a), Ok(b)) if a == b);
    if same(dir, home) {
        return Vec::new();
    }
    let mut found = Vec::new();
    for client in PROJECT_CLIENTS {
        if !clients
            .iter()
            .any(|c| normalize_client(c).is_ok_and(|n| n == client))
        {
            continue;
        }
        let Ok(rel) = project_config_path(client) else {
            continue;
        };
        let path = dir.join(rel);
        if written.iter().any(|w| same(w, &path)) {
            continue;
        }
        let Ok(text) = fs::read_to_string(&path) else {
            continue;
        };
        let command_ok = |cmd: &str| cmd.contains("trello-mcp");
        // The text match is only for files that do not parse.
        let hit = if client == CODEX {
            match text.parse::<toml::Table>() {
                Ok(t) => t
                    .get("mcp_servers")
                    .and_then(|m| m.get("trello")?.get("command")?.as_str())
                    .is_some_and(command_ok),
                Err(_) => text.contains("trello-mcp"),
            }
        } else {
            let key = if client == OPENCODE {
                "mcp"
            } else {
                "mcpServers"
            };
            match serde_json::from_str::<Value>(&text) {
                Ok(v) => v
                    .get(key)
                    .and_then(|m| m.get("trello")?.get("command"))
                    .and_then(|c| c.as_str().or_else(|| c.get(0)?.as_str()))
                    .is_some_and(command_ok),
                Err(_) => text.contains("trello-mcp"),
            }
        };
        if hit {
            found.push(path);
        }
    }
    found
}

/// Merges trello-mcp into `target`, a client's config file. JSON is patched
/// as text so every other byte survives; Codex TOML uses a managed block.
/// A symlinked file is edited through the link. `new_mode` applies only to
/// files this call creates.
pub(super) fn apply(
    client: &str,
    target: &Path,
    options: &Options,
    new_mode: u32,
) -> Result<PathBuf, Error> {
    let generated = render(client, options)?;
    let mut target = target.to_path_buf();
    if fs::symlink_metadata(&target).is_ok_and(|m| m.file_type().is_symlink()) {
        target = fs::canonicalize(&target).map_err(io_err(format!(
            "resolve symlinked {client} config {}",
            target.display()
        )))?;
    }
    let label = format!("{client} config {}", target.display());
    let mut attempt = 0;
    let output = loop {
        let existing = match fs::read(&target) {
            Ok(data) => data,
            Err(e) if e.kind() == ErrorKind::NotFound => Vec::new(),
            Err(e) => return Err(io_err(format!("read {label}"))(e)),
        };
        let (bom, body) = match existing.strip_prefix(b"\xEF\xBB\xBF") {
            Some(rest) => ("\u{feff}", rest),
            None => ("", existing.as_slice()),
        };
        let text = std::str::from_utf8(body)
            .map_err(|e| Error::Invalid(format!("parse existing {label}: {e}")))?;
        let eol = if text.contains("\r\n") { "\r\n" } else { "\n" };
        let wrap = |e: Error| match e {
            Error::Invalid(m) => Error::Invalid(format!("{label}: {m}")),
            other => other,
        };

        let output = if client == CODEX {
            merge_codex(text, &generated, &label, eol)?
        } else if !text.trim().is_empty() {
            let key = if client == OPENCODE {
                "mcp"
            } else {
                "mcpServers"
            };
            let entry = serde_json::from_str::<Value>(&generated)
                .ok()
                .and_then(|mut d| d.get_mut(key)?.get_mut("trello").map(Value::take))
                .ok_or_else(|| Error::Invalid(format!("generated {client} config is invalid")))?;
            let out = jsonpatch::patch(text, key, "trello", &entry, eol).map_err(wrap)?;
            if serde_json::from_str::<Value>(text).is_ok()
                && let Err(e) = serde_json::from_str::<Value>(&out)
            {
                return Err(wrap(Error::Invalid(format!(
                    "refusing to write invalid JSON: {e}"
                ))));
            }
            out
        } else {
            generated.clone()
        };
        let output = format!("{bom}{output}");
        // Another process (Claude Code rewrites ~/.claude.json) may have written
        // meanwhile: merge again on top of its version rather than revert it.
        attempt += 1;
        if attempt >= 3 || fs::read(&target).ok().as_deref().unwrap_or(&[]) == existing.as_slice() {
            break output;
        }
    };
    write_config_file(&target, output.as_bytes(), new_mode)?;
    Ok(target)
}

fn parse_toml(text: &str, label: &str) -> Result<toml::Table, Error> {
    text.parse::<toml::Table>()
        .map_err(|e| Error::Invalid(format!("{label}: cannot parse TOML: {}", e.message())))
}

struct Line {
    start: usize,
    /// End of the text, before the line break.
    end: usize,
    /// Start of the next line.
    next: usize,
}

fn lines(content: &str) -> Vec<Line> {
    let mut out = Vec::new();
    let mut start = 0;
    for raw in content.split_inclusive('\n') {
        let end = start + raw.trim_end_matches(['\r', '\n']).len();
        out.push(Line {
            start,
            end,
            next: start + raw.len(),
        });
        start += raw.len();
    }
    out
}

/// Key path of a table header line (`[a.b]` -> ["a", "b"]), if it is one.
fn header_path(line: &str) -> Option<Vec<String>> {
    let line = line.trim();
    if !line.starts_with('[') {
        return None;
    }
    let mut table = line.parse::<toml::Table>().ok()?;
    let mut path = Vec::new();
    while table.len() == 1 {
        let (key, value) = table.into_iter().next()?;
        path.push(key);
        table = match value {
            toml::Value::Table(t) => t,
            toml::Value::Array(mut a) if a.len() == 1 => a.remove(0).try_into().ok()?,
            _ => break,
        };
    }
    Some(path)
}

fn is_trello_path(path: &[String]) -> bool {
    path.len() >= 2 && path[0] == "mcp_servers" && path[1] == "trello"
}

/// End of our tables, starting at `from`: the first header that is not
/// mcp_servers.trello[.*], backed up over comment lines glued to it.
fn region_end(content: &str, ls: &[Line], from: usize) -> usize {
    let first = ls.iter().position(|l| l.start >= from).unwrap_or(ls.len());
    let header = (first..ls.len()).find(|&i| {
        header_path(&content[ls[i].start..ls[i].end]).is_some_and(|p| !is_trello_path(&p))
    });
    // Back up over blank and comment lines: they belong to the user, not to us.
    let mut j = header.unwrap_or(ls.len());
    let loose = |i: usize| {
        let t = content[ls[i].start..ls[i].end].trim();
        t.is_empty() || (t.starts_with('#') && t != CODEX_MANAGED_END && t != CODEX_MANAGED_BEGIN)
    };
    while j > first && loose(j - 1) {
        j -= 1;
    }
    ls.get(j).map_or(content.len(), |l| l.start)
}

/// Our block: the generated entry plus any keys and subtables the user added
/// to the old trello table that the generated entry does not set.
fn codex_block(
    generated: &str,
    old: Option<&toml::Table>,
    label: &str,
    eol: &str,
) -> Result<String, Error> {
    let mut doc = parse_toml(generated, label)?;
    let ours = doc
        .get_mut("mcp_servers")
        .and_then(|m| m.get_mut("trello"))
        .and_then(|t| t.as_table_mut())
        .ok_or_else(|| Error::Invalid("generated codex config is invalid".into()))?;
    let mut extras = false;
    for (k, v) in old.into_iter().flatten() {
        if !ours.contains_key(k) {
            ours.insert(k.clone(), v.clone());
            extras = true;
        }
    }
    let body = if extras {
        toml::to_string(&doc).map_err(|e| Error::Invalid(format!("{label}: encode TOML: {e}")))?
    } else {
        generated.to_string()
    };
    let body = body.trim().replace('\n', eol);
    Ok(format!(
        "{CODEX_MANAGED_BEGIN}{eol}{body}{eol}{CODEX_MANAGED_END}"
    ))
}

fn merge_codex(content: &str, generated: &str, label: &str, eol: &str) -> Result<String, Error> {
    let err = |m: String| Error::Invalid(format!("{label}: {m}"));
    let table = parse_toml(content, label)?;
    let ls = lines(content);
    let marked = |marker: &str| -> Vec<&Line> {
        ls.iter()
            .filter(|l| content[l.start..l.end].trim() == marker)
            .collect()
    };
    let (begins, ends) = (marked(CODEX_MANAGED_BEGIN), marked(CODEX_MANAGED_END));

    // Our region [from, to), and the old marker lines to delete.
    let (region, strays) = match (begins.as_slice(), ends.as_slice()) {
        ([], []) => (None, vec![]),
        ([b], [e]) if b.start < e.start => {
            let to = region_end(content, &ls, b.next);
            let span = parse_toml(&content[b.next..to], label)?;
            let only_ours = span.is_empty()
                || (span.len() == 1
                    && span
                        .get("mcp_servers")
                        .and_then(|v| v.as_table())
                        .is_some_and(|t| t.keys().all(|k| k == "trello")));
            if !only_ours {
                return Err(err(format!(
                    "settings other than mcp_servers.trello sit right after \"{CODEX_MANAGED_BEGIN}\"; move them out or remove the markers"
                )));
            }
            (Some((b.start, to)), vec![*e])
        }
        // Codex and Orca rewrite the file and drop or move our marker comments:
        // any marker proves we wrote it, so find our table by its header.
        _ => {
            let own = ls.iter().find(|l| {
                header_path(&content[l.start..l.end])
                    .is_some_and(|p| p == ["mcp_servers", "trello"])
            });
            let mut strays: Vec<&Line> = begins.iter().chain(&ends).copied().collect();
            strays.sort_by_key(|l| l.start);
            (
                own.map(|l| (l.start, region_end(content, &ls, l.next))),
                strays,
            )
        }
    };

    let mut content = content.to_string();
    let mut table = table;
    if let Some((from, to)) = region {
        let stray = ls.iter().any(|l| {
            (l.start < from || l.start >= to)
                && header_path(&content[l.start..l.end]).is_some_and(|p| is_trello_path(&p))
        });
        if stray {
            return Err(err(
                "foreign tables sit between the mcp_servers.trello tables; move them out of the way".into(),
            ));
        }
        let old = parse_toml(&content[from..to], label)?;
        let old = old
            .get("mcp_servers")
            .and_then(|m| m.get("trello"))
            .and_then(|t| t.as_table());
        let block = codex_block(generated, old, label, eol)?;
        let span = &content[from..to];
        let mut trailing = span[span.trim_end().len()..].to_string();
        if to == content.len() && trailing.is_empty() {
            trailing = eol.into();
        }
        let mut edits = vec![(from, to, format!("{block}{trailing}"))];
        for e in strays.iter().filter(|e| e.start < from || e.start >= to) {
            edits.push((e.start, e.next, String::new()));
        }
        edits.sort_by_key(|e| std::cmp::Reverse(e.0));
        for (a, b, with) in edits {
            content.replace_range(a..b, &with);
        }
        return checked_toml(content, label);
    }
    if !strays.is_empty() {
        for e in strays.iter().rev() {
            content.replace_range(e.start..e.next, "");
        }
        table = parse_toml(&content, label)?;
    }

    let has_trello = table
        .get("mcp_servers")
        .and_then(|v| v.as_table())
        .is_some_and(|t| t.contains_key("trello"));
    if has_trello {
        let at = content
            .lines()
            .position(|l| {
                !l.trim_start().starts_with('#')
                    && l.contains("mcp_servers")
                    && l.contains("trello")
            })
            .map_or(String::new(), |n| format!(":{}", n + 1));
        return Err(Error::Invalid(format!(
            "{label}{at}: existing un-managed mcp_servers.trello found; remove it or use client-config codex to merge manually"
        )));
    }
    let block = codex_block(generated, None, label, eol)?;
    let trimmed = content.trim_end_matches(['\r', '\n']);
    let out = if trimmed.is_empty() {
        format!("{block}{eol}")
    } else {
        format!("{trimmed}{eol}{eol}{block}{eol}")
    };
    checked_toml(out, label)
}

/// The merged TOML must still parse before it is written.
fn checked_toml(out: String, label: &str) -> Result<String, Error> {
    out.parse::<toml::Table>().map_err(|e| {
        Error::Invalid(format!(
            "{label}: refusing to write invalid TOML: {}",
            e.message()
        ))
    })?;
    Ok(out)
}

fn write_config_file(path: &Path, data: &[u8], new_mode: u32) -> Result<(), Error> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(io_err("create config directory".into()))?;
    }
    let mut mode = new_mode;
    match fs::metadata(path) {
        #[cfg(unix)]
        Ok(info) => {
            use std::os::unix::fs::PermissionsExt;
            mode = info.permissions().mode() & 0o777;
        }
        #[cfg(not(unix))]
        Ok(_) => {}
        Err(e) if e.kind() == ErrorKind::NotFound => {}
        Err(e) => return Err(io_err(format!("inspect config {}", path.display()))(e)),
    }
    crate::util::write_atomic(path, data, mode, ".trello-mcp-config-")
        .map_err(io_err(format!("write config {}", path.display())))
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
    fn setup_project_rejects_trailing_json_content() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join(".mcp.json");
        let original = "{\"mcpServers\":{}}\nkeep me\n";
        fs::write(&path, original).unwrap();
        let err = setup("claude-code", root.path()).unwrap_err().to_string();
        assert!(
            err.contains("cannot parse JSON") && err.contains(".mcp.json"),
            "{err}"
        );
        assert_eq!(fs::read_to_string(&path).unwrap(), original);
    }

    #[test]
    fn setup_project_rejects_invalid_utf8_codex_config() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join(".codex/config.toml");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let original = b"# caf\xe9\n";
        fs::write(&path, original).unwrap();
        let err = setup("codex", root.path()).unwrap_err().to_string();
        assert!(err.contains("parse existing codex config"), "{err}");
        assert_eq!(fs::read(&path).unwrap(), original);
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
            "claude-code config ".to_string()
                + &root.path().join(".mcp.json").display().to_string()
                + ": field \"mcpServers\" is not an object"
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

    fn names(c: &[&str]) -> Vec<String> {
        c.iter().map(|c| c.to_string()).collect()
    }

    #[test]
    fn project_setups_detects_scopes_and_skips_user_files() {
        let (root, home) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let all = names(&["claude-code", "opencode", "codex"]);
        assert!(project_setups(root.path(), home.path(), &[], &all).is_empty());
        let mcp = root.path().join(".mcp.json");
        fs::write(
            &mcp,
            r#"{"mcpServers":{"trello":{"command":"/x/trello-mcp"}}}"#,
        )
        .unwrap();
        fs::write(
            root.path().join("opencode.json"),
            r#"{"mcp":{"trello":{"command":["/y/bin/other"]}}}"#,
        )
        .unwrap();
        // A parsed file without our entry is not a hit, even if it mentions trello-mcp.
        fs::create_dir_all(root.path().join(".codex")).unwrap();
        fs::write(
            root.path().join(".codex/config.toml"),
            "# trello-mcp\nx = 1\n",
        )
        .unwrap();
        assert_eq!(
            project_setups(root.path(), home.path(), &[], &all),
            vec![mcp.clone()]
        );
        // Scoped to the clients this run configured.
        assert!(project_setups(root.path(), home.path(), &[], &names(&["codex"])).is_empty());
        assert_eq!(
            project_setups(root.path(), home.path(), &[], &names(&["claude"])),
            vec![mcp.clone()]
        );
        // A user target written by this run is not a project file.
        assert!(project_setups(root.path(), home.path(), &[mcp], &all).is_empty());
        // A file that does not parse falls back to a text match.
        fs::write(root.path().join(".mcp.json"), "// c\n{\"trello-mcp\"").unwrap();
        assert_eq!(project_setups(root.path(), home.path(), &[], &all).len(), 1);
    }

    #[test]
    fn project_setups_is_quiet_when_folder_is_home() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = dir.path().join(".codex/config.toml");
        fs::create_dir_all(cfg.parent().unwrap()).unwrap();
        fs::write(&cfg, "[mcp_servers.trello]\ncommand = \"/x/trello-mcp\"\n").unwrap();
        fs::write(
            dir.path().join(".mcp.json"),
            r#"{"mcpServers":{"trello":{"command":"trello-mcp"}}}"#,
        )
        .unwrap();
        let all = names(&["codex", "claude-code"]);
        assert!(
            project_setups(dir.path(), dir.path(), std::slice::from_ref(&cfg), &all).is_empty()
        );
        assert!(project_setups(dir.path(), dir.path(), &[], &all).is_empty());
        // Another folder whose project file is the written user file: skipped.
        let other = tempfile::tempdir().unwrap();
        let link = other.path().join(".codex");
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(dir.path().join(".codex"), &link).unwrap();
            assert!(
                project_setups(other.path(), Path::new("/nonexistent"), &[cfg], &all).is_empty()
            );
        }
    }
}
