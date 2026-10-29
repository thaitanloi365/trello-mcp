//! Send to Orca: one worktree per card, or per group of cards, named after them so a card never
//! gets two, branched from what the repo's main folder has checked out, with an agent in bypass
//! mode told to fix the cards (a prompt for snags, features or spikes, from their labels), run the
//! tests, open a PR into that branch and, once the user has reviewed it and confirms, merge it and
//! remove the branch and worktree. Each card's session is saved in its own file. Send selected starts a planner that groups the cards and sends each group.

use std::{
    fs::File,
    io::{Read, Seek},
    path::Path,
    process::Stdio,
    time::Duration,
};

use serde_json::{Value, json};
use tokio::process::Command;

/// The page's agent names, and how each starts in bypass mode with the prompt as its last
/// argument: Orca's `worktree create --agent` cannot pass flags.
pub(crate) const AGENTS: [(&str, &str); 3] = [
    ("claude", "claude --dangerously-skip-permissions"),
    ("codex", "codex --dangerously-bypass-approvals-and-sandbox"),
    (
        "agy",
        "agy --dangerously-skip-permissions --prompt-interactive",
    ),
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

/// The bypass-mode command for one of the page's agents.
fn launch(agent: &str) -> Result<&'static str, String> {
    AGENTS
        .iter()
        .find(|(a, _)| *a == agent)
        .map(|(_, cmd)| *cmd)
        .ok_or_else(|| format!("unknown agent {agent:?}"))
}

/// What a card asks for, from its labels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Kind {
    Snag,
    Feature,
    Spike,
    Other,
}

impl Kind {
    /// A spike, else a snag or bug, else a feature, enhancement or change request.
    pub(crate) fn of<'a>(labels: impl IntoIterator<Item = &'a str>) -> Kind {
        let labels: Vec<String> = labels.into_iter().map(str::to_lowercase).collect();
        let has = |w: &str| labels.iter().any(|l| l.contains(w));
        if has("spike") {
            Kind::Spike
        } else if has("snag") || has("bug") {
            Kind::Snag
        } else if has("feature") || has("enhancement") || has("change request") {
            Kind::Feature
        } else {
            Kind::Other
        }
    }

    /// How the agent is told to work on a card of this kind.
    fn hint(self) -> Option<&'static str> {
        match self {
            Kind::Snag => Some(
                "a snag (a bug): reproduce it first, find the root cause, add a test that fails without the fix, then make the smallest fix",
            ),
            Kind::Feature => Some(
                "a feature: build what its description, checklists and acceptance criteria ask, no more, with tests for the new behavior",
            ),
            Kind::Spike => Some(
                "a spike: investigate it and report the findings, the options and your recommendation, with no code change for it",
            ),
            Kind::Other => None,
        }
    }
}

/// A card as Orca gets it: its short link, `#<number> <name>`, and its kind.
#[derive(Clone, Debug)]
pub(crate) struct Card {
    pub(crate) link: String,
    pub(crate) title: String,
    pub(crate) kind: Kind,
}

