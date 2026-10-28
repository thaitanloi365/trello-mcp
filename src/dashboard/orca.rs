//! Send to Orca: one worktree per card, named after the card so it never gets two, branched from
//! what the repo's main folder has checked out, with an agent told to fix the card, run the tests,
//! open a PR into that branch, merge it and remove the worktree.

use std::{
    fs::File,
    io::{Read, Seek},
    process::Stdio,
    time::Duration,
};

use serde_json::{Value, json};
use tokio::process::Command;

/// The page's agent names, and Orca's ids for them.
pub(crate) const AGENTS: [(&str, &str); 3] = [
    ("claude", "claude"),
    ("codex", "codex"),
    ("agy", "antigravity"),
];
const TIMEOUT: Duration = Duration::from_secs(60);

/// $ORCA_CLI_COMMAND, else the app's own copy (the tray's PATH lacks it), else `orca` on PATH.
pub(crate) fn cli() -> String {
    if let Ok(c) = std::env::var("ORCA_CLI_COMMAND")
        && !c.is_empty()
    {
        return c;
    }
    let app = "/Applications/Orca.app/Contents/Resources/bin/orca";
    if std::path::Path::new(app).exists() {
        app.into()
    } else {
        "orca".into()
    }
}

/// A command's stdout; `what` names it in errors. Output goes to files, not pipes: a process the
/// command leaves running, or one that inherits the pipe while being spawned, would keep a pipe
/// open, and the read waiting, long after the command exits.
async fn output(cmd: &mut Command, what: &str) -> Result<Vec<u8>, String> {
    let fail = |e: std::io::Error| format!("{what}: {e}");
    let (mut out, mut err) = (
        tempfile::tempfile().map_err(fail)?,
        tempfile::tempfile().map_err(fail)?,
    );
    let status = tokio::time::timeout(
        TIMEOUT,
        cmd.stdin(Stdio::null())
            .stdout(out.try_clone().map_err(fail)?)
            .stderr(err.try_clone().map_err(fail)?)
            .kill_on_drop(true)
            .status(),
    )
    .await
    .map_err(|_| format!("{what} timed out"))?
    .map_err(fail)?;
    let read = |f: &mut File| -> std::io::Result<Vec<u8>> {
        let mut v = Vec::new();
        f.rewind()?;
        f.read_to_end(&mut v)?;
        Ok(v)
    };
    let stdout = read(&mut out).map_err(fail)?;
    // A failed orca --json still prints its error as JSON.
    if !status.success() && stdout.is_empty() {
        let stderr = read(&mut err).map_err(fail)?;
        return Err(format!(
            "{what}: {}",
            String::from_utf8_lossy(&stderr).trim()
        ));
    }
    Ok(stdout)
}

/// The `result` of `orca <args> --json`.
async fn orca(bin: &str, args: &[&str]) -> Result<Value, String> {
    let what = format!("orca {}", args[..2].join(" "));
    let out = output(Command::new(bin).args(args).arg("--json"), &what).await?;
    let v: Value = serde_json::from_slice(&out).map_err(|e| format!("{what}: {e}"))?;
    if v["ok"] == true {
        return Ok(v["result"].clone());
    }
    let e = &v["error"];
    Err(format!(
        "{what}: {}",
        e["message"]
            .as_str()
            .or(e["code"].as_str())
            .unwrap_or("failed")
    ))
}

/// Orca's git repos: id, name, path.
pub(crate) async fn repos(bin: &str) -> Result<Vec<Value>, String> {
    let r = orca(bin, &["repo", "list"]).await?;
    Ok(r["repos"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|r| r["kind"] == "git")
        .map(|r| json!({"id": r["id"], "name": r["displayName"], "path": r["path"]}))
        .collect())
}

/// This card's worktree: its folder or its branch carries the name, in any case (Orca may
/// lowercase it).
fn is_named(wt: &Value, name: &str) -> bool {
    let last = |k: &str| {
        wt[k]
            .as_str()
            .and_then(|s| s.rsplit('/').next())
            .is_some_and(|s| s.eq_ignore_ascii_case(name))
    };
    last("path") || last("branch")
}

/// What the agent is told. One line: a newline could submit a TUI prompt early.
fn prompt(url: &str, base: &str) -> String {
    format!(
        "Fix the Trello card {url} . \
         1) Read the card with the Trello MCP tools: description, checklists, attachments and comments. \
         2) Make the change in this worktree; its branch was made from {base}. \
         3) Run the project's tests and fix every failure. \
         4) Commit, push this branch and open a pull request into {base} (gh pr create --base {base}). \
         5) Wait for its checks (gh pr checks --watch) and fix and push until they pass, then merge the pull request yourself with gh pr merge, using a merge method the repo allows. Do not pass --delete-branch: {base} is checked out in another worktree. \
         6) Once it is merged, delete the remote branch if GitHub kept it, then, as the very last step, remove this worktree: orca worktree rm --worktree active --force . \
         If a step fails and you cannot fix it, stop there, keep the worktree and the pull request, and say what is blocking."
    )
}

