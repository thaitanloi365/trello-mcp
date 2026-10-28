//! Send to Orca: one worktree per card, or per group of cards, named after them so a card never
//! gets two, branched from what the repo's main folder has checked out, with an agent told to fix
//! the cards, run the tests, open a PR into that branch, merge it and remove the worktree. Send
//! selected starts a planner that groups the cards and sends each group.

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

/// The name and path of Orca's git repo `repo_id`.
async fn repo_of(bin: &str, repo_id: &str) -> Result<(String, String), String> {
    repos(bin)
        .await?
        .iter()
        .find(|r| r["id"] == repo_id)
        .and_then(|r| Some((r["name"].as_str()?.into(), r["path"].as_str()?.into())))
        .ok_or_else(|| format!("no Orca repo {repo_id:?}"))
}

/// Orca's id for one of the page's agents.
fn agent_id(agent: &str) -> Result<&'static str, String> {
    AGENTS
        .iter()
        .find(|(a, _)| *a == agent)
        .map(|(_, id)| *id)
        .ok_or_else(|| format!("unknown agent {agent:?}"))
}

/// Every worktree in every repo, with its repo's name and its agents.
async fn worktrees(bin: &str) -> Result<Vec<Value>, String> {
    // A big limit: a card's worktree cut off the page would get a second one.
    let r = orca(bin, &["worktree", "ps", "--limit", "100000"]).await?;
    Ok(r["worktrees"].as_array().cloned().unwrap_or_default())
}

/// The cards a worktree is for, lowercased: its folder or its branch is `trello-<short link>`, or
/// `trello-<link>-<link>…` for a group, in any case (Orca may lowercase it). Short links are 8
/// letters and digits: "trello-mcp" is a repo.
fn cards_of(wt: &Value) -> Vec<String> {
    ["path", "branch"]
        .iter()
        .find_map(|k| {
            let last = wt[*k].as_str()?.rsplit('/').next()?;
            let (pre, links) = last.split_at_checked(7)?;
            let links: Vec<String> = links.split('-').map(str::to_lowercase).collect();
            (pre.eq_ignore_ascii_case("trello-")
                && links
                    .iter()
                    .all(|l| l.len() == 8 && l.bytes().all(|b| b.is_ascii_alphanumeric())))
            .then_some(links)
        })
        .unwrap_or_default()
}

/// A card worktree as the page shows it, once per card: its repo, its first agent and that
/// agent's state, else the worktree's.
fn sessions_of(wt: &Value) -> Vec<Value> {
    let a = &wt["agents"][0];
    cards_of(wt)
        .into_iter()
        .map(|card| {
            json!({
                "card": card,
                "repo": wt["repo"],
                "name": wt["displayName"],
                "agent": a["agentType"],
                "state": a.get("state").unwrap_or(&wt["status"]),
            })
        })
        .collect()
}

/// The cards being worked on in Orca, in any repo.
pub(crate) async fn sessions(bin: &str) -> Result<Vec<Value>, String> {
    Ok(worktrees(bin).await?.iter().flat_map(sessions_of).collect())
}

/// What the agent is told. One line: a newline could submit a TUI prompt early.
fn prompt(urls: &[String], base: &str) -> String {
    let cards = match urls {
        [one] => format!("the Trello card {one}"),
        _ => format!(
            "the Trello cards {} , one after another in that order, all in this worktree and one pull request",
            urls.join(" ")
        ),
    };
    format!(
        "Fix {cards} . \
         1) Read each card with the Trello MCP tools: description, checklists, attachments and comments. \
         2) Make the change in this worktree; its branch was made from {base}. \
         3) Run the project's tests and fix every failure. \
         4) Commit, push this branch and open a pull request into {base} (gh pr create --base {base}). \
         5) Wait for its checks (gh pr checks --watch) and fix and push until they pass, then merge the pull request yourself with gh pr merge, using a merge method the repo allows. Do not pass --delete-branch: {base} is checked out in another worktree. \
         6) Once it is merged, delete the remote branch if GitHub kept it, then, as the very last step, remove this worktree: orca worktree rm --worktree active --force . \
         If a step fails and you cannot fix it, stop there, keep the worktree and the pull request, and say what is blocking."
    )
}