fn card_url(link: &str) -> String {
    format!("https://trello.com/c/{link}")
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
                "id": wt["worktreeId"],
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

/// In Orca: shows the worktree of the card `link` on its first agent's terminal, else its first
/// terminal, and brings Orca to the front.
pub(crate) async fn focus(bin: &str, link: &str) -> Result<Value, String> {
    let wts = worktrees(bin).await?;
    let wt = wts
        .iter()
        .find(|w| cards_of(w).contains(&link.to_lowercase()))
        .ok_or_else(|| format!("{link} is not in Orca"))?;
    let id = format!("id:{}", wt["worktreeId"].as_str().unwrap_or_default());
    let terms = orca(bin, &["terminal", "list", "--worktree", &id]).await?;
    let terms = terms["terminals"].as_array().cloned().unwrap_or_default();
    // An agent's pane key is its terminal's `<tabId>:<leafId>`.
    let pane = wt["agents"][0]["paneKey"]
        .as_str()
        .and_then(|k| k.split_once(':'));
    let term = terms
        .iter()
        .find(|t| pane.is_some_and(|(tab, leaf)| t["tabId"] == tab && t["leafId"] == leaf))
        .or(terms.first())
        .and_then(|t| t["handle"].as_str())
        .ok_or_else(|| {
            let name = wt["displayName"].as_str().unwrap_or(link);
            format!("{name} has no terminal open in Orca")
        })?;
    orca(bin, &["terminal", "switch", "--terminal", term]).await?;
    // shortcut: an `orca` outside an app bundle (on PATH) leaves Orca where it is; Orca.app's own
    // copy is the default.
    if let Some((app, _)) = bin.split_once(".app/") {
        let _ = output(
            Command::new("open").args(["-a", &format!("{app}.app")]),
            "open",
        )
        .await;
    }
    Ok(json!({"name": wt["displayName"], "repo": wt["repo"]}))
}

/// What the agent is told: research for spikes only, else fix, with a hint per snag, feature or
/// spike. One line: a newline could submit a TUI prompt early.
fn prompt(cards: &[&Card], base: &str) -> String {
    let urls: Vec<String> = cards.iter().map(|c| card_url(&c.link)).collect();
    let read = "1) Read each card with the Trello MCP tools: description, checklists, attachments and comments.";
    let release = "Never release a new version (make release, make release-ios, make release-android or any other release) unless the user asks.";
    if cards.iter().all(|c| c.kind == Kind::Spike) {
        let what = match urls.as_slice() {
            [one] => format!("the Trello card {one}"),
            _ => format!(
                "the Trello cards {} , one after another in that order",
                urls.join(" ")
            ),
        };
        return format!(
            "Research {what} . Spikes: answer their questions without changing the code: no commit, no push, no pull request. {release} \
             {read} \
             2) Investigate in this worktree: read the code and run experiments, undoing any change they make. \
             3) Report what you found, the options with their trade-offs and your recommendation, then stop and keep this worktree."
        );
    }
    let cards_text = match urls.as_slice() {
        [one] => format!("the Trello card {one}"),
        _ => format!(
            "the Trello cards {} , one after another in that order, all in this worktree and one pull request",
            urls.join(" ")
        ),
    };
    let hints: String = cards
        .iter()
        .filter_map(|c| {
            let h = c.kind.hint()?;
            Some(match cards {
                [_] => format!("It is {h}. "),
                _ => format!("{} is {h}. ", card_url(&c.link)),
            })
        })
        .collect();
    format!(
        "Fix {cards_text} . {hints}{release} \
         {read} \
         2) Make the change in this worktree; its branch was made from {base}. \
         3) Run the project's tests and fix every failure. \
         4) Commit, push this branch and open a pull request into {base} (gh pr create --base {base}). \
         5) Wait for its checks (gh pr checks --watch) and fix and push until they pass. \
         6) Then stop and ask the user to review the pull request: do not merge it, and keep this worktree and its branch, local and remote, until the user confirms with ok. If they ask for changes instead, make them, push and ask again. \
         7) Only after the user confirms, merge the pull request with gh pr merge, using a merge method the repo allows. Do not pass --delete-branch: {base} is checked out in another worktree. \
         8) Once it is merged, delete the remote branch if GitHub kept it, then, as the very last step, remove this worktree: orca worktree rm --worktree active --force . \
         If a step fails and you cannot fix it, stop there, keep the worktree and the pull request, and say what is blocking."
    )
}

/// Saves `session` as each of `links`' Orca session, one file per card in `dir`: the dashboard
/// and the planner's `orca-send` may write at once.
fn save_session(dir: &Path, links: &[&str], session: &Value) -> Result<(), String> {
    let fail = |e: std::io::Error| format!("save the Orca session in {}: {e}", dir.display());
    std::fs::create_dir_all(dir).map_err(fail)?;
    let data = serde_json::to_vec_pretty(session).map_err(|e| e.to_string())?;
    for link in links {
        let file = dir.join(format!("{}.json", link.to_lowercase()));
        crate::util::write_atomic(&file, &data, 0o600, ".orca-session").map_err(fail)?;
    }
    Ok(())
}

/// Creates one worktree in `repo_id` for `cards`, in that order, with `agent` working through them
/// in bypass mode, and saves the session in `saved` for each card. Cards that already have a
/// worktree in any repo are skipped: one card, one agent. When all have one, marks the first one's
/// unread in Orca's sidebar and says so.
pub(crate) async fn send(
    bin: &str,
    repo_id: &str,
    agent: &str,
    cards: &[Card],
    saved: &Path,
) -> Result<Value, String> {
    let launch = launch(agent)?;
    let wts = worktrees(bin).await?;
    let in_orca = |link: &str| {
        wts.iter()
            .find(|w| cards_of(w).contains(&link.to_lowercase()))
    };
    let mut fresh: Vec<&Card> = Vec::new();
    for c in cards {
        if in_orca(&c.link).is_none() && !fresh.iter().any(|f| f.link.eq_ignore_ascii_case(&c.link))
        {
            fresh.push(c);
        }
    }
    let skipped: Vec<&str> = cards
        .iter()
        .filter(|c| in_orca(&c.link).is_some())
        .map(|c| c.link.as_str())
        .collect();
    if fresh.is_empty() {
        let (link, wt) = cards
            .first()
            .and_then(|c| Some((&c.link, in_orca(&c.link)?)))
            .ok_or("no cards")?;
        let id = format!("id:{}", wt["worktreeId"].as_str().unwrap_or_default());
        let _ = orca(bin, &["worktree", "set", "--worktree", &id, "--unread"]).await;
        return Ok(
            json!({"existing": true, "name": format!("trello-{link}"), "repo": wt["repo"], "path": wt["path"]}),
        );
    }
    let links: Vec<&str> = fresh.iter().map(|c| c.link.as_str()).collect();
    let name = format!("trello-{}", links.join("-"));
    let (repo_name, path) = repo_of(bin, repo_id).await?;
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
    let urls: Vec<String> = links.iter().map(|l| card_url(l)).collect();
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
            "--comment",
            &urls.join(" "),
        ],
    )
    .await?;
    let wt = &created["worktree"];
    let wt_id = wt["id"].as_str().unwrap_or_default();
    let id = format!("id:{wt_id}");
    let title: Vec<&str> = fresh.iter().map(|c| c.title.as_str()).collect();
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
    let command = format!("{launch} {}", sh(&prompt(&fresh, &base)));
    let term = orca(
        bin,
        &[
            "terminal",
            "create",
            "--worktree",
            &id,
            "--command",
            &command,
        ],
    )
    .await
    .map_err(|e| format!("{name} was made, but its agent did not start: {e}"))?;
    let mut v = json!({"existing": false, "name": name, "base": base, "agent": agent,
        "path": wt["path"], "skipped": skipped, "worktree": wt_id,
        "terminal": term["terminal"]["handle"]});
    let session = json!({"worktree": wt_id, "terminal": v["terminal"], "repo": repo_name,
        "name": name, "base": base, "agent": agent, "path": wt["path"], "cards": urls,
        "sentAt": super::now() as i64});
    // The agent is already on it: a save that fails is reported, not a failed send.
    if let Err(e) = save_session(saved, &links, &session) {
        v["warning"] = json!(e);
    }
    Ok(v)
}