/// Creates the card's worktree in `repo_id` with `agent` working in it, or, when the card already
/// has one there, marks that one unread in Orca's sidebar and says so. `link` is the card's short
/// link; `title` its Orca display name.
pub(crate) async fn send(
    bin: &str,
    repo_id: &str,
    agent: &str,
    link: &str,
    title: &str,
) -> Result<Value, String> {
    let agent_id = AGENTS
        .iter()
        .find(|(a, _)| *a == agent)
        .map(|(_, id)| *id)
        .ok_or_else(|| format!("unknown agent {agent:?}"))?;
    let path = repos(bin)
        .await?
        .iter()
        .find(|r| r["id"] == repo_id)
        .and_then(|r| r["path"].as_str().map(str::to_string))
        .ok_or_else(|| format!("no Orca repo {repo_id:?}"))?;
    let name = format!("trello-{link}");
    let repo = format!("id:{repo_id}");
    let list = orca(bin, &["worktree", "list", "--repo", &repo]).await?;
    if let Some(wt) = list["worktrees"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|w| is_named(w, &name))
    {
        let id = format!("id:{}", wt["id"].as_str().unwrap_or_default());
        let _ = orca(bin, &["worktree", "set", "--worktree", &id, "--unread"]).await;
        return Ok(json!({"existing": true, "name": name, "path": wt["path"]}));
    }
    let base = output(
        Command::new("git").args(["-C", &path, "branch", "--show-current"]),
        "git branch",
    )
    .await?;
    let base = String::from_utf8_lossy(&base).trim().to_string();
    if base.is_empty() {
        return Err(format!(
            "{path} has no branch checked out: check one out to build on"
        ));
    }
    let url = format!("https://trello.com/c/{link}");
    let created = orca(
        bin,
        &[
            "worktree",
            "create",
            "--repo",
            &repo,
            "--name",
            &name,
            "--base-branch",
            &base,
            "--no-parent",
            "--agent",
            agent_id,
            "--prompt",
            &prompt(&url, &base),
            "--comment",
            &url,
        ],
    )
    .await?;
    let wt = &created["worktree"];
    let id = format!("id:{}", wt["id"].as_str().unwrap_or_default());
    let _ = orca(
        bin,
        &[
            "worktree",
            "set",
            "--worktree",
            &id,
            "--display-name",
            title,
        ],
    )
    .await;
    Ok(json!({"existing": false, "name": name, "base": base, "agent": agent, "path": wt["path"]}))
}

#[cfg(test)]
pub(crate) mod tests {
    use std::{os::unix::fs::PermissionsExt, path::Path};

    use super::*;

    /// A stand-in `orca` in `dir` that logs each call's arguments, one per line, to `dir/log` and
    /// answers `worktree list` with `dir/worktrees.json`; its one repo, r1, is a git repo on
    /// branch feature-x.
    pub(crate) fn fake_orca(dir: &Path) -> String {
        let repo = dir.join("repo");
        let git = std::process::Command::new("git")
            .args(["init", "-q", "-b", "feature-x"])
            .arg(&repo)
            .status()
            .unwrap();
        assert!(git.success());
        std::fs::write(
            dir.join("worktrees.json"),
            r#"{"ok":true,"result":{"worktrees":[]}}"#,
        )
        .unwrap();
        let d = dir.display();
        let script = format!(
            r#"#!/bin/sh
printf '%s\n' "$@" >> '{d}/log'; echo --- >> '{d}/log'
case "$1 $2" in
  "repo list") echo '{{"ok":true,"result":{{"repos":[{{"id":"r1","kind":"git","displayName":"app","path":"{repo}"}},{{"id":"f1","kind":"folder","displayName":"notes","path":"/n"}}]}}}}' ;;
  "worktree list") cat '{d}/worktrees.json' ;;
  "worktree create") echo '{{"ok":true,"result":{{"worktree":{{"id":"r1::/wt/trello-AbCd1234","path":"/wt/trello-AbCd1234"}}}}}}' ;;
  *) echo '{{"ok":true,"result":{{}}}}' ;;