/// Creates one worktree in `repo_id` for `cards` (short link, title), in that order, with `agent`
/// working through them. Cards that already have a worktree in any repo are skipped: one card, one
/// agent. When all have one, marks the first one's unread in Orca's sidebar and says so.
pub(crate) async fn send(
    bin: &str,
    repo_id: &str,
    agent: &str,
    cards: &[(String, String)],
) -> Result<Value, String> {
    let agent_id = agent_id(agent)?;
    let wts = worktrees(bin).await?;
    let in_orca = |link: &str| {
        wts.iter()
            .find(|w| cards_of(w).contains(&link.to_lowercase()))
    };
    let mut fresh: Vec<&(String, String)> = Vec::new();
    for c in cards {
        if in_orca(&c.0).is_none() && !fresh.iter().any(|f| f.0.eq_ignore_ascii_case(&c.0)) {
            fresh.push(c);
        }
    }
    let skipped: Vec<&str> = cards
        .iter()
        .filter(|(l, _)| in_orca(l).is_some())
        .map(|(l, _)| l.as_str())
        .collect();
    if fresh.is_empty() {
        let (link, wt) = cards
            .first()
            .and_then(|(l, _)| Some((l, in_orca(l)?)))
            .ok_or("no cards")?;
        let id = format!("id:{}", wt["worktreeId"].as_str().unwrap_or_default());
        let _ = orca(bin, &["worktree", "set", "--worktree", &id, "--unread"]).await;
        return Ok(
            json!({"existing": true, "name": format!("trello-{link}"), "repo": wt["repo"], "path": wt["path"]}),
        );
    }
    let links: Vec<&str> = fresh.iter().map(|(l, _)| l.as_str()).collect();
    let name = format!("trello-{}", links.join("-"));
    let (_, path) = repo_of(bin, repo_id).await?;
    let repo = format!("id:{repo_id}");
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
    let urls: Vec<String> = links
        .iter()
        .map(|l| format!("https://trello.com/c/{l}"))
        .collect();
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
            &prompt(&urls, &base),
            "--comment",
            &urls.join(" "),
        ],
    )
    .await?;
    let wt = &created["worktree"];
    let id = format!("id:{}", wt["id"].as_str().unwrap_or_default());
    let title: Vec<&str> = fresh.iter().map(|(_, t)| t.as_str()).collect();
    let _ = orca(
        bin,
        &[
            "worktree",
            "set",
            "--worktree",
            &id,
            "--display-name",
            &title.join(" + "),
        ],
    )
    .await;
    Ok(
        json!({"existing": false, "name": name, "base": base, "agent": agent, "path": wt["path"], "skipped": skipped}),
    )
}

/// Send selected: drops the cards already in Orca, then sends a lone card straight to `send`, or
/// starts a planner on several (see `plan`).
pub(crate) async fn send_selected(
    bin: &str,
    cli: &str,
    repo_id: &str,
    agent: &str,
    cards: &[(String, String)],
) -> Result<Value, String> {
    agent_id(agent)?;
    let busy: Vec<String> = worktrees(bin).await?.iter().flat_map(cards_of).collect();
    let fresh: Vec<(String, String)> = cards
        .iter()
        .filter(|(l, _)| !busy.contains(&l.to_lowercase()))
        .cloned()
        .collect();
    match fresh.len() {
        0 => Err("every picked card is already in Orca".into()),
        1 => send(bin, repo_id, agent, &fresh).await,
        _ => plan(bin, cli, repo_id, agent, &fresh).await,
    }
}

