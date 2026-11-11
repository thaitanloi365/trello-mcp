//! The call log: one JSON line per tool call, in a file beside the config. Every server
//! appends to the same file; the dashboard's Logs tab reads it. Arguments other than the card,
//! board or list a call names are never written.

use std::{
    fs,
    io::Write,
    os::unix::fs::OpenOptionsExt,
    path::Path,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use serde_json::{Map, Value, json};

pub(crate) const FILE: &str = "calls.log";
/// A log this large moves to calls.log.1, replacing the one before.
const MAX_BYTES: u64 = 2 << 20;
const MAX_TEXT: usize = 300;

/// The card, board or list a call names.
pub(crate) fn target(args: &Option<Map<String, Value>>) -> String {
    ["cardId", "list", "boardId"]
        .iter()
        .find_map(|name| args.as_ref()?.get(*name)?.as_str())
        .unwrap_or("")
        .to_string()
}

fn short(text: &str) -> String {
    text.chars().take(MAX_TEXT).collect()
}

pub(crate) fn entry(
    client: &str,
    tool: &str,
    target: &str,
    took: Duration,
    error: Option<&str>,
) -> Value {
    let at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    json!({
        "t": at, "client": short(client), "tool": short(tool), "target": short(target),
        "ms": took.as_millis() as u64, "error": error.map(short),
    })
}

/// A failed write is dropped: logging must never fail a tool call.
pub(crate) fn append(path: &Path, entry: &Value) {
    if fs::metadata(path).is_ok_and(|m| m.len() > MAX_BYTES) {
        let _ = fs::rename(path, path.with_extension("log.1"));
    }
    if let Ok(mut file) = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(path)
    {
        // One write per line: appends from several servers do not interleave.
        let _ = file.write_all(format!("{entry}\n").as_bytes());
    }
}

/// The last `limit` calls, newest first; no file is no calls.
pub(crate) fn tail(path: &Path, limit: usize) -> Vec<Value> {
    let text = fs::read_to_string(path).unwrap_or_default();
    text.lines()
        .rev()
        .filter_map(|line| serde_json::from_str(line).ok())
        .take(limit)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn appends_and_tails_newest_first_without_bodies() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE);
        assert!(tail(&path, 10).is_empty());
        let args = json!({"cardId": "abc", "text": "secret body"});
        let args = args.as_object().cloned();
        for (tool, error) in [("get_card", None), ("add_comment", Some("boom"))] {
            append(
                &path,
                &entry(
                    "codex",
                    tool,
                    &target(&args),
                    Duration::from_millis(7),
                    error,
                ),
            );
        }
        let calls = tail(&path, 10);
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0]["tool"], "add_comment");
        assert_eq!(calls[0]["error"], "boom");
        assert_eq!(calls[1]["target"], "abc");
        assert_eq!(calls[1]["error"], Value::Null);
        assert!(!fs::read_to_string(&path).unwrap().contains("secret"));
        assert_eq!(tail(&path, 1).len(), 1);
    }
}