esac
"#,
            repo = repo.display()
        );
        let bin = dir.join("orca");
        std::fs::write(&bin, script).unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        bin.to_string_lossy().into_owned()
    }

    /// Each logged call, as its arguments.
    pub(crate) fn logged(dir: &Path) -> Vec<Vec<String>> {
        std::fs::read_to_string(dir.join("log"))
            .unwrap_or_default()
            .split("---\n")
            .filter(|c| !c.is_empty())
            .map(|c| c.lines().map(str::to_string).collect())
            .collect()
    }

    fn flag<'a>(call: &'a [String], name: &str) -> &'a str {
        let i = call.iter().position(|a| a == name).unwrap();
        &call[i + 1]
    }

    #[tokio::test]
    #[ignore = "starts processes, which macOS can freeze after the procs scan test launches a fresh binary: cargo test -- --ignored orca"]
    async fn send_creates_a_worktree_named_after_the_card() {
        let dir = tempfile::tempdir().unwrap();
        let bin = fake_orca(dir.path());
        let v = send(&bin, "r1", "agy", "AbCd1234", "#348 Block physical address")
            .await
            .unwrap();
        assert_eq!(
            v,
            json!({"existing": false, "name": "trello-AbCd1234", "base": "feature-x", "agent": "agy", "path": "/wt/trello-AbCd1234"})
        );
        let calls = logged(dir.path());
        let create = calls
            .iter()
            .find(|c| c[..2] == ["worktree", "create"])
            .unwrap();
        assert_eq!(flag(create, "--repo"), "id:r1");
        assert_eq!(flag(create, "--name"), "trello-AbCd1234");
        assert_eq!(flag(create, "--base-branch"), "feature-x");
        assert_eq!(flag(create, "--agent"), "antigravity");
        assert!(create.contains(&"--no-parent".to_string()));
        let p = flag(create, "--prompt");
        assert!(
            p.contains("https://trello.com/c/AbCd1234")
                && p.contains("tests")
                && p.contains("gh pr create --base feature-x")
                && p.contains("gh pr merge")
                && p.contains("orca worktree rm --worktree active"),
            "{p}"
        );
        let set = calls
            .iter()
            .find(|c| c[..2] == ["worktree", "set"])
            .unwrap();
        assert_eq!(
            (flag(set, "--worktree"), flag(set, "--display-name")),
            ("id:r1::/wt/trello-AbCd1234", "#348 Block physical address")
        );
    }

    #[tokio::test]
    #[ignore = "starts processes, which macOS can freeze after the procs scan test launches a fresh binary: cargo test -- --ignored orca"]
    async fn a_card_never_gets_a_second_worktree() {
        let dir = tempfile::tempdir().unwrap();
        let bin = fake_orca(dir.path());
        std::fs::write(
            dir.path().join("worktrees.json"),
            r#"{"ok":true,"result":{"worktrees":[
                {"id":"r1::/repo","path":"/repo","branch":"refs/heads/feature-x"},
                {"id":"r1::/wt/other","path":"/wt/other","branch":"refs/heads/me/trello-abcd1234"}]}}"#,
        )
        .unwrap();
        let v = send(&bin, "r1", "codex", "AbCd1234", "#1 x").await.unwrap();
        assert_eq!(
            v,
            json!({"existing": true, "name": "trello-AbCd1234", "path": "/wt/other"})
        );
        let calls = logged(dir.path());
        assert!(!calls.iter().any(|c| c[1] == "create"), "{calls:?}");
        let set = calls.iter().find(|c| c[1] == "set").unwrap();
        assert_eq!(flag(set, "--worktree"), "id:r1::/wt/other");
        assert!(set.contains(&"--unread".to_string()));
    }

    #[tokio::test]
    #[ignore = "starts processes, which macOS can freeze after the procs scan test launches a fresh binary: cargo test -- --ignored orca"]
    async fn send_refuses_unknown_agents_and_repos() {
        let dir = tempfile::tempdir().unwrap();
        let bin = fake_orca(dir.path());
        assert!(
            send(&bin, "r1", "bash", "AbCd1234", "#1 x")
                .await
                .unwrap_err()
                .contains("unknown agent")
        );
        assert!(logged(dir.path()).is_empty(), "nothing ran");
        for repo in ["nope", "f1"] {
            assert!(
                send(&bin, repo, "claude", "AbCd1234", "#1 x")
                    .await
                    .unwrap_err()
                    .contains("no Orca repo"),
                "{repo}: folders are not git repos"
            );
        }
        assert!(!logged(dir.path()).iter().any(|c| c[1] == "create"));
    }

    #[tokio::test]
    #[ignore = "starts processes, which macOS can freeze after the procs scan test launches a fresh binary: cargo test -- --ignored orca"]
    async fn orca_errors_are_reported() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("orca");
        std::fs::write(
            &bin,
            "#!/bin/sh\necho '{\"ok\":false,\"error\":{\"code\":\"not_running\",\"message\":\"Orca is not running\"}}'; exit 1\n",
        )
        .unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(
            repos(bin.to_str().unwrap()).await.unwrap_err(),
            "orca repo list: Orca is not running"
        );
        let missing = dir.path().join("none").to_string_lossy().into_owned();
        assert!(
            repos(&missing)
                .await
                .unwrap_err()
                .starts_with("orca repo list: ")
        );
    }

    #[tokio::test]
    #[ignore = "starts processes, which macOS can freeze after the procs scan test launches a fresh binary: cargo test -- --ignored orca"]
    async fn a_process_left_running_does_not_hold_the_answer() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("orca");
        std::fs::write(
            &bin,
            "#!/bin/sh\n/bin/sleep 5 &\necho '{\"ok\":true,\"result\":{\"repos\":[]}}'\n",
        )
        .unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        let t = std::time::Instant::now();
        assert_eq!(
            repos(bin.to_str().unwrap()).await.unwrap(),
            Vec::<Value>::new()
        );
        assert!(t.elapsed() < Duration::from_secs(3), "{:?}", t.elapsed());
    }
}