/// Send selected: drops the cards already in Orca, then sends a lone card straight to `send`, or
/// starts a planner on several (see `plan`).
pub(crate) async fn send_selected(
    bin: &str,
    cli: &str,
    repo_id: &str,
    agent: &str,
    cards: &[Card],
    saved: &Path,
) -> Result<Value, String> {
    launch(agent)?;
    let busy: Vec<String> = worktrees(bin).await?.iter().flat_map(cards_of).collect();
    let fresh: Vec<Card> = cards
        .iter()
        .filter(|c| !busy.contains(&c.link.to_lowercase()))
        .cloned()
        .collect();
    match fresh.len() {
        0 => Err("every picked card is already in Orca".into()),
        1 => send(bin, repo_id, agent, &fresh, saved).await,
        _ => plan(bin, cli, repo_id, agent, &fresh).await,
    }
}

/// A shell word.
fn sh(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// Starts Claude in bypass mode in a terminal in the repo's main folder, told to group `cards`
/// and run `cli orca-send` once per group: one fresh `agent` session each.
async fn plan(
    bin: &str,
    cli: &str,
    repo_id: &str,
    agent: &str,
    cards: &[Card],
) -> Result<Value, String> {
    if cli.is_empty() {
        return Err("cannot find this trello-mcp binary for the planner".into());
    }
    let (name, path) = repo_of(bin, repo_id).await?;
    let urls: Vec<String> = cards.iter().map(|c| card_url(&c.link)).collect();
    let prompt = format!(
        "Group these Trello cards into Orca sessions: {} . Do not change any files here. \
         1) Read each card with the Trello MCP tools (get_card): description, checklists and comments. \
         2) Put cards in one group when one needs another or they change the same screens, data or code. A card that depends on no other is a group of its own. Never put a card in two groups. \
         3) For each group, one group at a time, run: {cli} orca-send --repo {repo_id} --agent {agent} followed by its card links, in the order to build them. Each run starts one fresh agent session for that group, in its own worktree. \
         4) Then list the groups, with one line each on why, and stop.",
        urls.join(" ")
    );
    let command = format!("{} {}", launch("claude")?, sh(&prompt));
    let numbers: Vec<&str> = cards
        .iter()
        .map(|c| c.title.split(' ').next().unwrap_or_default())
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
    /// answers `worktree ps` with `dir/worktrees.json` and `terminal list` with
    /// `dir/terminals.json`; its one repo, r1, is a git repo on branch feature-x. Sessions are
    /// saved in `dir/saved`.
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
        std::fs::write(
            dir.join("terminals.json"),
            r#"{"ok":true,"result":{"terminals":[]}}"#,
        )
        .unwrap();
        let d = dir.display();
        let script = format!(
            r#"#!/bin/sh
printf '%s\n' "$@" >> '{d}/log'; echo --- >> '{d}/log'
case "$1 $2" in
  "repo list") echo '{{"ok":true,"result":{{"repos":[{{"id":"r1","kind":"git","displayName":"app","path":"{repo}"}},{{"id":"f1","kind":"folder","displayName":"notes","path":"/n"}}]}}}}' ;;
  "worktree ps") cat '{d}/worktrees.json' ;;
  "terminal list") cat '{d}/terminals.json' ;;
  "worktree create") echo '{{"ok":true,"result":{{"worktree":{{"id":"r1::/wt/trello-AbCd1234","path":"/wt/trello-AbCd1234"}}}}}}' ;;
  "terminal create") echo '{{"ok":true,"result":{{"terminal":{{"handle":"term_1"}}}}}}' ;;
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
        let saved = dir.path().join("saved");
        let v = send(
            &bin,
            "r1",
            "agy",
            &[card("AbCd1234", "#348 Block physical address")],
            &saved,
        )
        .await
        .unwrap();
        assert_eq!(
            v,
            json!({"existing": false, "name": "trello-AbCd1234", "base": "feature-x", "agent": "agy",
                "path": "/wt/trello-AbCd1234", "skipped": [], "worktree": "r1::/wt/trello-AbCd1234",
                "terminal": "term_1"})
        );
        let calls = logged(dir.path());
        let create = calls
            .iter()
            .find(|c| c[..2] == ["worktree", "create"])
            .unwrap();
        assert_eq!(flag(create, "--repo"), "id:r1");
        assert_eq!(flag(create, "--name"), "trello-AbCd1234");
        assert_eq!(flag(create, "--base-branch"), "feature-x");
        assert!(create.contains(&"--no-parent".to_string()));
        assert!(!create.contains(&"--agent".to_string()), "{create:?}");
        let set = calls
            .iter()
            .find(|c| c[..2] == ["worktree", "set"])
            .unwrap();
        assert_eq!(
            (flag(set, "--worktree"), flag(set, "--display-name")),
            ("id:r1::/wt/trello-AbCd1234", "#348 Block physical address")
        );
        // The agent starts in the new worktree, in bypass mode, with the prompt as one word.
        let term = calls
            .iter()
            .find(|c| c[..2] == ["terminal", "create"])
            .unwrap();
        assert_eq!(flag(term, "--worktree"), "id:r1::/wt/trello-AbCd1234");
        let words = shell_words(flag(term, "--command"));
        assert_eq!(
            words[..3],
            [
                "agy",
                "--dangerously-skip-permissions",
                "--prompt-interactive"
            ]
        );
        let p = &words[3];
        assert!(
            words.len() == 4
                && p.contains("https://trello.com/c/AbCd1234")
                && p.contains("tests")
                && p.contains("gh pr create --base feature-x")
                && p.contains("gh pr merge")
                && p.contains("do not merge it")
                && p.contains("until the user confirms")
                && p.contains("orca worktree rm --worktree active"),
            "{words:?}"
        );
        let session: Value =
            serde_json::from_slice(&std::fs::read(saved.join("abcd1234.json")).unwrap()).unwrap();
        assert_eq!(
            (
                &session["worktree"],
                &session["terminal"],
                &session["repo"],
                &session["agent"]
            ),
            (
                &json!("r1::/wt/trello-AbCd1234"),
                &json!("term_1"),
                &json!("app"),
                &json!("agy")
            ),
            "{session}"
        );
    }

    /// A card as `send` takes it, with no label that picks a prompt.
    fn card(link: &str, title: &str) -> Card {
        Card {
            link: link.into(),
            title: title.into(),
            kind: Kind::Other,
        }
    }

    /// The words the shell reads `command` as.
    fn shell_words(command: &str) -> Vec<String> {
        let out = std::process::Command::new("/bin/sh")
            .args(["-c", &format!("printf '%s\\n' {command}")])
            .output()
            .unwrap();
        String::from_utf8(out.stdout)
            .unwrap()
            .lines()
            .map(str::to_string)
            .collect()
    }

    #[test]
    fn labels_pick_the_prompt() {
        assert_eq!(Kind::of(["Snag", "UI/UX"]), Kind::Snag);
        assert_eq!(Kind::of(["Bug"]), Kind::Snag);
        assert_eq!(Kind::of(["Feature/Enhancement", "MVP"]), Kind::Feature);
        assert_eq!(Kind::of(["Change Request"]), Kind::Feature);
        assert_eq!(Kind::of(["SPIKE", "Feature/Enhancement"]), Kind::Spike);
        assert_eq!(Kind::of(["Sync", "Client Feedback"]), Kind::Other);
        assert_eq!(Kind::of([]), Kind::Other);

        let with = |link: &str, kind| Card {
            kind,
            ..card(link, "#1 x")
        };
        let snag = with("AbCd1234", Kind::Snag);
        let p = prompt(&[&snag], "dev");
        assert!(
            p.starts_with("Fix the Trello card https://trello.com/c/AbCd1234 . It is a snag (a bug): reproduce it first")
                && p.contains("gh pr merge")
                && p.contains("Never release a new version (make release, make release-ios, make release-android"),
            "{p}"
        );
        let plain = prompt(&[&card("AbCd1234", "#1 x")], "dev");
        assert!(
            plain.starts_with("Fix the Trello card https://trello.com/c/AbCd1234 . Never release"),
            "{plain}"
        );

        let spike = with("Ef56Gh78", Kind::Spike);
        let p = prompt(&[&spike], "dev");
        assert!(
            p.starts_with(
                "Research the Trello card https://trello.com/c/Ef56Gh78 . Spikes: answer"
            ) && !p.contains("gh pr")
                && p.contains("keep this worktree")
                && p.contains("Never release"),
            "{p}"
        );
        // A spike in a group with code to change: the group still ships, the spike gets a note.
        let p = prompt(&[&snag, &spike], "dev");
        assert!(
            p.starts_with("Fix the Trello cards https://trello.com/c/AbCd1234 https://trello.com/c/Ef56Gh78 , one after another")
                && p.contains("https://trello.com/c/AbCd1234 is a snag")
                && p.contains("https://trello.com/c/Ef56Gh78 is a spike")
                && p.contains("gh pr merge"),
            "{p}"
        );
        assert!(
            !p.contains('\n'),
            "one line: a newline could submit a TUI prompt early"
        );
    }

    #[test]
    fn a_session_is_a_trello_worktree_with_its_agent() {
        let working = json!({"repo": "app", "displayName": "#1 x", "path": "/wt/trello-AbCd1234",
            "status": "working", "agents": [{"agentType": "claude", "state": "done"}]});
        assert_eq!(
            sessions_of(&working),
            [
                json!({"card": "abcd1234", "id": null, "repo": "app", "name": "#1 x", "agent": "claude", "state": "done"})
            ]
        );
        let group = json!({"worktreeId": "r2::/wt/other", "repo": "web", "displayName": "#2 y + #3 z", "path": "/wt/other",
            "branch": "refs/heads/me/TRELLO-Ef56Gh78-Ij90Kl12", "status": "active", "agents": []});
        assert_eq!(
            sessions_of(&group),
            [
                json!({"card": "ef56gh78", "id": "r2::/wt/other", "repo": "web", "name": "#2 y + #3 z", "agent": null, "state": "active"}),
                json!({"card": "ij90kl12", "id": "r2::/wt/other", "repo": "web", "name": "#2 y + #3 z", "agent": null, "state": "active"}),
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
            &dir.path().join("saved"),
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
        let term = calls
            .iter()
            .find(|c| c[1] == "create" && c[0] == "terminal")
            .unwrap();
        let words = shell_words(flag(term, "--command"));
        assert_eq!(words[..2], ["claude", "--dangerously-skip-permissions"]);
        assert!(
            words[2].starts_with("Fix the Trello cards https://trello.com/c/AbCd1234 https://trello.com/c/Ij90Kl12 , one after another in that order"),
            "{words:?}"
        );
        let set = calls.iter().find(|c| c[1] == "set").unwrap();
        assert_eq!(flag(set, "--display-name"), "#1 a + #3 c");
        let saved = dir.path().join("saved");
        assert!(saved.join("abcd1234.json").exists() && saved.join("ij90kl12.json").exists());
        assert!(
            !saved.join("ef56gh78.json").exists(),
            "a skipped card keeps its session"
        );
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
        let saved = dir.path().join("saved");
        let all_in_orca = send_selected(
            &bin,
            cli,
            "r1",
            "codex",
            &[card("Ef56Gh78", "#2 b")],
            &saved,
        )
        .await;
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
            &saved,
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
        // The shell reads the command as claude in bypass mode, then its prompt.
        let words = shell_words(flag(term, "--command"));
        assert_eq!(words[..2], ["claude", "--dangerously-skip-permissions"]);
        assert_eq!(words.len(), 3, "{words:?}");
        assert!(
            words[2].starts_with("Group these Trello cards into Orca sessions: https://trello.com/c/AbCd1234 https://trello.com/c/Ij90Kl12 .")
                && words[2].contains("run: /x/trello-mcp orca-send --repo r1 --agent codex followed by"),
            "{}",
            words[2]
        );
        let lone = send_selected(
            &bin,
            cli,
            "r1",
            "codex",
            &[card("AbCd1234", "#1 a")],
            &saved,
        )
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
        let v = send(&bin, "r1", "codex", &[card("AbCd1234", "#1 x")], dir.path())
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
    async fn in_orca_shows_the_cards_agent_terminal() {
        let dir = tempfile::tempdir().unwrap();
        let bin = fake_orca(dir.path());
        let write = |file: &str, body: &str| std::fs::write(dir.path().join(file), body).unwrap();
        write(
            "worktrees.json",
            r##"{"ok":true,"result":{"worktrees":[
                {"worktreeId":"r1::/wt/trello-abcd1234","repo":"app","displayName":"#1 x","path":"/wt/trello-abcd1234",
                 "agents":[{"paneKey":"t2:l2"}]}]}}"##,
        );
        assert_eq!(
            focus(&bin, "AbCd1234").await.unwrap_err(),
            "#1 x has no terminal open in Orca"
        );
        write(
            "terminals.json",
            r#"{"ok":true,"result":{"terminals":[
                {"handle":"term_shell","tabId":"t1","leafId":"l1"},
                {"handle":"term_agent","tabId":"t2","leafId":"l2"}]}}"#,
        );
        assert_eq!(
            focus(&bin, "AbCd1234").await.unwrap(),
            json!({"name": "#1 x", "repo": "app"})
        );
        let calls = logged(dir.path());
        let list = calls
            .iter()
            .find(|c| c[..2] == ["terminal", "list"])
            .unwrap();
        assert_eq!(flag(list, "--worktree"), "id:r1::/wt/trello-abcd1234");
        let switch = calls
            .iter()
            .find(|c| c[..2] == ["terminal", "switch"])
            .unwrap();
        assert_eq!(flag(switch, "--terminal"), "term_agent");
        assert_eq!(
            focus(&bin, "Ef56Gh78").await.unwrap_err(),
            "Ef56Gh78 is not in Orca"
        );
    }

    #[tokio::test]
    #[ignore = "starts processes, which macOS can freeze after the procs scan test launches a fresh binary: cargo test -- --ignored orca"]
    async fn send_refuses_unknown_agents_and_repos() {
        let dir = tempfile::tempdir().unwrap();
        let bin = fake_orca(dir.path());
        assert!(
            send(&bin, "r1", "bash", &[card("AbCd1234", "#1 x")], dir.path())
                .await
                .unwrap_err()
                .contains("unknown agent")
        );
        assert!(logged(dir.path()).is_empty(), "nothing ran");
        for repo in ["nope", "f1"] {
            assert!(
                send(
                    &bin,
                    repo,
                    "claude",
                    &[card("AbCd1234", "#1 x")],
                    dir.path()
                )
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