/// A shell word.
fn sh(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// Starts Claude in a terminal in the repo's main folder, told to group `cards` and run
/// `cli orca-send` once per group: one fresh `agent` session each. It may read cards and run that
/// command; anything else asks first, in its terminal.
async fn plan(
    bin: &str,
    cli: &str,
    repo_id: &str,
    agent: &str,
    cards: &[(String, String)],
) -> Result<Value, String> {
    if cli.is_empty() {
        return Err("cannot find this trello-mcp binary for the planner".into());
    }
    let (name, path) = repo_of(bin, repo_id).await?;
    let urls: Vec<String> = cards
        .iter()
        .map(|(l, _)| format!("https://trello.com/c/{l}"))
        .collect();
    let prompt = format!(
        "Group these Trello cards into Orca sessions: {} . Do not change any files here. \
         1) Read each card with the Trello MCP tools (get_card): description, checklists and comments. \
         2) Put cards in one group when one needs another or they change the same screens, data or code. A card that depends on no other is a group of its own. Never put a card in two groups. \
         3) For each group, one group at a time, run: {cli} orca-send --repo {repo_id} --agent {agent} followed by its card links, in the order to build them. Each run starts one fresh agent session for that group, in its own worktree. \
         4) Then list the groups, with one line each on why, and stop.",
        urls.join(" ")
    );
    let command = [
        "claude".into(),
        sh(&prompt),
        "--allowedTools".into(),
        sh("mcp__trello__get_card"),
        sh("mcp__trello__find_checklist_items"),
        sh(&format!("Bash({cli} orca-send *)")),
    ]
    .join(" ");
    let numbers: Vec<&str> = cards
        .iter()
        .map(|(_, t)| t.split(' ').next().unwrap_or_default())
        .collect();
    orca(
        bin,
        &[
            "terminal",
            "create",
            "--worktree",
            &format!("path:{path}"),
            "--title",
            &format!("Group {}", numbers.join(" ")),
            "--command",
            &command,
        ],
    )
    .await?;
    Ok(json!({"planned": true, "cards": cards.len(), "repo": name}))
}

#[cfg(test)]
pub(crate) mod tests {
    use std::{os::unix::fs::PermissionsExt, path::Path};

    use super::*;

    /// A stand-in `orca` in `dir` that logs each call's arguments, one per line, to `dir/log` and
    /// answers `worktree ps` with `dir/worktrees.json`; its one repo, r1, is a git repo on
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
  "worktree ps") cat '{d}/worktrees.json' ;;
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
        let v = send(
            &bin,
            "r1",
            "agy",
            &[card("AbCd1234", "#348 Block physical address")],
        )
        .await
        .unwrap();
        assert_eq!(
            v,
            json!({"existing": false, "name": "trello-AbCd1234", "base": "feature-x", "agent": "agy", "path": "/wt/trello-AbCd1234", "skipped": []})
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

    /// A card as `send` takes it.
    fn card(link: &str, title: &str) -> (String, String) {
        (link.into(), title.into())
    }

    #[test]
    fn a_session_is_a_trello_worktree_with_its_agent() {
        let working = json!({"repo": "app", "displayName": "#1 x", "path": "/wt/trello-AbCd1234",
            "status": "working", "agents": [{"agentType": "claude", "state": "done"}]});
        assert_eq!(
            sessions_of(&working),
            [
                json!({"card": "abcd1234", "repo": "app", "name": "#1 x", "agent": "claude", "state": "done"})
            ]
        );
        let group = json!({"repo": "web", "displayName": "#2 y + #3 z", "path": "/wt/other",
            "branch": "refs/heads/me/TRELLO-Ef56Gh78-Ij90Kl12", "status": "active", "agents": []});
        assert_eq!(
            sessions_of(&group),
            [
                json!({"card": "ef56gh78", "repo": "web", "name": "#2 y + #3 z", "agent": null, "state": "active"}),
                json!({"card": "ij90kl12", "repo": "web", "name": "#2 y + #3 z", "agent": null, "state": "active"}),
            ]
        );
        for other in [
            json!({"path": "/repo", "branch": "refs/heads/feature-x"}),
            json!({"path": "/wt/trello-", "branch": "refs/heads/trello"}),
            json!({"path": "/p/trello-mcp", "branch": "refs/heads/trello-AbCd12345"}),
            json!({"path": "/wt/trello-AbCd1234-fix"}),
            json!({}),
        ] {
            assert_eq!(sessions_of(&other), Vec::<Value>::new(), "{other}");
        }
    }

    #[test]
    fn shell_words_keep_quotes() {
        assert_eq!(sh("it's 'x'"), r#"'it'\''s '\''x'\'''"#);
    }

    #[tokio::test]
    #[ignore = "starts processes, which macOS can freeze after the procs scan test launches a fresh binary: cargo test -- --ignored orca"]
    async fn a_group_shares_one_worktree_and_skips_cards_in_orca() {
        let dir = tempfile::tempdir().unwrap();
        let bin = fake_orca(dir.path());
        std::fs::write(
            dir.path().join("worktrees.json"),
            r#"{"ok":true,"result":{"worktrees":[
                {"worktreeId":"r1::/wt/x","repo":"app","path":"/wt/trello-ef56gh78"}]}}"#,
        )
        .unwrap();
        let v = send(
            &bin,
            "r1",
            "claude",
            &[
                card("AbCd1234", "#1 a"),
                card("Ef56Gh78", "#2 b"),
                card("Ij90Kl12", "#3 c"),
            ],
        )
        .await
        .unwrap();
        assert_eq!(
            (&v["name"], &v["skipped"]),
            (&json!("trello-AbCd1234-Ij90Kl12"), &json!(["Ef56Gh78"])),
            "{v}"
        );
        let calls = logged(dir.path());
        let create = calls
            .iter()
            .find(|c| c[..2] == ["worktree", "create"])
            .unwrap();
        assert_eq!(flag(create, "--name"), "trello-AbCd1234-Ij90Kl12");
        assert_eq!(
            flag(create, "--comment"),
            "https://trello.com/c/AbCd1234 https://trello.com/c/Ij90Kl12"
        );
        let p = flag(create, "--prompt");
        assert!(
            p.starts_with("Fix the Trello cards https://trello.com/c/AbCd1234 https://trello.com/c/Ij90Kl12 , one after another in that order"),
            "{p}"
        );
        let set = calls.iter().find(|c| c[1] == "set").unwrap();
        assert_eq!(flag(set, "--display-name"), "#1 a + #3 c");
    }

    #[tokio::test]
    #[ignore = "starts processes, which macOS can freeze after the procs scan test launches a fresh binary: cargo test -- --ignored orca"]
    async fn send_selected_plans_several_cards_and_sends_a_lone_one() {
        let dir = tempfile::tempdir().unwrap();
        let bin = fake_orca(dir.path());
        std::fs::write(
            dir.path().join("worktrees.json"),
            r#"{"ok":true,"result":{"worktrees":[
                {"worktreeId":"r1::/wt/x","repo":"app","path":"/wt/trello-ef56gh78"}]}}"#,
        )
        .unwrap();
        let cli = "/x/trello-mcp";
        let all_in_orca =
            send_selected(&bin, cli, "r1", "codex", &[card("Ef56Gh78", "#2 b")]).await;
        assert_eq!(
            all_in_orca.unwrap_err(),
            "every picked card is already in Orca"
        );
        let v = send_selected(
            &bin,
            cli,
            "r1",
            "codex",
            &[
                card("AbCd1234", "#1 a"),
                card("Ef56Gh78", "#2 b"),
                card("Ij90Kl12", "#3 c"),
            ],
        )
        .await
        .unwrap();
        assert_eq!(v, json!({"planned": true, "cards": 2, "repo": "app"}));
        let calls = logged(dir.path());
        assert!(
            !calls.iter().any(|c| c[..2] == ["worktree", "create"]),
            "{calls:?}"
        );
        let term = calls
            .iter()
            .find(|c| c[..2] == ["terminal", "create"])
            .unwrap();
        let repo = dir.path().join("repo");
        assert_eq!(flag(term, "--worktree"), format!("path:{}", repo.display()));
        assert_eq!(flag(term, "--title"), "Group #1 #3");
        // The shell reads the command as claude, its prompt, then the allowed tools.
        let command = flag(term, "--command");
        let out = std::process::Command::new("/bin/sh")
            .args(["-c", &command.replacen("claude", r"printf '%s\n'", 1)])
            .output()
            .unwrap();
        let out = String::from_utf8(out.stdout).unwrap();
        let words: Vec<&str> = out.lines().collect();
        assert_eq!(
            words[1..],
            [
                "--allowedTools",
                "mcp__trello__get_card",
                "mcp__trello__find_checklist_items",
                "Bash(/x/trello-mcp orca-send *)"
            ],
            "{command}"
        );
        assert!(
            words[0].starts_with("Group these Trello cards into Orca sessions: https://trello.com/c/AbCd1234 https://trello.com/c/Ij90Kl12 .")
                && words[0].contains("run: /x/trello-mcp orca-send --repo r1 --agent codex followed by"),
            "{}",
            words[0]
        );
        let lone = send_selected(&bin, cli, "r1", "codex", &[card("AbCd1234", "#1 a")])
            .await
            .unwrap();
        assert_eq!(lone["name"], "trello-AbCd1234", "{lone}");
    }

    #[tokio::test]
    #[ignore = "starts processes, which macOS can freeze after the procs scan test launches a fresh binary: cargo test -- --ignored orca"]
    async fn a_card_never_gets_a_second_worktree() {
        let dir = tempfile::tempdir().unwrap();
        let bin = fake_orca(dir.path());
        std::fs::write(
            dir.path().join("worktrees.json"),
            r#"{"ok":true,"result":{"worktrees":[
                {"worktreeId":"r1::/repo","repo":"app","path":"/repo","branch":"refs/heads/feature-x"},
                {"worktreeId":"r2::/wt/other","repo":"web","path":"/wt/other","branch":"refs/heads/me/trello-abcd1234"}]}}"#,
        )
        .unwrap();
        // Not even in another repo.
        let v = send(&bin, "r1", "codex", &[card("AbCd1234", "#1 x")])
            .await
            .unwrap();
        assert_eq!(
            v,
            json!({"existing": true, "name": "trello-AbCd1234", "repo": "web", "path": "/wt/other"})
        );
        let calls = logged(dir.path());
        assert!(!calls.iter().any(|c| c[1] == "create"), "{calls:?}");
        let set = calls.iter().find(|c| c[1] == "set").unwrap();
        assert_eq!(flag(set, "--worktree"), "id:r2::/wt/other");
        assert!(set.contains(&"--unread".to_string()));
    }

    #[tokio::test]
    #[ignore = "starts processes, which macOS can freeze after the procs scan test launches a fresh binary: cargo test -- --ignored orca"]
    async fn send_refuses_unknown_agents_and_repos() {
        let dir = tempfile::tempdir().unwrap();
        let bin = fake_orca(dir.path());
        assert!(
            send(&bin, "r1", "bash", &[card("AbCd1234", "#1 x")])
                .await
                .unwrap_err()
                .contains("unknown agent")
        );
        assert!(logged(dir.path()).is_empty(), "nothing ran");
        for repo in ["nope", "f1"] {
            assert!(
                send(&bin, repo, "claude", &[card("AbCd1234", "#1 x")])
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
