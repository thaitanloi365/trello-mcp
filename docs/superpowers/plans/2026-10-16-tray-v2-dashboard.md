# Tray v2 and Trello MCP Dashboard Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A compact Swiss tray with folding projects, a theme switch and dashboard links, plus a `trello-mcp dashboard` web page: the tray's server table, Trello card search, @mentions waiting on you, and the cards in "Ready for dev" and "In progress" lists.

**Architecture:** `trello-mcp dashboard` is a new subcommand of this binary, built like gws-mcp's dashboard: axum on a current-thread runtime, one embedded `web/index.html`, a loopback listener on 8790, and a second loopback listener on 8791 for `tailscale serve`. That second one is open only to the owner's Tailscale login. Trello data comes from the same `mcp::Server` that `serve` uses: new `Server` methods in `src/mcp/feeds.rs`, not MCP tools. The dashboard finds and stops servers itself, with the tray's rules ported to Rust (`src/dashboard/servers.rs` pure, `src/dashboard/procs.rs` libproc). The Swift tray keeps its own scanner. It gets the new layout and starts and supervises the dashboard.

**Tech Stack:** Rust 1.88 (edition 2024), axum 0.8, tokio current-thread, libc (macOS libproc), wiremock for Trello tests. Swift + AppKit for the tray (no XCTest: `-D SELFTEST` preconditions). Plain HTML, CSS and JS for the page.

**Spec:** `docs/superpowers/specs/2026-10-16-tray-v2-dashboard-design.md`. Approved mockups: `.superpowers/brainstorm/42452-1792096558/content/tray-v2.html` and `.superpowers/brainstorm/42452-1792096558/content/dashboard.html`.

## Global Constraints

- `rust-version = "1.88"`, edition 2024. New dependencies: only `axum = { version = "0.8.9", default-features = false, features = ["http1", "tokio", "query"] }` and `libc = "0.2"`. tokio gains the `net` and `sync` features.
- Ports: the dashboard on `127.0.0.1:8790`; the tailnet listener on `127.0.0.1:8791`; `tailscale serve --https=8448`. Never point `tailscale serve` at 8790.
- No new MCP tools. `tests/fixtures/tools_list.json` must not change.
- Swiss tokens (claude-usage DESIGN.md):
  - light: paper `#fff`, ink `#111`, muted `#5c5c5c`, hair `#d4d4d4`, red `#da291c`, red-ink `#da291c`;
  - dark: paper `#0d0d0d`, ink `#f2f2f2`, muted `#a3a3a3`, hair `#333`, red-ink `#ff6a5c`;
  - Helvetica Neue, tabular numerals, square corners. Swiss red only for the action the page asks for (Confirm, Copy comment link) and alarms.
- Tray: panel 460 pt wide. Server rows 26 pt, group rows 28 pt, figures 17 pt. Columns: PID 40, Build (flexible), Up 52, Active 52, CPU 40, Mem 46, action 56, with 6 pt gaps. Groups start folded.
- Timing:
  - "Waiting on you" covers the last 30 days.
  - Trello results are cached for 60 s; errors are not cached.
  - The page polls servers every 2 s and Trello every 5 minutes, only while visible.
  - Quit sends SIGTERM, then SIGKILL after 5 s.
  - A crashed dashboard restarts after 1 s, doubling to 60 s, and back to 1 s after 5 minutes up.
- Errors from Trello never carry the request URL: the `trello::Client` already strips it, so always show `err.to_string()`.
- Rust test and lint evidence: run `rtk proxy cargo ...`, because the RTK hook hides cargo warnings.
- Until Task 4 wires the subcommand, the new Rust code is unused. Run `cargo test`, not `make lint`, in Tasks 1-3 (dead-code warnings are expected there).
- Commits: Conventional Commits, one short subject plus at most a few lines, with no AI attribution. Comments: one line, the why only, matching the surrounding style.

## Review Focus

The inputs and failure modes below are the ones most likely to bite a user that the spec's own tests don't cover. Each now has a test in the task named.

1. **Trello not configured, or a broken config file.** The dashboard must still start and serve the server table. The Trello sections show the config error. (Task 4: `servers_answer_without_trello_config`.)
2. **One board unreadable** (removed access: 401 or 404 on its snapshot). The other boards still list, and the list sections name the failed board. (Task 4: `trello_sections_are_cached_for_a_minute`.)
3. **Card names and mention text holding HTML or script.** These must render as text, and links must only ever point at `https://trello.com/`. (Task 5: `page_never_injects_html`, plus the `trello()` URL guard.)
4. **A stop request for a pid the last scan didn't list** (the test process itself, pid 1). It must get 404, and nothing is signalled. (Task 4: `stop_needs_our_page_and_a_listed_server`.)
5. **A mention longer than 280 characters, in emoji or Vietnamese.** It is cut on a character boundary with "…", with no panic. (Task 3: `long_mentions_are_cut_on_a_character`.)

---

### Task 1: Server rules and text in Rust

**Files:**
- Modify: `src/util.rs` (add `ago`, plus a test in its `mod tests`)
- Create: `src/dashboard/mod.rs` (only `mod servers;` for now)
- Create: `src/dashboard/servers.rs`
- Modify: `src/main.rs:1-6` (add `mod dashboard;`)

**Interfaces:**
- Produces:
  - `crate::util::ago(seconds: f64) -> String`
  - In `dashboard::servers`:
    - `ServerId { pid: i32, start: u64 }`
    - `Sample { id, ppid: i32, parent_started: Option<u64>, folder: Option<String>, path: String, cpu_ns: u64, mem: u64 }`
    - `Row { s: Sample, cpu: Option<f64>, last_active: Option<f64> }`
    - `ACTIVE_CPU_NS`, `MINUTE_SCAN_SECS`, `NO_SESSION`
    - `is_server(&[String]) -> bool`
    - `parse_proc_args(&[u8]) -> Option<Vec<String>>`
    - `baseline(Option<f64>, f64) -> Option<f64>`
    - `advance(&[Row], Vec<Sample>, Option<f64>, f64) -> Vec<Row>`
    - `view(&[Row], &HashSet<ServerId>, now: f64, cli: &str, home: &str) -> serde_json::Value`
  - Times are seconds since 1970 as `f64`.

- [ ] **Step 1: Write the failing tests**

Add to `mod tests` at the end of `src/util.rs`:

```rust
    #[test]
    fn ago_counts_minutes_hours_days() {
        assert_eq!(
            [ago(-5.0), ago(59.0), ago(12.0 * 60.0), ago(3.0 * 3600.0), ago(2.0 * 86400.0)],
            ["now", "now", "12m ago", "3h ago", "2d ago"]
        );
    }
```

Create `src/dashboard/mod.rs`:

```rust
//! `trello-mcp dashboard`: the tray's server table and Trello views as a local web page.

mod servers;
```

Add `mod dashboard;` to `src/main.rs` between `mod config;` and `mod mcp;`.

Create `src/dashboard/servers.rs`, with the tests only (the implementation comes in Step 3):

```rust
//! The tray's server rules and text (tray/Procs.swift) for the dashboard: which trello-mcp
//! processes are servers, CPU and activity across scans, grouping by session folder, and the
//! strings the page shows. Pure; procs.rs reads the processes.

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use serde_json::json;

    use super::*;

    const S0: u64 = 1_791_800_000_000_000; // a start time in µs
    const PS0: u64 = 1_791_700_000_000_000; // a session's start time

    fn args(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    fn srv(pid: i32, folder: Option<&str>, ppid: i32, parent: Option<u64>, mem: u64) -> Sample {
        Sample {
            id: ServerId { pid, start: S0 },
            ppid,
            parent_started: parent,
            folder: folder.map(str::to_string),
            path: "/x/trello-mcp".into(),
            cpu_ns: 0,
            mem,
        }
    }

    fn one(cpu_ns: u64) -> Sample {
        Sample { cpu_ns, ..srv(1, Some("/Users/me/work/app"), 900, Some(PS0), 10 << 20) }
    }

    #[test]
    fn which_processes_are_servers() {
        assert!(is_server(&args(&["/x/trello-mcp", "serve"])) && is_server(&args(&["/x/trello-mcp"])));
        assert!(is_server(&args(&["trello-mcp", "--config", "/a b/c.json", "serve"])));
        assert!(is_server(&args(&["trello-mcp", "serve", "--config", "/c.json"])));
        assert!(is_server(&args(&["trello-mcp", "--config=/c.json"])));
        for other in [
            &["trello-mcp", "config", "show"][..],
            &["trello-mcp", "version"],
            &["trello-mcp", "-v"],
            &["trello-mcp", "--config", "/c.json", "update"],
            &["trello-mcp", "dashboard", "--tailscale", "h:1=me"],
        ] {
            assert!(!is_server(&args(other)), "{other:?}");
        }
    }

    #[test]
    fn procargs_parse() {
        let mut buf = 4i32.to_ne_bytes().to_vec();
        buf.extend_from_slice(b"/My Apps/trello-mcp\0\0\0\0trello-mcp\0serve\0--config\0/a b/c.json\0HOME=/Users/me\0");
        assert_eq!(parse_proc_args(&buf), Some(args(&["trello-mcp", "serve", "--config", "/a b/c.json"])));
        assert_eq!(parse_proc_args(&[1, 0]), None, "shorter than argc");
        let mut few = 3i32.to_ne_bytes().to_vec();
        few.extend_from_slice(b"/x\0\0a\0");
        assert_eq!(parse_proc_args(&few), None, "fewer strings than argc");
        let mut cut = 1i32.to_ne_bytes().to_vec();
        cut.extend_from_slice(b"/x\0\0abc");
        assert_eq!(parse_proc_args(&cut), None, "cut before its NUL");
    }

    #[test]
    fn activity_and_cpu_across_scans() {
        let t0 = 1_000_000.0;
        let r1 = advance(&[], vec![one(0)], None, t0);
        assert_eq!((r1[0].cpu, r1[0].last_active), (None, None), "first scan");
        let r2 = advance(&r1, vec![one(1_999_999)], Some(t0), t0 + 2.0);
        assert!(r2[0].last_active.is_none() && (r2[0].cpu.unwrap() - 0.1).abs() < 1e-6, "under 2 ms is idle");
        let r3 = advance(&r2, vec![one(4_000_000)], Some(t0 + 2.0), t0 + 4.0);
        assert_eq!(r3[0].last_active, Some(t0 + 4.0), "2 ms or more is activity");
        let r4 = advance(&r3, vec![one(4_000_000)], None, t0 + 9000.0);
        assert_eq!((r4[0].cpu, r4[0].last_active), (None, Some(t0 + 4.0)), "no baseline: no CPU, activity kept");
        let r5 = advance(&r3, vec![one(4_000_000 + ACTIVE_CPU_NS)], None, t0 + 9000.0);
        assert_eq!(r5[0].last_active, None, "worked in a gap with no baseline: when is unknown");
        let mut reused = one(9_000_000);
        reused.id.start += 1;
        assert_eq!(advance(&r3, vec![reused], Some(t0 + 4.0), t0 + 6.0)[0].last_active, None, "a reused pid starts fresh");
        assert_eq!(baseline(Some(t0), t0 + 180.0), Some(t0));
        assert_eq!(baseline(Some(t0), t0 + 181.0), None, "older than three minute scans");
        assert_eq!(baseline(None, t0), None);
    }

    #[test]
    fn labels_and_text() {
        assert_eq!(build_label("/r/trello-mcp/.claude/worktrees/feat-x/bin/trello-mcp", "/c"), "feat-x");
        assert_eq!(
            [build_label("/Users/me/old/bin/trello-mcp", "/c"), build_label("trello-mcp", "/c"), build_label("/c", "/c")],
            ["old/bin", "trello-mcp", "installed"]
        );
        assert_eq!(build_label("/Users/me/old/bin/trello-mcp", ""), "old/bin", "no --cli never matches");
        let h = "/Users/me";
        assert_eq!(group_label(Some("/Users/me/Projects/eventcreate/eventcreate_app"), h), "eventcreate/eventcreate_app");
        assert_eq!([group_label(Some(h), h), group_label(Some("/"), h), group_label(Some("/tmp"), h)], ["~", "/", "tmp"]);
        assert_eq!([group_label(None, h), group_label(Some(""), h)], [NO_SESSION, NO_SESSION]);
        assert_eq!([mem_text(8_598_323), mem_text(49 << 20), mem_text(1229 << 20)], ["8.2 MB", "49 MB", "1.2 GB"]);
        assert_eq!(
            [cpu_text(None), cpu_text(Some(0.04)), cpu_text(Some(96.84)), cpu_text(Some(143.2))],
            ["—", "0.0%", "96.8%", "143%"]
        );
        assert_eq!(
            [countdown(45.0 * 60.0), countdown(5.0 * 3600.0 + 12.0 * 60.0), countdown(6.0 * 86400.0 + 21.0 * 3600.0)],
            ["45m", "5h 12m", "6d 21h"]
        );
        assert_eq!([active_text(None, 0.0), active_text(Some(0.0), 59.0), active_text(Some(0.0), 720.0)], ["—", "now", "12m ago"]);
    }

    #[test]
    fn groups_and_view() {
        let h = "/Users/me";
        let mix = advance(
            &[],
            vec![
                srv(5, Some("/a/zeta"), 900, Some(PS0), 10 << 20),
                srv(3, None, 1, None, 10 << 20),
                srv(7, None, 880, None, 10 << 20),
                srv(4, Some("/a/Alpha"), 901, Some(300), 10 << 20),
                srv(2, Some("/a/Alpha"), 902, Some(100), 10 << 20),
                srv(6, Some("/a/Alpha"), 902, Some(100), 3 << 20),
                srv(8, Some(h), 903, Some(50), 10 << 20),
            ],
            None,
            1_000_000.0,
        );
        let gs = groups(&mix, h);
        assert_eq!(gs.iter().map(|g| g.label.as_str()).collect::<Vec<_>>(), ["a/Alpha", "a/zeta", "~", NO_SESSION]);
        assert_eq!(gs[0].rows.iter().map(|r| r.s.id.pid).collect::<Vec<_>>(), [2, 6, 4], "oldest session first, then pid");
        assert_eq!(gs[3].rows.iter().map(|r| r.s.id.pid).collect::<Vec<_>>(), [3, 7]);
        let stopping: HashSet<ServerId> = [ServerId { pid: 6, start: S0 }].into();
        let v = view(&mix, &stopping, 1_000_000.0, "/x/trello-mcp", h);
        assert_eq!(v["figures"], json!({"servers": "7", "sessions": "4", "memory": "63 MB", "cpu": "—"}));
        let alpha = &v["groups"][0];
        assert_eq!(
            (&alpha["label"], &alpha["count"], &alpha["sessions"], &alpha["active"], &alpha["cpu"], &alpha["mem"]),
            (&json!("a/Alpha"), &json!(3), &json!(2), &json!("—"), &json!("—"), &json!("23 MB"))
        );
        let six = &alpha["rows"][1];
        assert_eq!(
            (&six["pid"], &six["start"], &six["build"], &six["mem"], &six["stopping"], &six["path"]),
            (&json!(6), &json!(S0), &json!("installed"), &json!("3.0 MB"), &json!(true), &json!("/x/trello-mcp"))
        );
        assert_eq!(v["groups"][3]["sessions"], 0);
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `rtk proxy cargo test --locked ago_counts dashboard::servers 2>&1 | tail -20`
Expected: compile errors such as "cannot find function `ago`" and "cannot find type `Sample`".

- [ ] **Step 3: Write the implementation**

Add to `src/util.rs`, after `path_unescape`:

```rust
/// "now" under a minute, then "12m ago", "3h ago", "2d ago".
pub fn ago(seconds: f64) -> String {
    let m = (seconds / 60.0).max(0.0) as i64;
    match m {
        0 => "now".into(),
        1..60 => format!("{m}m ago"),
        60..1440 => format!("{}h ago", m / 60),
        _ => format!("{}d ago", m / 1440),
    }
}
```

In `src/dashboard/servers.rs`, put this between the module doc comment and `#[cfg(test)]`:

```rust
use std::collections::{HashMap, HashSet};

use serde_json::{Value, json};

use crate::util::ago;

/// A process's identity: the start time (µs since 1970) tells a reused pid apart.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct ServerId {
    pub pid: i32,
    pub start: u64,
}

/// One server as a scan sees it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Sample {
    pub id: ServerId,
    pub ppid: i32,
    /// The session's start time (µs); None: no session (parent is launchd or gone).
    pub parent_started: Option<u64>,
    /// The session's working folder.
    pub folder: Option<String>,
    pub path: String,
    pub cpu_ns: u64,
    /// Physical footprint in bytes: Activity Monitor's Memory.
    pub mem: u64,
}

/// What the dashboard knows about one server across scans.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Row {
    pub s: Sample,
    /// Percent of one core since the previous scan; None on the first.
    pub cpu: Option<f64>,
    /// The last scan (s since 1970) that saw it work; None: not seen since the dashboard started.
    pub last_active: Option<f64>,
}

/// CPU time a server must burn between two scans to count as working: one tool call costs a few
/// ms; an idle server adds well under 1 ms per 2 s.
pub(crate) const ACTIVE_CPU_NS: u64 = 2_000_000;
pub(crate) const MINUTE_SCAN_SECS: f64 = 60.0;
pub(crate) const NO_SESSION: &str = "No session";

/// Arguments after the binary, with --config and its value taken out.
fn plain_args(argv: &[String]) -> Vec<&str> {
    let mut out = Vec::new();
    let mut it = argv.iter().skip(1);
    while let Some(a) = it.next() {
        if a == "--config" {
            it.next();
        } else if !a.starts_with("--config=") {
            out.push(a.as_str());
        }
    }
    out
}

/// trello-mcp serves when its first argument, --config aside, is `serve` or there is none.
pub(crate) fn is_server(argv: &[String]) -> bool {
    plain_args(argv).first().is_none_or(|a| *a == "serve")
}

/// argv from a KERN_PROCARGS2 buffer: argc (i32), the exec path, NUL padding, then argc NUL-ended strings.
pub(crate) fn parse_proc_args(b: &[u8]) -> Option<Vec<String>> {
    let argc = usize::try_from(i32::from_ne_bytes(b.get(..4)?.try_into().ok()?)).ok()?;
    let mut i = 4;
    while i < b.len() && b[i] != 0 {
        i += 1; // exec path
    }
    while i < b.len() && b[i] == 0 {
        i += 1; // padding
    }
    let mut out = Vec::with_capacity(argc);
    while out.len() < argc && i < b.len() {
        let end = i + b[i..].iter().position(|&c| c == 0)?; // cut off before its NUL: None
        out.push(String::from_utf8_lossy(&b[i..end]).into_owned());
        i = end + 1;
    }
    (out.len() == argc).then_some(out)
}

/// The previous scan as the baseline for this one; None when it is more than three minute scans
/// old (the Mac slept), so work done in that gap gets no made-up time.
pub(crate) fn baseline(prev: Option<f64>, at: f64) -> Option<f64> {
    prev.filter(|p| at - p <= 3.0 * MINUTE_SCAN_SECS)
}

/// Folds a scan into the rows: history carries over by ServerId, gone servers drop out. Without a
/// baseline there is no CPU figure; the last activity is kept unless the server worked in the gap.
pub(crate) fn advance(old: &[Row], scan: Vec<Sample>, prev_at: Option<f64>, at: f64) -> Vec<Row> {
    let by_id: HashMap<ServerId, &Row> = old.iter().map(|r| (r.s.id, r)).collect();
    scan.into_iter()
        .map(|s| {
            let Some(o) = by_id.get(&s.id) else {
                return Row { s, cpu: None, last_active: None };
            };
            match prev_at.filter(|p| at > *p) {
                None => {
                    let last_active = if s.cpu_ns < o.s.cpu_ns + ACTIVE_CPU_NS { o.last_active } else { None };
                    Row { s, cpu: None, last_active }
                }
                Some(p) => {
                    let d = s.cpu_ns.saturating_sub(o.s.cpu_ns);
                    let cpu = d as f64 / 1e9 / (at - p) * 100.0;
                    let last_active = if d >= ACTIVE_CPU_NS { Some(at) } else { o.last_active };
                    Row { s, cpu: Some(cpu), last_active }
                }
            }
        })
        .collect()
}

/// "now", "12m ago", …; "—" before any activity was seen.
pub(crate) fn active_text(last: Option<f64>, now: f64) -> String {
    last.map_or_else(|| "—".into(), |l| ago(now - l))
}

/// The Build cell: "installed" for the tray's CLI, the worktree's name under .claude/worktrees,
/// else the last two folders of the binary's path.
pub(crate) fn build_label(path: &str, cli: &str) -> String {
    if path == cli {
        return "installed".into();
    }
    if let Some((_, rest)) = path.split_once("/.claude/worktrees/") {
        return rest.split('/').next().unwrap_or_default().into();
    }
    let parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
    if parts.len() <= 1 {
        return path.into();
    }
    parts[parts.len().saturating_sub(3)..parts.len() - 1].join("/")
}

/// A group's label: the last two folders of its session's working folder, "~" for home.
pub(crate) fn group_label(folder: Option<&str>, home: &str) -> String {
    match folder {
        None | Some("") => NO_SESSION.into(),
        Some(f) if f == home => "~".into(),
        Some(f) => {
            let parts: Vec<&str> = f.split('/').filter(|p| !p.is_empty()).collect();
            if parts.is_empty() { f.into() } else { parts[parts.len().saturating_sub(2)..].join("/") }
        }
    }
}

/// "6d 21h", "5h 12m" or "12m", rounded down to the minute.
pub(crate) fn countdown(t: f64) -> String {
    let m = (t / 60.0).max(0.0) as i64;
    if m >= 1440 {
        format!("{}d {}h", m / 1440, m % 1440 / 60)
    } else if m >= 60 {
        format!("{}h {}m", m / 60, m % 60)
    } else {
        format!("{m}m")
    }
}

/// The Up cell: how long the owning session has run; "—" without one.
fn up_text(r: &Row, now: f64) -> String {
    r.s.parent_started.map_or_else(|| "—".into(), |p| countdown(now - p as f64 / 1e6))
}

/// "0.6%", "143%" from 100, "—" before a second sample.
pub(crate) fn cpu_text(p: Option<f64>) -> String {
    match p {
        None => "—".into(),
        Some(p) if p >= 100.0 => format!("{p:.0}%"),
        Some(p) => format!("{p:.1}%"),
    }
}

/// "8.2 MB" under 10 MB, "49 MB", "1.2 GB" from 1024 MB.
pub(crate) fn mem_text(b: u64) -> String {
    let mb = b as f64 / 1_048_576.0;
    if mb >= 1024.0 {
        format!("{:.1} GB", mb / 1024.0)
    } else if mb < 10.0 {
        format!("{mb:.1} MB")
    } else {
        format!("{mb:.0} MB")
    }
}

pub(crate) struct Group<'a> {
    pub label: String,
    pub rows: Vec<&'a Row>,
}

/// Groups by session folder, alphabetically with "No session" last; rows oldest session first.
pub(crate) fn groups<'a>(rows: &'a [Row], home: &str) -> Vec<Group<'a>> {
    let mut by: HashMap<String, Vec<&Row>> = HashMap::new();
    for r in rows {
        by.entry(group_label(r.s.folder.as_deref(), home)).or_default().push(r);
    }
    let mut out: Vec<Group> = by
        .into_iter()
        .map(|(label, mut rows)| {
            rows.sort_by_key(|r| (r.s.parent_started.unwrap_or(r.s.id.start), r.s.id.pid));
            Group { label, rows }
        })
        .collect();
    out.sort_by_key(|g| (g.label == NO_SESSION, g.label.to_lowercase()));
    out
}

/// Distinct sessions among the rows.
fn session_count(rows: &[&Row]) -> usize {
    rows.iter()
        .filter_map(|r| r.s.parent_started.map(|p| (r.s.ppid, p)))
        .collect::<HashSet<_>>()
        .len()
}

/// Total CPU of the rows; None while every row still waits for its second scan.
fn total_cpu(rows: &[&Row]) -> Option<f64> {
    rows.iter().filter_map(|r| r.cpu).reduce(|a, b| a + b)
}

fn total_mem(rows: &[&Row]) -> u64 {
    rows.iter().map(|r| r.s.mem).sum()
}

/// The /api/servers body: the figures, then the groups with their totals and rows, as display text.
pub(crate) fn view(rows: &[Row], stopping: &HashSet<ServerId>, now: f64, cli: &str, home: &str) -> Value {
    let all: Vec<&Row> = rows.iter().collect();
    let groups: Vec<Value> = groups(rows, home)
        .into_iter()
        .map(|g| {
            let latest = g.rows.iter().filter_map(|r| r.last_active).reduce(f64::max);
            let rows: Vec<Value> = g
                .rows
                .iter()
                .map(|r| {
                    json!({
                        "pid": r.s.id.pid, "start": r.s.id.start, "build": build_label(&r.s.path, cli),
                        "up": up_text(r, now), "active": active_text(r.last_active, now),
                        "cpu": cpu_text(r.cpu), "mem": mem_text(r.s.mem), "path": r.s.path,
                        "stopping": stopping.contains(&r.s.id),
                    })
                })
                .collect();
            json!({
                "label": g.label, "count": g.rows.len(), "sessions": session_count(&g.rows),
                "active": active_text(latest, now), "cpu": cpu_text(total_cpu(&g.rows)),
                "mem": mem_text(total_mem(&g.rows)), "rows": rows,
            })
        })
        .collect();
    json!({
        "figures": {
            "servers": rows.len().to_string(), "sessions": session_count(&all).to_string(),
            "memory": mem_text(total_mem(&all)), "cpu": cpu_text(total_cpu(&all)),
        },
        "groups": groups,
    })
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `rtk proxy cargo test --locked ago_counts dashboard::servers 2>&1 | tail -15`
Expected: `test result: ok. 6 passed` (dead-code warnings are expected until Task 4).

- [ ] **Step 5: Commit**

```bash
git add src/util.rs src/main.rs src/dashboard/mod.rs src/dashboard/servers.rs
git commit -m "feat(dashboard): port the tray's server rules and text to Rust"
```

---

### Task 2: Find, measure and stop servers with libproc (macOS)

**Files:**
- Modify: `Cargo.toml` (`libc = "0.2"` in `[dependencies]`)
- Modify: `src/dashboard/mod.rs` (add `#[cfg(target_os = "macos")] mod procs;`)
- Create: `src/dashboard/procs.rs`

**Interfaces:**
- Consumes: `servers::{Sample, ServerId, is_server, parse_proc_args}` from Task 1.
- Produces:
  - `procs::SERVER_NAME: &str = "trello-mcp"`
  - `procs::Scanner` (`Default`, `fn scan(&mut self) -> Vec<Sample>`)
  - `procs::terminate(ServerId, name: &str) -> Result<bool, String>`: `Ok(true)` once SIGTERM is sent, `Ok(false)` when it is gone or is no longer the same process, `Err(text)` when the kill fails.
  - `procs::kill_if_alive(ServerId, name: &str)`
  - `procs::STOP_GRACE: Duration` (5 s)

- [ ] **Step 1: Write the failing tests**

Add `libc = "0.2"` to `[dependencies]` in `Cargo.toml`, in alphabetical order, after `dirs`. Add `#[cfg(target_os = "macos")]\nmod procs;` to `src/dashboard/mod.rs` above `mod servers;`.

Create `src/dashboard/procs.rs`, with the tests only:

```rust
//! Finding, measuring and stopping the running trello-mcp servers with libproc, as the tray's
//! Procs.swift does; the rules they are read by live in servers.rs.

#[cfg(test)]
mod tests {
    use std::{
        os::unix::process::ExitStatusExt,
        path::Path,
        process::{Child, Command, Stdio},
        thread::sleep,
        time::Duration,
    };

    use super::*;

    const FAKE: &str = "dashboard::procs::tests::fake_server_sleeps";

    /// The fake server: this test binary, copied as trello-mcp, runs only this test, which sleeps.
    /// Copies of system binaries (arm64e) are killed on launch, so the tray's selftest does the same.
    #[test]
    #[ignore = "helper: the fake server the scan tests start"]
    fn fake_server_sleeps() {
        if std::env::var_os("TRELLO_MCP_FAKE_SERVER").is_some() {
            sleep(Duration::from_secs(30));
        }
    }

    /// This test binary copied into `dir` as trello-mcp, started with `first` as its first argument.
    fn fake(dir: &Path, first: &str) -> Child {
        let exe = dir.join(SERVER_NAME);
        if !exe.exists() {
            std::fs::copy(std::env::current_exe().unwrap(), &exe).unwrap();
        }
        Command::new(&exe)
            .args([first, "--exact", FAKE, "--ignored"])
            .env("TRELLO_MCP_FAKE_SERVER", "1")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap()
    }

    fn wait_for(mut done: impl FnMut() -> bool) {
        for _ in 0..200 {
            if done() {
                return;
            }
            sleep(Duration::from_millis(10));
        }
    }

    fn cpu_now() -> u64 {
        let mut t = libc::timespec { tv_sec: 0, tv_nsec: 0 };
        unsafe { libc::clock_gettime(libc::CLOCK_PROCESS_CPUTIME_ID, &mut t) };
        t.tv_sec as u64 * 1_000_000_000 + t.tv_nsec as u64
    }

    #[test]
    fn own_process() {
        let me = std::process::id() as i32;
        let (ppid, start) = bsd_info(me).expect("own bsd info");
        assert!(ppid == unsafe { libc::getppid() } && start > 0);
        assert_eq!(proc_name(me).as_deref().map(|n| n.starts_with("trello_mcp")), Some(true), "{:?}", proc_name(me));
        assert_eq!(args(me), Some(std::env::args().collect::<Vec<_>>()));
        assert!(user_pids().contains(&me));
        assert_eq!(folder(me).map(std::path::PathBuf::from), std::env::current_dir().ok());
        assert!(usage(me).expect("own usage").1 > 1 << 20);
        assert!(usage(-1).is_none() && bsd_info(-1).is_none() && proc_name(-1).is_none());
    }

    #[test]
    fn cpu_time_is_in_nanoseconds() {
        // Mach ticks, not ns, on Apple silicon: 0.3 s of CPU must read as about 0.3 s.
        let me = std::process::id() as i32;
        let u0 = usage(me).unwrap().0;
        let c0 = cpu_now();
        let mut n = 0u64;
        while cpu_now() - c0 < 300_000_000 {
            n = std::hint::black_box(n.wrapping_add(1));
        }
        let burnt = (usage(me).unwrap().0 - u0) as f64 / 1e9;
        assert!((0.15..0.9).contains(&burnt), "{burnt} s of CPU for 0.3 s busy");
    }

    #[test]
    fn scan_finds_servers_not_dashboards_and_stop_checks_identity() {
        let tmp = tempfile::Builder::new().prefix("procs test ").tempdir().unwrap();
        let mut server = fake(tmp.path(), "serve");
        let mut dash = fake(tmp.path(), "dashboard");
        let (sp, dp) = (server.id() as i32, dash.id() as i32);
        let mut sc = Scanner::default();
        let mut found = None;
        wait_for(|| {
            found = sc.scan().into_iter().find(|s| s.id.pid == sp);
            found.is_some()
        });
        let found = found.expect("the fake server is found");
        let dir_name = tmp.path().file_name().unwrap().to_string_lossy().into_owned();
        assert!(found.path.ends_with(&format!("{dir_name}/trello-mcp")), "binary path with a space: {found:?}");
        assert_eq!(found.ppid, std::process::id() as i32, "its session is this test process");
        assert_eq!(found.parent_started, bsd_info(std::process::id() as i32).map(|(_, s)| s));
        assert_eq!(found.folder.as_deref().map(Path::new), std::env::current_dir().ok().as_deref());
        assert!(found.mem > 0);
        wait_for(|| args(dp).is_some_and(|a| a.get(1).map(String::as_str) == Some("dashboard")));
        assert!(sc.scan().iter().all(|s| s.id.pid != dp), "a dashboard is not a server");

        // A reused pid (another start time) or another name is never signalled.
        assert_eq!(terminate(ServerId { pid: sp, start: found.id.start + 1 }, SERVER_NAME), Ok(false));
        assert_eq!(terminate(found.id, "sleep"), Ok(false));
        assert!(server.try_wait().unwrap().is_none());
        assert_eq!(terminate(found.id, SERVER_NAME), Ok(true));
        assert_eq!(server.wait().unwrap().signal(), Some(libc::SIGTERM));
        assert!(sc.scan().iter().all(|s| s.id != found.id), "a stopped server leaves the scan");
        dash.kill().unwrap();
        dash.wait().unwrap();
    }

    #[test]
    fn sigkill_after_the_grace_period() {
        let mut stubborn = Command::new("/bin/sh").args(["-c", "trap '' TERM; exec /bin/sleep 30"]).spawn().unwrap();
        let pid = stubborn.id() as i32;
        wait_for(|| proc_name(pid).as_deref() == Some("sleep"));
        let id = ServerId { pid, start: bsd_info(pid).unwrap().1 };
        assert_eq!(terminate(id, "sleep"), Ok(true));
        sleep(Duration::from_millis(300));
        assert!(stubborn.try_wait().unwrap().is_none(), "it ignores SIGTERM");
        kill_if_alive(id, "sleep");
        assert_eq!(stubborn.wait().unwrap().signal(), Some(libc::SIGKILL));
    }

    #[test]
    fn stop_failure_text() {
        assert_eq!(stop_failure(4648, libc::EPERM).as_deref(), Some("Could not stop PID 4648: Operation not permitted"));
        assert_eq!(stop_failure(4648, libc::ESRCH), None);
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `rtk proxy cargo test --locked dashboard::procs 2>&1 | tail -15`
Expected: compile errors such as "cannot find function `bsd_info`".

- [ ] **Step 3: Write the implementation**

In `src/dashboard/procs.rs`, put this between the module doc comment and `#[cfg(test)]`:

```rust
use std::{
    collections::{HashMap, HashSet},
    ffi::{CStr, c_int},
    mem::{MaybeUninit, size_of},
    sync::OnceLock,
    time::Duration,
};

use super::servers::{Sample, ServerId, is_server, parse_proc_args};

/// The server binary's process name; the tray is trello-mcp-tray, so it never matches.
pub(crate) const SERVER_NAME: &str = "trello-mcp";
/// Between SIGTERM and SIGKILL.
pub(crate) const STOP_GRACE: Duration = Duration::from_secs(5);
/// libproc.h; the libc crate does not export it.
const PROC_UID_ONLY: u32 = 4;

/// CPU times come in Mach ticks, not ns, on Apple silicon.
fn timebase() -> (u64, u64) {
    static T: OnceLock<(u64, u64)> = OnceLock::new();
    *T.get_or_init(|| {
        let mut t = libc::mach_timebase_info { numer: 0, denom: 0 };
        #[allow(deprecated)]
        let rc = unsafe { libc::mach_timebase_info(&mut t) };
        if rc == 0 && t.denom != 0 { (u64::from(t.numer), u64::from(t.denom)) } else { (1, 1) }
    })
}

/// The size of the argument area, for KERN_PROCARGS2.
fn arg_max() -> usize {
    static N: OnceLock<usize> = OnceLock::new();
    *N.get_or_init(|| {
        let (mut n, mut size): (c_int, usize) = (0, size_of::<c_int>());
        let mut mib = [libc::CTL_KERN, libc::KERN_ARGMAX];
        let rc = unsafe { libc::sysctl(mib.as_mut_ptr(), 2, (&raw mut n).cast(), &mut size, std::ptr::null_mut(), 0) };
        if rc == 0 && n > 0 { n as usize } else { 1 << 20 }
    })
}

/// This user's pids: the only processes the dashboard may stop.
fn user_pids() -> Vec<i32> {
    let uid = unsafe { libc::getuid() };
    let bytes = unsafe { libc::proc_listpids(PROC_UID_ONLY, uid, std::ptr::null_mut(), 0) };
    if bytes <= 0 {
        return Vec::new();
    }
    let mut pids = vec![0i32; bytes as usize / size_of::<i32>() + 64]; // room for processes started since
    let got = unsafe {
        libc::proc_listpids(PROC_UID_ONLY, uid, pids.as_mut_ptr().cast(), (pids.len() * size_of::<i32>()) as c_int)
    };
    pids.truncate(got.max(0) as usize / size_of::<i32>());
    pids.retain(|p| *p > 0);
    pids
}

fn proc_name(pid: i32) -> Option<String> {
    let mut b = [0u8; 64]; // proc_name fills up to 2 * MAXCOMLEN
    let n = unsafe { libc::proc_name(pid, b.as_mut_ptr().cast(), b.len() as u32) };
    if n <= 0 {
        return None;
    }
    CStr::from_bytes_until_nul(&b).ok().map(|c| c.to_string_lossy().into_owned())
}

/// Parent pid and start time (µs); None once the process is gone.
fn bsd_info(pid: i32) -> Option<(i32, u64)> {
    let mut i = MaybeUninit::<libc::proc_bsdinfo>::zeroed();
    let size = size_of::<libc::proc_bsdinfo>() as c_int;
    if unsafe { libc::proc_pidinfo(pid, libc::PROC_PIDTBSDINFO, 0, i.as_mut_ptr().cast(), size) } != size {
        return None;
    }
    let i = unsafe { i.assume_init() };
    Some((i.pbi_ppid as i32, i.pbi_start_tvsec * 1_000_000 + i.pbi_start_tvusec))
}

/// The working folder.
fn folder(pid: i32) -> Option<String> {
    let mut v = MaybeUninit::<libc::proc_vnodepathinfo>::zeroed();
    let size = size_of::<libc::proc_vnodepathinfo>() as c_int;
    if unsafe { libc::proc_pidinfo(pid, libc::PROC_PIDVNODEPATHINFO, 0, v.as_mut_ptr().cast(), size) } != size {
        return None;
    }
    let v = unsafe { v.assume_init() };
    // vip_path is MAXPATHLEN (1024) bytes, typed as [[c_char; 32]; 32] by the libc crate.
    let bytes: &[u8] = unsafe { std::slice::from_raw_parts(v.pvi_cdir.vip_path.as_ptr().cast(), 1024) };
    CStr::from_bytes_until_nul(bytes).ok().map(|c| c.to_string_lossy().into_owned())
}

/// The binary.
fn path(pid: i32) -> Option<String> {
    let mut b = vec![0u8; 4096]; // PROC_PIDPATHINFO_MAXSIZE
    let n = unsafe { libc::proc_pidpath(pid, b.as_mut_ptr().cast(), b.len() as u32) };
    (n > 0).then(|| String::from_utf8_lossy(&b[..n as usize]).into_owned())
}

fn args(pid: i32) -> Option<Vec<String>> {
    let mut size = arg_max();
    let mut b = vec![0u8; size];
    let mut mib = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid];
    let rc = unsafe { libc::sysctl(mib.as_mut_ptr(), 3, b.as_mut_ptr().cast(), &mut size, std::ptr::null_mut(), 0) };
    if rc != 0 {
        return None;
    }
    parse_proc_args(&b[..size])
}

/// CPU time in ns and memory footprint in bytes, as Activity Monitor counts them; None once it is gone.
fn usage(pid: i32) -> Option<(u64, u64)> {
    let mut ri = MaybeUninit::<libc::rusage_info_v2>::zeroed();
    if unsafe { libc::proc_pid_rusage(pid, libc::RUSAGE_INFO_V2, ri.as_mut_ptr().cast()) } != 0 {
        return None;
    }
    let ri = unsafe { ri.assume_init() };
    let (numer, denom) = timebase();
    Some(((ri.ri_user_time + ri.ri_system_time) * numer / denom, ri.ri_phys_footprint))
}

/// What never changes for a server: read once per (pid, start time).
#[derive(Clone)]
struct Fixed {
    path: String,
    ppid: i32,
    parent_started: Option<u64>,
    folder: Option<String>,
}

/// Finds and measures the running servers; a repeat scan reads only names, parents and usage.
#[derive(Default)]
pub(crate) struct Scanner {
    fixed: HashMap<ServerId, Fixed>,
    others: HashSet<ServerId>, // trello-mcp processes that are not servers
}

impl Scanner {
    pub(crate) fn scan(&mut self) -> Vec<Sample> {
        let (mut out, mut next_fixed, mut next_others) = (Vec::new(), HashMap::new(), HashSet::new());
        for pid in user_pids() {
            if proc_name(pid).as_deref() != Some(SERVER_NAME) {
                continue;
            }
            // A failing call means the process just exited: skip it.
            let Some((ppid, start)) = bsd_info(pid) else { continue };
            let id = ServerId { pid, start };
            if self.others.contains(&id) {
                next_others.insert(id);
                continue;
            }
            let f = match self.fixed.get(&id) {
                Some(f) => f.clone(),
                None => {
                    let Some(argv) = args(pid) else { continue };
                    if !is_server(&argv) {
                        next_others.insert(id);
                        continue;
                    }
                    // The session is the parent; launchd (1) or a parent that is gone means none.
                    let session = if ppid > 1 { bsd_info(ppid) } else { None };
                    Fixed {
                        path: path(pid).or_else(|| argv.first().cloned()).unwrap_or_else(|| SERVER_NAME.into()),
                        ppid,
                        parent_started: session.map(|(_, s)| s),
                        folder: session.and_then(|_| folder(ppid)),
                    }
                }
            };
            let Some((cpu_ns, mem)) = usage(pid) else { continue };
            // A server whose parent exited is adopted by launchd: it has no session from then on.
            let session = ppid == f.ppid;
            out.push(Sample {
                id,
                ppid,
                parent_started: if session { f.parent_started } else { None },
                folder: if session { f.folder.clone() } else { None },
                path: f.path.clone(),
                cpu_ns,
                mem,
            });
            next_fixed.insert(id, f);
        }
        self.fixed = next_fixed;
        self.others = next_others;
        out
    }
}

/// The process still runs under this pid and start time, with this name.
fn alive(id: ServerId, name: &str) -> bool {
    proc_name(id.pid).as_deref() == Some(name) && bsd_info(id.pid).map(|(_, s)| s) == Some(id.start)
}

/// Why kill failed; None when the process was already gone.
fn stop_failure(pid: i32, err: i32) -> Option<String> {
    let text = unsafe { CStr::from_ptr(libc::strerror(err)) }.to_string_lossy();
    (err != libc::ESRCH).then(|| format!("Could not stop PID {pid}: {text}"))
}

/// SIGTERM if `id` still runs as `name`. Ok(false): gone, or its pid now belongs to another
/// process, so nothing was sent. The caller follows up with `kill_if_alive` after STOP_GRACE.
pub(crate) fn terminate(id: ServerId, name: &str) -> Result<bool, String> {
    if !alive(id, name) {
        return Ok(false);
    }
    if unsafe { libc::kill(id.pid, libc::SIGTERM) } != 0 {
        let err = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
        return stop_failure(id.pid, err).map_or(Ok(false), Err);
    }
    Ok(true)
}

/// SIGKILL if `id` still runs as `name`.
pub(crate) fn kill_if_alive(id: ServerId, name: &str) {
    if alive(id, name) {
        unsafe { libc::kill(id.pid, libc::SIGKILL) };
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `rtk proxy cargo test --locked dashboard::procs 2>&1 | tail -15`
Expected: `test result: ok. 5 passed; 0 failed; 1 ignored`. If `own_process` fails on the name, print `proc_name(me)`: the test binary's name is `trello_mcp-<hash>`, cut to 16 characters by the kernel.

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml Cargo.lock src/dashboard/mod.rs src/dashboard/procs.rs
git commit -m "feat(dashboard): find, measure and stop servers with libproc"
```

---

### Task 3: Trello feeds: lists, mentions waiting on you, search

**Files:**
- Create: `src/mcp/feeds.rs`
- Modify: `src/mcp/mod.rs:8-15` (add `pub(crate) mod feeds;`)

**Interfaces:**
- Consumes:
  - `Server::list_boards("")`, which returns `[{id, name, workspace?}]` for the open boards in the allowed workspaces;
  - `self.client.call(Method, path, params, None)`;
  - `nested_string`, `object_slice`, `escape`, `msg`, `Error` from `src/mcp/mod.rs`;
  - `crate::util::ago`.
- Produces, in `mcp::feeds`:
  - `ListKind { ReadyForDev, InProgress }` and `list_kind(&str) -> Option<ListKind>`
  - `unix_seconds(&str) -> Option<i64>` and `iso_date(i64) -> String`
  - `card_view(card, board, now) -> Value`, returning `{id, number, name, url, board, list, members: [initials], activity}`
  - `named_lists(boards: &[Value], ListKind, now) -> Vec<Value>`, returning `[{board, url, cards}]`
  - `waiting_rows(notes, mine, boards, now) -> Vec<Value>`: each row is a card view plus `{mention, by, age, commentUrl, earlier}`
  - `classify(&str) -> Result<Query, Error>`
  - `Server::board_snapshot(&self, board_id) -> Result<Value, Error>`
  - `Server::waiting(&self, boards: &[Value], now) -> Result<Vec<Value>, Error>`
  - `Server::search_cards(&self, q, boards: &[Value], now) -> Result<Vec<Value>, Error>`
  - `#[cfg(test)] pub(crate) fn tests::board() -> Value`: the board fixture that Task 4 reuses.
  - `now` is seconds since 1970 as `i64`.

- [ ] **Step 1: Write the failing tests**

Add `pub(crate) mod feeds;` to `src/mcp/mod.rs`, after `mod extras;`. Create `src/mcp/feeds.rs`, with the tests only:

```rust
//! Trello data for the dashboard: cards in "Ready for dev" and "In progress" lists, the @mentions
//! you have not answered, and card search. Server methods, not MCP tools: the tool list stays lean.

#[cfg(test)]
pub(crate) mod tests {
    use serde_json::{Value, json};

    use super::*;
    use crate::mcp::test_support::{calls, json_response, not_found, query, test_server};

    pub(crate) const NOW_ISO: &str = "2026-10-16T12:00:00.000Z";

    fn now() -> i64 {
        unix_seconds(NOW_ISO).unwrap()
    }

    fn at(seconds_ago: i64) -> String {
        iso_date(now() - seconds_ago)
    }

    fn card(id: &str, number: i64, name: &str, link: &str, list: &str, pos: f64) -> Value {
        json!({
            "id": id, "idShort": number, "name": name, "shortUrl": format!("https://trello.com/c/{link}"),
            "shortLink": link, "idList": list, "idBoard": "b1", "idMembers": ["m1"], "pos": pos,
            "dateLastActivity": at(2 * 3600),
        })
    }

    /// One board as `board_snapshot` returns it.
    pub(crate) fn board() -> Value {
        json!({
            "id": "b1", "name": "Client Portal", "url": "https://trello.com/b/B1/client-portal",
            "lists": [{"id": "l1", "name": "In Progress"}, {"id": "l2", "name": "Ready for Dev 🚀"}, {"id": "l3", "name": "Done"}],
            "members": [{"id": "m1", "initials": "MT", "fullName": "Mai Tran"}],
            "cards": [
                card("c1", 348, "Block physical address", "aaaa1111", "l1", 1.0),
                card("c2", 61, "Migrate cron jobs", "bbbb2222", "l2", 2.0),
                card("c3", 7, "Rate limit quotes", "cccc3333", "l2", 1.0),
                card("c4", 9, "Shipped thing", "dddd4444", "l3", 1.0),
            ]
        })
    }

    fn mention(id: &str, card: &str, seconds_ago: i64, text: &str) -> Value {
        json!({
            "id": format!("n-{id}"), "type": "mentionedOnCard", "date": at(seconds_ago), "idAction": id,
            "data": {"text": text, "card": {"id": card}, "board": {"id": "b1"}},
            "memberCreator": {"fullName": "Mai Tran"},
        })
    }

    fn my_comment(card: &str, seconds_ago: i64) -> Value {
        json!({"date": at(seconds_ago), "data": {"card": {"id": card}}})
    }

    #[test]
    fn list_names_match_loosely() {
        for (name, kind) in [
            ("Ready for dev", Some(ListKind::ReadyForDev)),
            ("Ready for Development", Some(ListKind::ReadyForDev)),
            ("READY FOR DEV 🚀", Some(ListKind::ReadyForDev)),
            ("In progress", Some(ListKind::InProgress)),
            ("In-Progress", Some(ListKind::InProgress)),
            ("In progress (BE)", Some(ListKind::InProgress)),
            ("Ready", None),
            ("Not in progress", None),
            ("Progress review", None),
            ("", None),
        ] {
            assert_eq!(list_kind(name), kind, "{name}");
        }
    }

    #[test]
    fn trello_dates() {
        assert_eq!(unix_seconds("1970-01-01T00:00:00.000Z"), Some(0));
        assert_eq!(unix_seconds("2000-03-01T00:00:00Z"), Some(951_868_800));
        assert_eq!(iso_date(951_868_800), "2000-03-01T00:00:00.000Z");
        let t = unix_seconds("2026-10-16T03:12:45.123Z").unwrap();
        assert_eq!(iso_date(t), "2026-10-16T03:12:45.000Z");
        assert_eq!(iso_date(0), "1970-01-01T00:00:00.000Z");
        assert_eq!(unix_seconds("nope"), None);
        assert_eq!(unix_seconds(""), None);
    }

    #[test]
    fn named_lists_keep_list_and_card_order() {
        let boards = [board()];
        let ready = named_lists(&boards, ListKind::ReadyForDev, now());
        assert_eq!(ready.len(), 1);
        assert_eq!((&ready[0]["board"], &ready[0]["url"]), (&json!("Client Portal"), &json!("https://trello.com/b/B1/client-portal")));
        assert_eq!(ready[0]["cards"].as_array().unwrap().iter().map(|c| c["number"].as_i64().unwrap()).collect::<Vec<_>>(), [7, 61]);
        assert_eq!(
            ready[0]["cards"][0],
            json!({"id": "c3", "number": 7, "name": "Rate limit quotes", "url": "https://trello.com/c/cccc3333",
                   "board": "Client Portal", "list": "Ready for Dev 🚀", "members": ["MT"], "activity": "2h ago"})
        );
        let progress = named_lists(&boards, ListKind::InProgress, now());
        assert_eq!(progress[0]["cards"][0]["number"], 348);
        let mut empty = board();
        empty["lists"] = json!([{"id": "l3", "name": "Done"}]);
        assert!(named_lists(&[empty], ListKind::InProgress, now()).is_empty(), "a board without such a list is left out");
    }

    #[test]
    fn waiting_keeps_unanswered_mentions_only() {
        let notes = json!([
            mention("a1", "c1", 2 * 3600, "@loithai can you confirm the API returns 422?"),
            mention("a2", "c2", 86_400, "@loithai is the retry policy final?"),
            mention("a3", "c2", 3 * 86_400, "@loithai ping"),
            mention("a4", "c3", 5 * 86_400, "@loithai answered already"),
            mention("a5", "c1", 40 * 86_400, "@loithai too old"),
            mention("a6", "gone", 3600, "@loithai on an archived card"),
            json!({"id": "n-x", "date": at(60), "data": {}}),
        ]);
        let mine = json!([my_comment("c3", 4 * 86_400), my_comment("c1", 3 * 3600)]);
        let rows = waiting_rows(&notes, &mine, &[board()], now());
        assert_eq!(rows.len(), 2, "{rows:#?}");
        assert_eq!(
            (&rows[0]["number"], &rows[0]["mention"], &rows[0]["by"], &rows[0]["age"], &rows[0]["earlier"]),
            (&json!(348), &json!("@loithai can you confirm the API returns 422?"), &json!("Mai Tran"), &json!("2h ago"), &json!(0))
        );
        assert_eq!(rows[0]["commentUrl"], "https://trello.com/c/aaaa1111#comment-a1");
        assert_eq!(rows[0]["list"], "In Progress");
        assert_eq!((&rows[1]["number"], &rows[1]["earlier"], &rows[1]["age"]), (&json!(61), &json!(1), &json!("1d ago")));
        assert_eq!(rows[1]["commentUrl"], "https://trello.com/c/bbbb2222#comment-a2", "the latest mention");
    }

    #[test]
    fn long_mentions_are_cut_on_a_character() {
        let text = "é😀 Xin chào ".repeat(40);
        let rows = waiting_rows(&json!([mention("a1", "c1", 60, &text)]), &json!([]), &[board()], now());
        let cut = rows[0]["mention"].as_str().unwrap();
        assert_eq!(cut.chars().count(), 280);
        assert!(cut.ends_with('…') && text.starts_with(cut.trim_end_matches('…')));
        let mut no_action = mention("", "c1", 60, "hi");
        no_action.as_object_mut().unwrap().remove("idAction");
        let rows = waiting_rows(&json!([no_action]), &json!([]), &[board()], now());
        assert_eq!(rows[0]["commentUrl"], "https://trello.com/c/aaaa1111", "no action id: the card's link");
    }

    #[test]
    fn queries_are_classified() {
        assert!(matches!(classify("  ").unwrap(), Query::Empty));
        assert!(matches!(classify("#348").unwrap(), Query::Number(348)));
        assert!(matches!(classify("348").unwrap(), Query::Number(348)));
        assert!(matches!(classify("https://trello.com/c/kfp0Te5b/348-block-physical").unwrap(), Query::Link(l) if l == "kfp0Te5b"));
        assert!(matches!(classify("kfp0Te5b").unwrap(), Query::Link(l) if l == "kfp0Te5b"));
        assert!(matches!(classify("invoices").unwrap(), Query::Text(t) if t == "invoices"), "8 letters, no digit: words");
        assert!(matches!(classify("invoice pdf").unwrap(), Query::Text(t) if t == "invoice pdf"));
        assert!(classify(&"a".repeat(200)).is_ok() && classify(&"a".repeat(201)).is_err());
    }

    #[tokio::test]
    async fn snapshot_and_waiting_ask_trello_for_the_right_things() {
        let h = test_server(&[], |req| match req.url.path() {
            "/boards/b1" => json_response(board()),
            "/members/me/notifications" => json_response(json!([mention("a1", "c1", 60, "@me hi")])),
            "/members/me/actions" => json_response(json!([])),
            _ => not_found(),
        })
        .await;
        let boards = vec![h.server.board_snapshot("b1").await.unwrap()];
        let rows = h.server.waiting(&boards, now()).await.unwrap();
        assert_eq!(rows.len(), 1);
        let reqs = h.api.received_requests().await.unwrap();
        let q = |path: &str| query(reqs.iter().find(|r| r.url.path() == path).unwrap());
        let b = q("/boards/b1");
        assert_eq!((b["lists"].as_str(), b["cards"].as_str(), b["members"].as_str()), ("open", "open", "all"));
        assert!(b["card_fields"].contains("shortLink") && b["card_fields"].contains("idShort") && b["card_fields"].contains("pos"));
        assert_eq!(q("/members/me/notifications")["filter"], "mentionedOnCard");
        let a = q("/members/me/actions");
        assert_eq!((a["filter"].as_str(), a["since"].as_str()), ("commentCard", iso_date(now() - 30 * 86_400).as_str()));
    }

    #[tokio::test]
    async fn search_uses_the_snapshot_for_numbers_and_links() {
        let h = test_server(&[], |req| match req.url.path() {
            "/search" => json_response(json!({"cards": [card("c1", 348, "Block physical address", "aaaa1111", "l1", 1.0)]})),
            _ => not_found(),
        })
        .await;
        let boards = [board()];
        let s = &h.server;
        assert_eq!(s.search_cards("#61", &boards, now()).await.unwrap()[0]["name"], "Migrate cron jobs");
        let url = "https://trello.com/c/cccc3333/7-rate-limit";
        assert_eq!(s.search_cards(url, &boards, now()).await.unwrap()[0]["number"], 7);
        assert!(s.search_cards("https://trello.com/c/zzzz9999", &boards, now()).await.unwrap().is_empty());
        assert!(s.search_cards("", &boards, now()).await.unwrap().is_empty());
        assert!(calls(&h.api).await.is_empty(), "numbers, links and empty queries never call Trello");
        let found = s.search_cards("address", &boards, now()).await.unwrap();
        assert_eq!((&found[0]["number"], &found[0]["list"]), (&json!(348), &json!("In Progress")));
        let reqs = h.api.received_requests().await.unwrap();
        let q = query(&reqs[0]);
        assert_eq!((q["query"].as_str(), q["modelTypes"].as_str(), q["idBoards"].as_str()), ("address", "cards", "b1"));
        // A bare 8-character short link that is no open card falls back to words.
        s.search_cards("abcd1234", &boards, now()).await.unwrap();
        assert_eq!(calls(&h.api).await.len(), 2);
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `rtk proxy cargo test --locked mcp::feeds 2>&1 | tail -15`
Expected: compile errors such as "cannot find type `ListKind`".

- [ ] **Step 3: Write the implementation**

In `src/mcp/feeds.rs`, put this between the module doc comment and `#[cfg(test)]`:

```rust
use std::collections::HashMap;

use reqwest::Method;
use serde_json::{Value, json};

use super::{Error, Server, escape, msg, nested_string, object_slice};
use crate::util::ago;

/// The card fields every view needs.
const CARD_FIELDS: &str = "name,idShort,shortUrl,shortLink,idList,idBoard,idMembers,dateLastActivity,pos";
pub(crate) const WAITING_DAYS: i64 = 30;
const MENTION_CHARS: usize = 280;
const QUERY_CHARS: usize = 200;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ListKind {
    ReadyForDev,
    InProgress,
}

/// "Ready for Development", "READY FOR DEV 🚀"; "In-Progress (BE)": case, spaces and marks aside.
pub(crate) fn list_kind(name: &str) -> Option<ListKind> {
    let key: String = name.chars().filter(char::is_ascii_alphanumeric).map(|c| c.to_ascii_lowercase()).collect();
    if key.starts_with("readyfordev") {
        Some(ListKind::ReadyForDev)
    } else if key.starts_with("inprogress") {
        Some(ListKind::InProgress)
    } else {
        None
    }
}

/// Seconds since 1970 of a Trello date ("2026-10-16T03:12:45.123Z"); None when it is not one.
pub(crate) fn unix_seconds(iso: &str) -> Option<i64> {
    let b = iso.as_bytes();
    if b.len() < 19 || b[4] != b'-' || b[7] != b'-' || b[10] != b'T' || b[13] != b':' || b[16] != b':' {
        return None;
    }
    let n = |from: usize, to: usize| iso.get(from..to)?.parse::<i64>().ok();
    let (y, m, d) = (n(0, 4)?, n(5, 7)?, n(8, 10)?);
    let (hh, mm, ss) = (n(11, 13)?, n(14, 16)?, n(17, 19)?);
    // Days from the civil date: Howard Hinnant's algorithm.
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some((era * 146_097 + doe - 719_468) * 86_400 + hh * 3600 + mm * 60 + ss)
}

/// The ISO date Trello takes for `since`.
pub(crate) fn iso_date(secs: i64) -> String {
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.000Z", rem / 3600, rem % 3600 / 60, rem % 60)
}

/// id → `field` of a board's lists or members.
fn by_id(board: &Value, key: &str, field: &str) -> HashMap<String, String> {
    object_slice(board.get(key).unwrap_or(&Value::Null))
        .iter()
        .map(|o| {
            let get = |k: &str| o.get(k).and_then(Value::as_str).unwrap_or_default().to_string();
            (get("id"), get(field))
        })
        .collect()
}

fn cards_of(board: &Value) -> impl Iterator<Item = &Value> {
    board.get("cards").and_then(Value::as_array).into_iter().flatten()
}

/// A card as the page shows it, with its list name and member initials from its board.
pub(crate) fn card_view(card: &Value, board: &Value, now: i64) -> Value {
    let lists = by_id(board, "lists", "name");
    let initials = by_id(board, "members", "initials");
    let members: Vec<String> = card
        .get("idMembers")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter_map(|id| initials.get(id).cloned())
        .collect();
    let activity = unix_seconds(&nested_string(card, &["dateLastActivity"]))
        .map_or_else(String::new, |t| ago((now - t) as f64));
    json!({
        "id": nested_string(card, &["id"]),
        "number": card.get("idShort").and_then(Value::as_i64),
        "name": nested_string(card, &["name"]),
        "url": nested_string(card, &["shortUrl"]),
        "board": nested_string(board, &["name"]),
        "list": lists.get(&nested_string(card, &["idList"])).cloned().unwrap_or_default(),
        "members": members,
        "activity": activity,
    })
}

/// The cards in every list of this kind, by board (by name); in list order, then card order.
pub(crate) fn named_lists(boards: &[Value], kind: ListKind, now: i64) -> Vec<Value> {
    let mut out: Vec<Value> = boards
        .iter()
        .filter_map(|board| {
            let lists: Vec<String> = object_slice(board.get("lists").unwrap_or(&Value::Null))
                .iter()
                .filter(|l| list_kind(l.get("name").and_then(Value::as_str).unwrap_or_default()) == Some(kind))
                .filter_map(|l| l.get("id").and_then(Value::as_str).map(str::to_string))
                .collect();
            let at = |c: &Value| lists.iter().position(|l| *l == nested_string(c, &["idList"]));
            let pos = |c: &Value| c.get("pos").and_then(Value::as_f64).unwrap_or_default();
            let mut cards: Vec<&Value> = cards_of(board).filter(|c| at(c).is_some()).collect();
            if cards.is_empty() {
                return None;
            }
            cards.sort_by(|a, b| at(a).cmp(&at(b)).then(pos(a).total_cmp(&pos(b))));
            Some(json!({
                "board": nested_string(board, &["name"]),
                "url": nested_string(board, &["url"]),
                "cards": cards.into_iter().map(|c| card_view(c, board, now)).collect::<Vec<_>>(),
            }))
        })
        .collect();
    out.sort_by_key(|b| nested_string(b, &["board"]).to_lowercase());
    out
}

/// The pure part of `waiting`: mentions of the last 30 days on open cards in `boards` with no
/// comment of yours on that card since; one row per card (its latest mention), newest first.
pub(crate) fn waiting_rows(notes: &Value, mine: &Value, boards: &[Value], now: i64) -> Vec<Value> {
    let since = now - WAITING_DAYS * 86_400;
    let mut my_last: HashMap<String, i64> = HashMap::new();
    for a in object_slice(mine) {
        let a = Value::Object(a);
        if let Some(t) = unix_seconds(&nested_string(&a, &["date"])) {
            let last = my_last.entry(nested_string(&a, &["data", "card", "id"])).or_insert(t);
            *last = (*last).max(t);
        }
    }
    let cards: HashMap<String, (&Value, &Value)> =
        boards.iter().flat_map(|b| cards_of(b).map(move |c| (nested_string(c, &["id"]), (c, b)))).collect();
    let mut by_card: HashMap<String, Vec<(i64, Value)>> = HashMap::new();
    for n in object_slice(notes) {
        let n = Value::Object(n);
        let card_id = nested_string(&n, &["data", "card", "id"]);
        let Some(t) = unix_seconds(&nested_string(&n, &["date"])) else { continue };
        // Archived or out-of-scope cards are not in `cards`.
        if t < since || !cards.contains_key(&card_id) || my_last.get(&card_id).is_some_and(|m| *m > t) {
            continue;
        }
        by_card.entry(card_id).or_default().push((t, n));
    }
    let mut rows: Vec<(i64, Value)> = by_card
        .into_iter()
        .map(|(card_id, mut mentions)| {
            mentions.sort_by_key(|(t, _)| -t);
            let (t, n) = &mentions[0];
            let (card, board) = cards[&card_id];
            let text = nested_string(n, &["data", "text"]);
            let mention = if text.chars().count() > MENTION_CHARS {
                text.chars().take(MENTION_CHARS - 1).chain(['…']).collect()
            } else {
                text
            };
            let action = nested_string(n, &["idAction"]);
            let url = nested_string(card, &["shortUrl"]);
            let comment_url = if action.is_empty() {
                url
            } else {
                format!("https://trello.com/c/{}#comment-{action}", nested_string(card, &["shortLink"]))
            };
            let mut row = card_view(card, board, now);
            if let Some(r) = row.as_object_mut() {
                r.insert("mention".into(), json!(mention));
                r.insert("by".into(), json!(nested_string(n, &["memberCreator", "fullName"])));
                r.insert("age".into(), json!(ago((now - t) as f64)));
                r.insert("commentUrl".into(), json!(comment_url));
                r.insert("earlier".into(), json!(mentions.len() - 1));
            }
            (*t, row)
        })
        .collect();
    rows.sort_by_key(|(t, _)| -t);
    rows.into_iter().map(|(_, r)| r).collect()
}

/// What a search box query asks for.
#[derive(Debug)]
pub(crate) enum Query {
    Empty,
    /// A card's short link, from its URL or typed bare.
    Link(String),
    /// A card number, on any board.
    Number(i64),
    Text(String),
}

pub(crate) fn classify(q: &str) -> Result<Query, Error> {
    let q = q.trim();
    if q.is_empty() {
        return Ok(Query::Empty);
    }
    if q.chars().count() > QUERY_CHARS {
        return Err(msg(format!("search is limited to {QUERY_CHARS} characters")));
    }
    if let Some((_, rest)) = q.split_once("trello.com/c/") {
        let link: String = rest.chars().take_while(char::is_ascii_alphanumeric).collect();
        if !link.is_empty() {
            return Ok(Query::Link(link));
        }
    }
    let digits = q.strip_prefix('#').unwrap_or(q);
    if !digits.is_empty() && digits.len() <= 7 && digits.bytes().all(|b| b.is_ascii_digit()) {
        return Ok(Query::Number(digits.parse().unwrap_or_default()));
    }
    // Short links are 8 random letters and digits; a plain 8-letter word stays a word.
    let b = q.as_bytes();
    if b.len() == 8 && b.iter().all(u8::is_ascii_alphanumeric) && b.iter().any(u8::is_ascii_digit) && b.iter().any(u8::is_ascii_alphabetic) {
        return Ok(Query::Link(q.to_string()));
    }
    Ok(Query::Text(q.to_string()))
}

/// Cards in the snapshot that `keep` picks, as views.
fn find(boards: &[Value], now: i64, keep: impl Fn(&Value) -> bool) -> Vec<Value> {
    boards.iter().flat_map(|b| cards_of(b).filter(|c| keep(c)).map(move |c| card_view(c, b, now))).collect()
}

impl Server {
    /// One board as the dashboard reads it: lists, open cards and members, in one call.
    pub(crate) async fn board_snapshot(&self, board_id: &str) -> Result<Value, Error> {
        Ok(self
            .client
            .call(
                Method::GET,
                &format!("/boards/{}", escape(board_id)),
                json!({
                    "fields": "name,url",
                    "lists": "open", "list_fields": "name",
                    "cards": "open", "card_fields": CARD_FIELDS,
                    "members": "all", "member_fields": "initials,fullName",
                }),
                None,
            )
            .await?)
    }

    /// The @mentions waiting on you, on the open cards of `boards` (their snapshots).
    pub(crate) async fn waiting(&self, boards: &[Value], now: i64) -> Result<Vec<Value>, Error> {
        let notes = self
            .client
            .call(
                Method::GET,
                "/members/me/notifications",
                json!({"filter": "mentionedOnCard", "limit": 200, "memberCreator": true, "memberCreator_fields": "fullName"}),
                None,
            )
            .await?;
        let mine = self
            .client
            .call(
                Method::GET,
                "/members/me/actions",
                json!({"filter": "commentCard", "since": iso_date(now - WAITING_DAYS * 86_400), "limit": 1000, "fields": "date,data"}),
                None,
            )
            .await?;
        Ok(waiting_rows(&notes, &mine, boards, now))
    }

    /// Card numbers and links resolve from the snapshot; words go to Trello's search on the same boards.
    pub(crate) async fn search_cards(&self, q: &str, boards: &[Value], now: i64) -> Result<Vec<Value>, Error> {
        let text = match classify(q)? {
            Query::Empty => return Ok(Vec::new()),
            Query::Number(n) => return Ok(find(boards, now, |c| c.get("idShort").and_then(Value::as_i64) == Some(n))),
            Query::Link(link) => {
                let hits = find(boards, now, |c| nested_string(c, &["shortLink"]) == link);
                if !hits.is_empty() || q.contains("trello.com/c/") {
                    return Ok(hits);
                }
                link
            }
            Query::Text(text) => text,
        };
        let ids: Vec<String> = boards.iter().map(|b| nested_string(b, &["id"])).collect();
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let found = self
            .client
            .call(
                Method::GET,
                "/search",
                json!({
                    "query": text, "modelTypes": "cards", "idBoards": ids.join(","),
                    "cards_limit": 20, "partial": true, "card_fields": CARD_FIELDS,
                }),
                None,
            )
            .await?;
        let by_board: HashMap<String, &Value> = boards.iter().map(|b| (nested_string(b, &["id"]), b)).collect();
        Ok(object_slice(found.get("cards").unwrap_or(&Value::Null))
            .into_iter()
            .filter_map(|c| {
                let c = Value::Object(c);
                by_board.get(&nested_string(&c, &["idBoard"])).map(|b| card_view(&c, b, now))
            })
            .collect())
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `rtk proxy cargo test --locked mcp::feeds 2>&1 | tail -15`
Expected: `test result: ok. 9 passed`. If `query()` values are `&String` and don't compare, add `.as_str()` as the assertions show. `query` returns `HashMap<String, String>`.

- [ ] **Step 5: Commit**

```bash
git add src/mcp/mod.rs src/mcp/feeds.rs
git commit -m "feat(dashboard): Trello lists, unanswered mentions and card search"
```

---

### Task 4: The dashboard server and the `dashboard` subcommand

**Files:**
- Modify: `Cargo.toml` (axum; tokio `net` and `sync`)
- Modify: `src/dashboard/mod.rs` (the whole server)
- Create: `web/index.html` (a one-line page; Task 5 replaces it)
- Modify: `src/main.rs` (the `Dashboard` command, `run_dashboard`, and a parse test in `mod tests`)

**Interfaces:**
- Consumes: Tasks 1-3, `mcp::Server::list_boards`, `crate::shutdown_signal()` (private in `src/main.rs`, which is visible to child modules).
- Produces:
  - `dashboard::ADDR` (`"127.0.0.1:8790"`) and `dashboard::TAILNET_ADDR` (`"127.0.0.1:8791"`)
  - `dashboard::run(config_path, addr, tailnet_addr, tailscale) -> anyhow::Result<()>`
  - `dashboard::PAGE`
  - HTTP routes: `GET /`, `GET /api/servers`, `POST /api/servers/{pid}/{start}/stop`, `GET /api/trello[?fresh=1]`, `GET /api/search?q=`.

- [ ] **Step 1: Write the failing tests**

In `Cargo.toml`:
- add `axum = { version = "0.8.9", default-features = false, features = ["http1", "tokio", "query"] }` at the top of `[dependencies]`, after `anyhow`;
- change the tokio line to `tokio = { version = "1", features = ["rt", "macros", "io-std", "net", "signal", "sync", "time"] }`.

Create `web/index.html`:

```html
<!doctype html><meta charset="utf-8"><title>Trello MCP</title><p>Trello MCP dashboard</p>
```

Replace `src/dashboard/mod.rs` with the module header and the tests only (the server comes in Step 3):

```rust
//! `trello-mcp dashboard`: the tray's server table plus Trello search, the @mentions waiting on
//! you, and the "Ready for dev" and "In progress" lists, as a local web page. Two loopback
//! listeners, as in gws-mcp: 127.0.0.1:8790 for this Mac, and with --tailscale a second one that
//! `tailscale serve` points at, open only to the owner's Tailscale login.

#[cfg(target_os = "macos")]
mod procs;
mod servers;

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};

    use serde_json::json;

    use super::*;
    use crate::mcp::test_support::{calls, json_response, not_found, test_server};

    /// A dashboard on 127.0.0.1:<random port>, and its tailnet listener on another.
    async fn start(trello: Result<mcp::Server, String>, tailscale: &str) -> (String, String) {
        let local = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = local.local_addr().unwrap().port();
        let dash = Dash::new(trello, "/x/trello-mcp".into(), port, tailscale);
        let ts = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let ts_base = format!("127.0.0.1:{}", ts.local_addr().unwrap().port());
        let (a, b) = (router(dash.clone(), false), router(dash, true));
        tokio::spawn(async move { axum::serve(local, a).await.unwrap() });
        tokio::spawn(async move { axum::serve(ts, b).await.unwrap() });
        (format!("127.0.0.1:{port}"), ts_base)
    }

    /// (status, raw response) of one HTTP/1.1 request sent as written, so the test picks the Host.
    async fn call(to: &str, method: &str, path: &str, headers: &[(&str, &str)]) -> (u16, String) {
        let mut req = format!("{method} {path} HTTP/1.1\r\nConnection: close\r\nContent-Length: 0\r\n");
        for (k, v) in headers {
            req.push_str(&format!("{k}: {v}\r\n"));
        }
        req.push_str("\r\n");
        let to = to.to_string();
        // A blocking socket on the blocking pool: the server runs on this test's runtime.
        tokio::task::spawn_blocking(move || {
            let mut s = std::net::TcpStream::connect(to).unwrap();
            s.write_all(req.as_bytes()).unwrap();
            let mut out = String::new();
            s.read_to_string(&mut out).unwrap();
            (out[9..12].parse().unwrap(), out)
        })
        .await
        .unwrap()
    }

    fn body(raw: &str) -> &str {
        raw.split_once("\r\n\r\n").map_or("", |(_, b)| b)
    }

    async fn get_json(to: &str, path: &str) -> Value {
        let (_, raw) = call(to, "GET", path, &[("Host", to)]).await;
        serde_json::from_str(body(&raw)).unwrap_or_else(|e| panic!("{e}: {raw}"))
    }

    #[test]
    fn flags_are_checked() {
        assert_eq!(check_addr("127.0.0.1:8790").unwrap(), 8790);
        assert_eq!(check_addr("localhost:1").unwrap(), 1);
        assert!(check_addr("0.0.0.0:8790").is_err() && check_addr("127.0.0.1:0").is_err() && check_addr("8790").is_err());
        assert!(check_tailscale("").is_ok() && check_tailscale("h.ts.net:8448=me@x.io").is_ok());
        assert!(check_tailscale("h.ts.net:8448").is_err() && check_tailscale("=me").is_err() && check_tailscale("h:1=").is_err());
    }

    #[tokio::test]
    async fn local_listener_takes_loopback_hosts_only() {
        let (us, _) = start(Err("no config".into()), "").await;
        let (status, page) = call(&us, "GET", "/", &[("Host", &us)]).await;
        let low = page.to_lowercase();
        assert_eq!(status, 200);
        assert!(low.contains("x-frame-options: deny") && low.contains("frame-ancestors 'none'") && low.contains("cache-control: no-store"), "{page}");
        assert!(page.contains("Trello MCP"));
        assert_eq!(call(&us, "GET", "/", &[("Host", &us.replace("127.0.0.1", "localhost"))]).await.0, 200);
        assert_eq!(call(&us, "GET", "/", &[("Host", "evil.example:80")]).await.0, 403);
        assert_eq!(call(&us, "GET", "/", &[("Host", &us), ("X-Forwarded-For", "100.64.0.1")]).await.0, 403, "never behind a proxy");
        assert_eq!(call(&us, "GET", "/nope", &[("Host", &us)]).await.0, 404);
    }

    #[tokio::test]
    async fn tailnet_listener_takes_the_owner_only() {
        let (us, ts) = start(Err("no config".into()), "mac.example.ts.net:8448=me@example.com").await;
        let host = ("Host", "mac.example.ts.net:8448");
        let owner = ("Tailscale-User-Login", "me@example.com");
        assert_eq!(call(&ts, "GET", "/", &[host, owner]).await.0, 200);
        assert_eq!(call(&ts, "GET", "/", &[host, ("Tailscale-User-Login", "other@example.com")]).await.0, 403);
        assert_eq!(call(&ts, "GET", "/", &[host]).await.0, 403);
        assert_eq!(call(&ts, "GET", "/", &[("Host", &us), owner]).await.0, 403);
        assert_eq!(call(&us, "GET", "/", &[host, owner]).await.0, 403, "the local listener takes no tailnet Host");
    }

    #[tokio::test]
    async fn stop_needs_our_page_and_a_listed_server() {
        let (us, _) = start(Err("no config".into()), "").await;
        let origin = format!("http://{us}");
        let path = format!("/api/servers/{}/1/stop", std::process::id());
        let mine = [("Host", us.as_str()), ("X-Trello-MCP", "1"), ("Origin", origin.as_str())];
        assert_eq!(call(&us, "POST", &path, &mine[..1]).await.0, 403, "no X-Trello-MCP header");
        assert_eq!(call(&us, "POST", &path, &[mine[0], mine[1], ("Origin", "https://evil.example")]).await.0, 403);
        assert_eq!(call(&us, "POST", &path, &mine).await.0, 404, "not a listed server: nothing is signalled");
        assert_eq!(call(&us, "POST", "/api/servers/1/1/stop", &mine).await.0, 404);
    }

    #[tokio::test]
    async fn servers_answer_without_trello_config() {
        let (us, _) = start(Err("read config: api_key is required".into()), "").await;
        let v = get_json(&us, "/api/servers").await;
        if cfg!(target_os = "macos") {
            assert!(v["figures"]["servers"].is_string() && v["groups"].is_array(), "{v}");
        } else {
            assert_eq!(v, json!({"unsupported": true}));
        }
        assert_eq!(get_json(&us, "/api/trello").await, json!({"error": "read config: api_key is required"}));
        let long = format!("/api/search?q={}", "a".repeat(201));
        assert_eq!(call(&us, "GET", &long, &[("Host", &us)]).await.0, 400);
        assert_eq!(get_json(&us, "/api/search?q=x").await, json!({"error": "read config: api_key is required"}));
    }

    #[tokio::test]
    async fn trello_sections_are_cached_for_a_minute() {
        let h = test_server(&[], |req| match req.url.path() {
            "/members/me/boards" => json_response(json!([
                {"id": "b1", "name": "Client Portal", "idOrganization": "w1"},
                {"id": "b2", "name": "Gone Board", "idOrganization": "w1"}
            ])),
            "/boards/b1" => json_response(feeds::tests::board()),
            "/members/me/notifications" | "/members/me/actions" => json_response(json!([])),
            _ => not_found(),
        })
        .await;
        let (us, _) = start(Ok(h.server.clone()), "").await;
        let v = get_json(&us, "/api/trello").await;
        assert_eq!(v["boards"], 1, "{v}");
        assert!(v["updatedAt"].as_i64().unwrap() > 1_700_000_000);
        assert_eq!(v["progress"]["boards"][0]["cards"][0]["number"], 348);
        assert_eq!(v["ready"]["boards"][0]["cards"].as_array().unwrap().len(), 2);
        assert!(v["ready"]["error"].as_str().unwrap().contains("Gone Board"), "a failed board is named: {v}");
        assert_eq!(v["waiting"], json!({"rows": []}));
        let n = calls(&h.api).await.len();
        get_json(&us, "/api/trello").await;
        assert_eq!(calls(&h.api).await.len(), n, "a second load within a minute comes from the cache");
        get_json(&us, "/api/trello?fresh=1").await;
        assert!(calls(&h.api).await.len() > n, "Refresh refetches");
        let found = get_json(&us, "/api/search?q=%23348").await;
        assert_eq!(found["cards"][0]["name"], "Block physical address");
    }

    #[tokio::test]
    async fn a_second_dashboard_on_the_same_port_fails() {
        let held = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = held.local_addr().unwrap().to_string();
        let err = run("", &addr, TAILNET_ADDR, "").await.unwrap_err();
        assert!(format!("{err:#}").contains("already running"), "{err:#}");
    }
}
```

In `src/main.rs`'s `mod tests`, add:

```rust
    #[test]
    fn dashboard_defaults_to_loopback_ports() {
        let cli = Cli::try_parse_from(["trello-mcp", "dashboard"]).unwrap();
        assert!(matches!(
            cli.command,
            Some(Command::Dashboard { ref addr, ref tailnet_addr, ref tailscale })
                if addr == "127.0.0.1:8790" && tailnet_addr == "127.0.0.1:8791" && tailscale.is_empty()
        ));
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `rtk proxy cargo test --locked dashboard 2>&1 | tail -15`
Expected: compile errors such as "cannot find function `start`… `Dash`" and "no variant named `Dashboard`".

- [ ] **Step 3: Write the implementation**

In `src/dashboard/mod.rs`, after `mod servers;` and before `#[cfg(test)]`, add:

```rust
use std::{
    collections::{HashMap, HashSet},
    future::IntoFuture,
    net::Ipv4Addr,
    sync::{Arc, Mutex},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, anyhow, bail};
use axum::{
    Router,
    extract::{Path, Query, Request, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde_json::{Value, json};

use crate::{
    config,
    mcp::{
        self,
        feeds::{self, ListKind},
    },
    trello,
};

pub(crate) const ADDR: &str = "127.0.0.1:8790";
pub(crate) const TAILNET_ADDR: &str = "127.0.0.1:8791";
pub(crate) const PAGE: &[u8] = include_bytes!("../../web/index.html");
const TRELLO_TTL: Duration = Duration::from_secs(60);

/// What the scans know; one scan at a time, under the lock.
#[derive(Default)]
struct Servers {
    rows: Vec<servers::Row>,
    scanned_at: Option<f64>,
    stopping: HashSet<servers::ServerId>,
    #[cfg(target_os = "macos")]
    scanner: procs::Scanner,
}

struct TrelloView {
    at: Instant,
    boards: Arc<Vec<Value>>,
    body: Value,
}

pub(crate) struct Dash {
    hosts: HashSet<String>,
    origins: HashSet<String>,
    ts_host: String,
    ts_login: String,
    trello: Result<mcp::Server, String>,
    cli: String,
    home: String,
    servers: Mutex<Servers>,
    trello_view: tokio::sync::Mutex<Option<TrelloView>>,
}

fn now() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64())
}

impl Dash {
    /// `cli` is this binary: its servers show as "installed". `tailscale` is "" or host:port=login.
    pub(crate) fn new(trello: Result<mcp::Server, String>, cli: String, port: u16, tailscale: &str) -> Arc<Self> {
        let hosts: HashSet<String> = [format!("127.0.0.1:{port}"), format!("localhost:{port}")].into();
        let mut origins: HashSet<String> = hosts.iter().map(|h| format!("http://{h}")).collect();
        let (ts_host, ts_login) = tailscale.split_once('=').unwrap_or((tailscale, ""));
        if !ts_host.is_empty() && !ts_login.is_empty() {
            origins.insert(format!("https://{ts_host}"));
        }
        Arc::new(Self {
            hosts,
            origins,
            ts_host: ts_host.into(),
            ts_login: ts_login.into(),
            trello,
            cli,
            home: dirs::home_dir().map(|h| h.to_string_lossy().into_owned()).unwrap_or_default(),
            servers: Mutex::new(Servers::default()),
            trello_view: tokio::sync::Mutex::new(None),
        })
    }

    /// Scans unless the last scan is under `min_gap` s old. Blocking: call it on the blocking pool.
    fn scan(&self, min_gap: f64) {
        let mut s = self.servers.lock().unwrap();
        let at = now();
        if s.scanned_at.is_some_and(|t| at - t < min_gap) {
            return;
        }
        #[cfg(target_os = "macos")]
        {
            let found = s.scanner.scan();
            let live: HashSet<servers::ServerId> = found.iter().map(|f| f.id).collect();
            s.stopping.retain(|id| live.contains(id)); // a stopped server that is gone is done stopping
            let prev = servers::baseline(s.scanned_at, at);
            s.rows = servers::advance(&s.rows, found, prev, at);
        }
        s.scanned_at = Some(at);
    }

    /// Lets only our page act: another site cannot send the custom header without a CORS
    /// preflight we never answer, and a present Origin must be ours.
    fn is_ours(&self, h: &HeaderMap) -> bool {
        let origin = hdr(h, header::ORIGIN);
        hdr(h, "x-trello-mcp") == "1" && (origin.is_empty() || self.origins.contains(&origin))
    }

    /// The boards in scope (snapshots) and the page's Trello sections, from a 60 s cache.
    async fn trello_view(&self, fresh: bool) -> (Arc<Vec<Value>>, Value) {
        let mut cache = self.trello_view.lock().await;
        if let Some(c) = cache.as_ref().filter(|c| !fresh && c.at.elapsed() < TRELLO_TTL) {
            return (c.boards.clone(), c.body.clone());
        }
        let (boards, body) = match &self.trello {
            Ok(server) => load_trello(server).await,
            Err(e) => (Vec::new(), json!({"error": e})),
        };
        let boards = Arc::new(boards);
        // An error is not cached: the next load tries again.
        *cache = (body.get("error").is_none()).then(|| TrelloView { at: Instant::now(), boards: boards.clone(), body: body.clone() });
        (boards, body)
    }
}

/// Every board's snapshot, then the sections; a board that fails is named, the rest still show.
async fn load_trello(server: &mcp::Server) -> (Vec<Value>, Value) {
    let now = now() as i64;
    let scope = match server.list_boards("").await {
        Ok(v) => mcp::object_slice(&v),
        Err(e) => return (Vec::new(), json!({"error": e.to_string()})),
    };
    let (mut boards, mut failed) = (Vec::new(), Vec::new());
    for b in &scope {
        let id = b.get("id").and_then(Value::as_str).unwrap_or_default();
        match server.board_snapshot(id).await {
            Ok(v) => boards.push(v),
            Err(e) => failed.push(format!("{}: {e}", b.get("name").and_then(Value::as_str).unwrap_or(id))),
        }
    }
    let lists_error = (!failed.is_empty()).then(|| format!("Could not read {} board(s): {}", failed.len(), failed.join("; ")));
    let section = |kind| {
        let mut v = json!({"boards": feeds::named_lists(&boards, kind, now)});
        if let Some(e) = &lists_error {
            v["error"] = json!(e);
        }
        v
    };
    let (ready, progress) = (section(ListKind::ReadyForDev), section(ListKind::InProgress));
    let waiting = match server.waiting(&boards, now).await {
        Ok(rows) => json!({"rows": rows}),
        Err(e) => json!({"rows": [], "error": e.to_string()}),
    };
    let body = json!({"boards": boards.len(), "updatedAt": now, "waiting": waiting, "ready": ready, "progress": progress});
    (boards, body)
}

/// Serves the dashboard on `addr` until SIGTERM or Ctrl-C. With `tailscale` set, a second loopback
/// listener on `tailnet_addr` serves the tailnet: `tailscale serve` points there.
pub(crate) async fn run(config_path: &str, addr: &str, tailnet_addr: &str, tailscale: &str) -> Result<()> {
    let port = check_addr(addr)?;
    check_tailscale(tailscale)?;
    // The bound port is the single-instance lock.
    let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, port))
        .await
        .with_context(|| format!("listen {addr}: is the dashboard already running?"))?;
    let tailnet = if tailscale.is_empty() {
        None
    } else {
        let p = check_addr(tailnet_addr)?;
        Some(
            tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, p))
                .await
                .with_context(|| format!("listen {tailnet_addr} (--tailnet-addr)"))?,
        )
    };
    // A bad config still serves the server table; the Trello sections show why.
    let trello = config::Manager::new(config_path)
        .map(|m| mcp::Server::new(Arc::new(trello::Client::new(Arc::new(m)))))
        .map_err(|e| e.to_string());
    let cli = std::env::current_exe()
        .and_then(|p| p.canonicalize())
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default();
    let dash = Dash::new(trello, cli, port, tailscale);
    // A scan a minute, so ACTIVE knows when each server last worked.
    let d = dash.clone();
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(60));
        loop {
            tick.tick().await;
            let d = d.clone();
            let _ = tokio::task::spawn_blocking(move || d.scan(0.0)).await;
        }
    });
    eprintln!("trello-mcp dashboard: http://{addr}");
    let local = axum::serve(listener, router(dash.clone(), false)).with_graceful_shutdown(crate::shutdown_signal()).into_future();
    match tailnet {
        None => local.await?,
        Some(l) => {
            eprintln!("trello-mcp dashboard (tailnet): http://{tailnet_addr}");
            let ts = axum::serve(l, router(dash, true)).into_future();
            tokio::select! {
                r = local => r?,
                r = ts => r?,
            }
        }
    }
    Ok(())
}

/// The port of `127.0.0.1:<port>` or `localhost:<port>`: the dashboard answers on loopback only.
fn check_addr(addr: &str) -> Result<u16> {
    addr.rsplit_once(':')
        .filter(|(host, _)| *host == "127.0.0.1" || *host == "localhost")
        .and_then(|(_, port)| port.parse::<u16>().ok())
        .filter(|p| *p > 0)
        .ok_or_else(|| anyhow!("--addr must be 127.0.0.1:<port> or localhost:<port>, got {addr:?}"))
}

/// "" (off) or host:port=login.
fn check_tailscale(ts: &str) -> Result<()> {
    match ts.split_once('=') {
        _ if ts.is_empty() => Ok(()),
        Some((host, login)) if !host.is_empty() && !login.is_empty() => Ok(()),
        _ => bail!("--tailscale must be host:port=login, got {ts:?}"),
    }
}

/// The local listener (`tailnet` false) or the one `tailscale serve` points at (true).
pub(crate) fn router(dash: Arc<Dash>, tailnet: bool) -> Router {
    Router::new()
        .route("/", get(|| async { ([(header::CONTENT_TYPE, "text/html; charset=utf-8")], PAGE) }))
        .route("/api/servers", get(servers_now))
        .route("/api/servers/{pid}/{start}/stop", post(stop_server))
        .route("/api/trello", get(trello_now))
        .route("/api/search", get(search))
        .fallback(|| async { http_error(StatusCode::NOT_FOUND, "404 page not found") })
        .layer(middleware::from_fn_with_state((dash.clone(), tailnet), guard))
        .with_state(dash)
}

/// Every route, never framed or cached. Trust follows the listener, not the Host a client sends:
/// the local one takes loopback Hosts and no proxy; the tailnet one takes the Tailscale Host and
/// the owner's login (`tailscale serve` replaces any Tailscale-User-Login a client sends).
async fn guard(State((dash, tailnet)): State<(Arc<Dash>, bool)>, req: Request, next: Next) -> Response {
    let h = req.headers();
    let host = hdr(h, header::HOST);
    let allowed = if tailnet {
        !dash.ts_login.is_empty() && host == dash.ts_host && hdr(h, "tailscale-user-login") == dash.ts_login
    } else {
        dash.hosts.contains(&host) && !h.keys().any(|k| k.as_str().starts_with("x-forwarded-"))
    };
    if !allowed {
        return http_error(StatusCode::FORBIDDEN, "forbidden");
    }
    let mut resp = next.run(req).await;
    let h = resp.headers_mut();
    h.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    h.insert(header::CONTENT_SECURITY_POLICY, HeaderValue::from_static("frame-ancestors 'none'"));
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    resp
}

async fn servers_now(State(dash): State<Arc<Dash>>) -> Response {
    if !cfg!(target_os = "macos") {
        return json_ok(&json!({"unsupported": true}));
    }
    let d = dash.clone();
    let _ = tokio::task::spawn_blocking(move || d.scan(1.0)).await;
    let s = dash.servers.lock().unwrap();
    json_ok(&servers::view(&s.rows, &s.stopping, now(), &dash.cli, &dash.home))
}

/// Quit's Confirm: SIGTERM now, SIGKILL after 5 s; only for a server the last scan listed.
async fn stop_server(State(dash): State<Arc<Dash>>, h: HeaderMap, Path((pid, start)): Path<(i32, u64)>) -> Response {
    if !dash.is_ours(&h) {
        return http_error(StatusCode::FORBIDDEN, "forbidden");
    }
    let id = servers::ServerId { pid, start };
    if !dash.servers.lock().unwrap().rows.iter().any(|r| r.s.id == id) {
        return http_error(StatusCode::NOT_FOUND, "no such server");
    }
    #[cfg(target_os = "macos")]
    {
        match procs::terminate(id, procs::SERVER_NAME) {
            Err(e) => return http_error(StatusCode::FORBIDDEN, &e),
            Ok(false) => return http_error(StatusCode::NOT_FOUND, "no such server"),
            Ok(true) => {
                eprintln!("trello-mcp dashboard: stopping PID {pid}");
                dash.servers.lock().unwrap().stopping.insert(id);
                tokio::spawn(async move {
                    tokio::time::sleep(procs::STOP_GRACE).await;
                    procs::kill_if_alive(id, procs::SERVER_NAME);
                });
            }
        }
    }
    StatusCode::ACCEPTED.into_response()
}

async fn trello_now(State(dash): State<Arc<Dash>>, Query(q): Query<HashMap<String, String>>) -> Response {
    json_ok(&dash.trello_view(q.contains_key("fresh")).await.1)
}

async fn search(State(dash): State<Arc<Dash>>, Query(q): Query<HashMap<String, String>>) -> Response {
    let q = q.get("q").map_or("", String::as_str);
    if let Err(e) = feeds::classify(q) {
        return http_error(StatusCode::BAD_REQUEST, &e.to_string());
    }
    let server = match &dash.trello {
        Ok(server) => server,
        Err(e) => return json_ok(&json!({"error": e})),
    };
    let (boards, _) = dash.trello_view(false).await;
    match server.search_cards(q, &boards, now() as i64).await {
        Ok(cards) => json_ok(&json!({"cards": cards})),
        Err(e) => json_ok(&json!({"error": e.to_string()})),
    }
}

fn json_ok(v: &Value) -> Response {
    ([(header::CONTENT_TYPE, "application/json")], v.to_string()).into_response()
}

fn http_error(status: StatusCode, msg: &str) -> Response {
    (
        status,
        [(header::CONTENT_TYPE, "text/plain; charset=utf-8"), (header::X_CONTENT_TYPE_OPTIONS, "nosniff")],
        format!("{msg}\n"),
    )
        .into_response()
}

fn hdr(h: &HeaderMap, name: impl header::AsHeaderName) -> String {
    h.get(name).map(|v| String::from_utf8_lossy(v.as_bytes()).into_owned()).unwrap_or_default()
}
```

In `src/main.rs`, add to `enum Command`, after `Serve`:

```rust
    /// Serve the dashboard: servers, Trello search, mentions waiting on you, list summaries
    Dashboard {
        /// local address (loopback only)
        #[arg(long, default_value = dashboard::ADDR)]
        addr: String,
        /// loopback address `tailscale serve` points at (with --tailscale)
        #[arg(long, default_value = dashboard::TAILNET_ADDR)]
        tailnet_addr: String,
        /// this Mac's tailnet host:port=owner login; only that login gets in
        #[arg(long, default_value = "")]
        tailscale: String,
    },
```

Add to the `match cli.command` (after the `Serve` arm):

```rust
        Some(Command::Dashboard { addr, tailnet_addr, tailscale }) => {
            run_dashboard(&config_path, &addr, &tailnet_addr, &tailscale)
        }
```

Add after `fn serve`:

```rust
fn run_dashboard(config_path: &str, addr: &str, tailnet_addr: &str, tailscale: &str) -> anyhow::Result<()> {
    // One owner and a few requests a minute: a current-thread runtime, as `serve` uses.
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(dashboard::run(config_path, addr, tailnet_addr, tailscale))
}
```

- [ ] **Step 4: Run the tests and lint**

Run: `rtk proxy cargo test --locked 2>&1 | tail -15`
Expected: every test passes, the earlier tasks' tests included.

Run: `rtk proxy make lint 2>&1 | tail -30`
Expected: no warnings. Fix any clippy findings in the new code, and run `cargo fmt` if formatting differs.

- [ ] **Step 5: Smoke-run it**

```bash
cargo build --locked && (target/debug/trello-mcp dashboard --addr 127.0.0.1:18790 & echo $! > /tmp/dash.pid; sleep 1)
curl -s -H 'Host: 127.0.0.1:18790' http://127.0.0.1:18790/api/servers | head -c 400; echo
curl -s -H 'Host: 127.0.0.1:18790' http://127.0.0.1:18790/api/trello | head -c 400; echo
kill "$(cat /tmp/dash.pid)"
```

Expected:
- `/api/servers` returns JSON whose figures count this Mac's real trello-mcp servers.
- `/api/trello` returns `boards` and `waiting` from the real account, or a clear `error`.

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml Cargo.lock src/main.rs src/dashboard/mod.rs web/index.html
git commit -m "feat(dashboard): trello-mcp dashboard with owner-only tailnet listener"
```

---

### Task 5: The dashboard page

**Files:**
- Modify: `web/index.html` (the whole page)
- Modify: `src/dashboard/mod.rs` (one test in `mod tests`)

**Interfaces:**
- Consumes: the JSON shapes from Task 4:
  - `/api/servers`: `{figures, groups: [{label, count, sessions, active, cpu, mem, rows: [{pid, start, build, up, active, cpu, mem, path, stopping}]}]}` or `{unsupported}`
  - `/api/trello`: `{boards, updatedAt, waiting: {rows, error?}, ready: {boards, error?}, progress: {boards, error?}}` or `{error}`
  - `/api/search`: `{cards}` or `{error}`; HTTP 400 text for a query that is too long
  - stop: `POST` with the `X-Trello-MCP: 1` header.
- Produces: the page. Its design is the mockup `.superpowers/brainstorm/42452-1792096558/content/dashboard.html`.

- [ ] **Step 1: Write the failing test**

Add to `mod tests` in `src/dashboard/mod.rs`:

```rust
    #[test]
    fn page_never_injects_html() {
        let page = std::str::from_utf8(PAGE).unwrap();
        for sink in ["innerHTML", "outerHTML", "insertAdjacentHTML", "document.write"] {
            assert!(!page.contains(sink), "{sink}: Trello text must only ever be set as text");
        }
        for wired in ["/api/servers", "/api/trello", "/api/search", "X-Trello-MCP", "open-groups", "localStorage"] {
            assert!(page.contains(wired), "{wired}");
        }
    }
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `rtk proxy cargo test --locked page_never_injects_html 2>&1 | tail -8`
Expected: FAIL, "/api/servers", because the page is still the one-line placeholder from Task 4.

- [ ] **Step 3: Write the page**

Replace `web/index.html` with:

```html
<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Trello MCP</title>
<style>
/* claude-usage's Swiss sheet: one grotesque, ink on paper, rules not boxes; Swiss red only for the action the page asks for and for alarms. */
:root { color-scheme: light dark; --paper:#fff; --ink:#111; --muted:#5c5c5c; --hair:#d4d4d4; --red:#da291c; --red-ink:#da291c; --on-red:#fff; }
@media (prefers-color-scheme: dark) { :root:not([data-theme=light]) { color-scheme: dark; --paper:#0d0d0d; --ink:#f2f2f2; --muted:#a3a3a3; --hair:#333; --red-ink:#ff6a5c; } }
:root[data-theme=dark] { color-scheme: dark; --paper:#0d0d0d; --ink:#f2f2f2; --muted:#a3a3a3; --hair:#333; --red-ink:#ff6a5c; }
:root[data-theme=light] { color-scheme: light; }
* { box-sizing: border-box; }
html { scrollbar-color: var(--ink) var(--paper); }
body { margin: 0; background: var(--paper); color: var(--ink); font: 15px/1.4 "Helvetica Neue", Helvetica, Arial, sans-serif; font-variant-numeric: tabular-nums; -webkit-text-size-adjust: 100%; }
main { max-width: 1240px; margin: 0 auto; padding: 32px 24px 64px; }
::selection { background: var(--red); color: var(--on-red); }
:focus-visible { outline: 2px solid var(--red-ink); outline-offset: 2px; }
h1, h2 { margin: 0; font-weight: 700; letter-spacing: -.02em; }
a { color: inherit; }
.mark { display: inline-flex; gap: 4.5px; align-items: flex-start; }
.mark i { width: 9px; background: currentColor; }

/* Masthead: name flush left, state and tools flush right, closed by a heavy rule. */
.mast { display: flex; flex-wrap: wrap; align-items: flex-end; gap: 12px 24px; padding-bottom: 16px; border-bottom: 4px solid var(--ink); }
h1 { display: flex; align-items: center; gap: 14px; font-size: 40px; line-height: 1; }
.tools { margin-left: auto; display: flex; align-items: center; flex-wrap: wrap; gap: 12px 16px; }
.meta { color: var(--muted); font-size: 13px; }

/* Buttons: square, ink outline; red only for the page's one action. */
.btn { display: inline-flex; align-items: center; justify-content: center; min-height: 32px; padding: 4px 12px; font: inherit; font-size: 13px; font-weight: 700; color: var(--ink); background: transparent; border: 2px solid var(--ink); border-radius: 0; cursor: pointer; white-space: nowrap; text-decoration: none; }
@media (hover: hover) { .btn:hover:not(:disabled) { background: var(--ink); color: var(--paper); } }
.btn.primary { background: var(--red); border-color: var(--red); color: var(--on-red); }
@media (hover: hover) { .btn.primary:hover:not(:disabled) { background: var(--ink); border-color: var(--ink); color: var(--paper); } }
.btn.sm { min-height: 26px; padding: 1px 8px; font-size: 12px; border-width: 1.5px; }
.btn.done { background: var(--ink); color: var(--paper); }
.btn:disabled { opacity: .4; cursor: default; }
@media (pointer: coarse) { .btn { min-height: 44px; } .btn.sm { min-height: 36px; } }
.seg { display: flex; }
.seg button { font: inherit; font-size: 11px; font-weight: 700; letter-spacing: .06em; padding: 4px 8px 3px; border: 1px solid var(--ink); margin-left: -1px; border-radius: 0; background: transparent; color: var(--ink); cursor: pointer; }
.seg button[aria-pressed=true] { background: var(--ink); color: var(--paper); }
.label { font-size: 11px; font-weight: 500; letter-spacing: .06em; text-transform: uppercase; color: var(--muted); }

.search { margin-top: 28px; }
.box { display: flex; margin-top: 6px; }
.box input { flex: 1; min-width: 0; height: 48px; padding: 0 14px; font: inherit; font-size: 17px; color: var(--ink); background: var(--paper); border: 2px solid var(--ink); border-right: 0; border-radius: 0; }
.box .btn { min-height: 48px; padding: 0 18px; }

.sec { display: flex; align-items: baseline; gap: 12px; margin-top: 44px; padding-bottom: 10px; border-bottom: 4px solid var(--ink); font-size: 22px; }
.sec .n.red { color: var(--red-ink); }
.sec .aside { margin-left: auto; font-size: 13px; font-weight: 400; color: var(--muted); letter-spacing: 0; }
.err { margin-top: 12px; padding: 10px 14px; border: 2px solid var(--red-ink); color: var(--red-ink); font-weight: 700; overflow-wrap: anywhere; }
.empty { margin: 0; padding: 16px 0; color: var(--muted); border-bottom: 1px solid var(--hair); }
.small { color: var(--muted); font-size: 12.5px; overflow-wrap: anywhere; }
.no { color: var(--muted); font-weight: 400; }

/* Waiting on you: card, the mention, the actions. */
.wait { display: grid; grid-template-columns: minmax(0, 1.3fr) minmax(0, 1.5fr) auto; gap: 8px 24px; align-items: center; padding: 14px 0; border-bottom: 1px solid var(--ink); }
.wait .t { font-size: 17px; font-weight: 700; line-height: 1.25; overflow-wrap: anywhere; }
.wait .t a { text-decoration: none; }
@media (hover: hover) { .wait .t a:hover { text-decoration: underline; } }
.quote { margin: 0; font-size: 14px; border-left: 3px solid var(--ink); padding-left: 10px; overflow-wrap: anywhere; }
.quote .small { display: block; margin-top: 2px; }
.acts { display: flex; gap: 8px; justify-content: flex-end; flex-wrap: wrap; }

/* Ready for dev | In progress. */
.cols { display: grid; grid-template-columns: 1fr 1fr; gap: 0 40px; }
.board { margin-top: 14px; display: flex; gap: 8px; align-items: baseline; padding-bottom: 4px; border-bottom: 1px solid var(--ink); font-size: 13px; font-weight: 700; }
.board a { text-decoration: none; }
.board span { color: var(--muted); font-weight: 400; }
.tk { display: grid; grid-template-columns: 52px minmax(0, 1fr) auto; gap: 10px; align-items: center; padding: 8px 0; border-bottom: 1px solid var(--hair); font-size: 14px; }
.tk .no { text-align: right; font-size: 13px; }
.tk .tt { min-width: 0; }
.tk .tt a { display: block; font-weight: 700; text-decoration: none; overflow: hidden; white-space: nowrap; text-overflow: ellipsis; }
@media (hover: hover) { .tk .tt a:hover { text-decoration: underline; } }
.tk .small { display: block; }

/* Servers: big figures, then the folding table. */
.figs { display: flex; flex-wrap: wrap; gap: 16px 40px; margin: 16px 0 14px; }
.figs b { display: block; font-size: 34px; line-height: 1; letter-spacing: -.02em; margin-top: 4px; }
.sg { display: grid; grid-template-columns: 56px minmax(0, 1fr) 64px 76px 60px 70px 92px; gap: 10px; align-items: center; font-size: 14px; }
.sg .r { text-align: right; }
.sg.th { padding-bottom: 6px; border-bottom: 1px solid var(--ink); }
button.sg { width: 100%; min-height: 40px; padding: 0; font: inherit; font-size: 14px; color: inherit; background: transparent; border: 0; border-bottom: 1px solid var(--hair); border-radius: 0; text-align: left; cursor: pointer; }
@media (hover: hover) { button.sg:hover { background: color-mix(in srgb, var(--ink) 6%, transparent); } }
.sg .lab { grid-column: 1 / 4; display: flex; gap: 8px; align-items: center; min-width: 0; font-weight: 700; }
.sg .lab .t { overflow: hidden; white-space: nowrap; text-overflow: ellipsis; }
.sg .lab .n { color: var(--muted); font-weight: 400; font-size: 13px; white-space: nowrap; }
.chev { width: 12px; flex: none; font-size: 10px; color: var(--muted); }
.sg.srv { min-height: 38px; border-bottom: 1px solid var(--hair); }
.sg .b { overflow: hidden; white-space: nowrap; text-overflow: ellipsis; }
.bold { font-weight: 700; }

@media (max-width: 860px) {
  main { padding: 20px 16px 48px; }
  h1 { font-size: 30px; }
  .cols, .wait { grid-template-columns: 1fr; }
  .acts { justify-content: flex-start; }
  .sg { grid-template-columns: 48px minmax(0, 1fr) 56px 50px 84px; }
  .sg .up, .sg .cpu { display: none; }
  .sg .lab { grid-column: 1 / 3; }
  .figs b { font-size: 26px; }
}
</style>
</head>
<body>
<main>
  <header class="mast">
    <h1><span class="mark" aria-hidden="true"><i style="height:36px"></i><i style="height:23px"></i><i style="height:29px"></i></span>Trello MCP</h1>
    <div class="tools">
      <span class="meta" id="meta">Loading…</span>
      <button class="btn" id="refresh" type="button">Refresh</button>
      <div class="seg" id="theme" role="group" aria-label="Theme">
        <button type="button" data-t="auto">AUTO</button><button type="button" data-t="light">LIGHT</button><button type="button" data-t="dark">DARK</button>
      </div>
    </div>
  </header>

  <section class="search">
    <label class="label" for="q">Search cards: words, a card number, or a card URL</label>
    <form class="box" id="search"><input id="q" type="search" autocomplete="off" placeholder="Search cards"><button class="btn" type="submit">Search</button></form>
    <div id="results" aria-live="polite"></div>
  </section>

  <h2 class="sec">Waiting on you <span class="n" id="waiting-n"></span><span class="aside">@mentions from the last 30 days you haven't commented after</span></h2>
  <div id="waiting"></div>

  <div class="cols">
    <div><h2 class="sec">Ready for dev <span class="n" id="ready-n"></span></h2><div id="ready"></div></div>
    <div><h2 class="sec">In progress <span class="n" id="progress-n"></span></h2><div id="progress"></div></div>
  </div>

  <h2 class="sec">Servers <span class="aside">live · every 2 s</span></h2>
  <div id="servers"></div>
</main>
<script>
'use strict';
const $ = id => document.getElementById(id);
/** An element with a class and text: Trello text is only ever set as text. */
function el(tag, cls, text) {
  const e = document.createElement(tag);
  if (cls) e.className = cls;
  if (text != null) e.textContent = text;
  return e;
}
// Links only ever point at Trello.
const trelloURL = u => /^https:\/\/trello\.com\//.test(u || '') ? u : null;
function link(cls, text, url) {
  const a = el('a', cls, text);
  const u = trelloURL(url);
  if (u) { a.href = u; a.target = '_blank'; a.rel = 'noopener'; }
  return a;
}
const store = {
  get(k) { try { return localStorage.getItem(k); } catch { return null; } },
  set(k, v) { try { localStorage.setItem(k, v); } catch {} },
};
function errorBox(text) { const e = el('div', 'err', text); e.setAttribute('role', 'alert'); return e; }
const join = (...parts) => parts.filter(Boolean).join(' · ');
const plural = (n, word) => `${n} ${word}${n === 1 ? '' : 's'}`;

// Theme: AUTO follows the system; the choice stays in this browser.
function applyTheme(t) {
  if (t === 'light' || t === 'dark') document.documentElement.dataset.theme = t;
  else { delete document.documentElement.dataset.theme; t = 'auto'; }
  for (const b of $('theme').querySelectorAll('button')) b.setAttribute('aria-pressed', String(b.dataset.t === t));
}
$('theme').addEventListener('click', e => {
  const t = e.target.closest('button')?.dataset.t;
  if (t) { store.set('theme', t); applyTheme(t); }
});
applyTheme(store.get('theme'));

async function getJSON(url) {
  const r = await fetch(url, { cache: 'no-store' });
  if (!r.ok) throw new Error((await r.text()).trim() || `HTTP ${r.status}`);
  return r.json();
}

async function copy(btn, text) {
  const was = btn.textContent;
  try { await navigator.clipboard.writeText(text); btn.textContent = 'Copied'; }
  catch { btn.textContent = 'Copy failed'; }
  btn.classList.add('done');
  setTimeout(() => { btn.textContent = was; btn.classList.remove('done'); }, 1200);
}
function copyBtn(label, text, primary) {
  const b = el('button', 'btn sm' + (primary ? ' primary' : ''), label);
  b.type = 'button';
  b.addEventListener('click', () => copy(b, text));
  return b;
}

// Cards.
const members = c => c.members.length ? c.members.join(', ') : 'no members';
const activity = c => c.activity ? 'active ' + c.activity : '';
const number = c => c.number != null ? '#' + c.number : '';
function cardRow(c, detail, actions) {
  const row = el('div', 'tk');
  const tt = el('span', 'tt');
  tt.append(link('', c.name, c.url), el('span', 'small', detail));
  const acts = el('span', 'acts');
  acts.append(...actions);
  row.append(el('span', 'no', number(c)), tt, acts);
  return row;
}

function renderWaiting(w) {
  const rows = w.rows || [];
  $('waiting-n').textContent = rows.length || '';
  $('waiting-n').classList.toggle('red', rows.length > 0);
  const out = w.error ? [errorBox(w.error)] : [];
  if (!rows.length && !w.error) out.push(el('p', 'empty', 'Nothing waiting on you.'));
  for (const c of rows) {
    const row = el('div', 'wait');
    const card = el('div');
    const t = el('div', 't');
    t.append(el('span', 'no', number(c) + ' '), link('', c.name, c.url));
    card.append(t, el('div', 'small', join(c.board, c.list)));
    const quote = el('blockquote', 'quote', c.mention);
    quote.append(el('span', 'small', join(c.by, c.age, c.earlier ? `+${plural(c.earlier, 'earlier mention')}` : '')));
    const acts = el('div', 'acts');
    acts.append(copyBtn('Copy comment link', c.commentUrl, true), link('btn sm', 'Open', c.url));
    row.append(card, quote, acts);
    out.push(row);
  }
  $('waiting').replaceChildren(...out);
}

function renderLists(id, section, name) {
  const boards = section.boards || [];
  $(id + '-n').textContent = boards.reduce((n, b) => n + b.cards.length, 0) || '';
  const out = section.error ? [errorBox(section.error)] : [];
  if (!boards.length) out.push(el('p', 'empty', `No cards in ${name} lists.`));
  for (const b of boards) {
    const head = el('div', 'board');
    head.append(link('', b.board, b.url), el('span', '', String(b.cards.length)));
    out.push(head);
    for (const c of b.cards) out.push(cardRow(c, join(members(c), activity(c)), [copyBtn('Copy', c.url)]));
  }
  $(id).replaceChildren(...out);
}

let loadingTrello = false;
async function loadTrello(fresh) {
  if (loadingTrello) return;
  loadingTrello = true;
  $('refresh').disabled = true;
  try {
    const d = await getJSON('/api/trello' + (fresh ? '?fresh=1' : ''));
    if (d.error) throw new Error(d.error);
    $('meta').textContent = `${plural(d.boards, 'board')} · updated ${new Date(d.updatedAt * 1000).toLocaleTimeString()}`;
    renderWaiting(d.waiting);
    renderLists('ready', d.ready, 'Ready for dev');
    renderLists('progress', d.progress, 'In progress');
  } catch (e) {
    $('meta').textContent = 'Trello unavailable';
    for (const id of ['waiting', 'ready', 'progress']) { $(id).replaceChildren(errorBox(e.message)); $(id + '-n').textContent = ''; }
  } finally {
    loadingTrello = false;
    $('refresh').disabled = false;
  }
}
$('refresh').addEventListener('click', () => loadTrello(true));

// Search: 300 ms after typing stops, at once on Enter; only the latest answer shows.
let searchTimer, searchSeq = 0;
async function search() {
  const q = $('q').value.trim();
  const seq = ++searchSeq;
  if (!q) { $('results').replaceChildren(); return; }
  try {
    const d = await getJSON('/api/search?q=' + encodeURIComponent(q));
    if (seq !== searchSeq) return;
    if (d.error) throw new Error(d.error);
    $('results').replaceChildren(...(d.cards.length
      ? d.cards.map(c => cardRow(c, join(c.board, c.list, members(c), activity(c)), [copyBtn('Copy link', c.url), link('btn sm', 'Open', c.url)]))
      : [el('p', 'empty', 'No cards found.')]));
  } catch (e) {
    if (seq === searchSeq) $('results').replaceChildren(errorBox(e.message));
  }
}
$('q').addEventListener('input', () => { clearTimeout(searchTimer); searchTimer = setTimeout(search, 300); });
$('search').addEventListener('submit', e => { e.preventDefault(); clearTimeout(searchTimer); search(); });

// Servers: folded by default; open groups are remembered in this browser.
let open = new Set();
try { open = new Set(JSON.parse(store.get('open-groups') || '[]')); } catch {}
let last = null, confirming = null, confirmTimer = null, serversError = '';

function serverRow(r) {
  const key = `${r.pid}-${r.start}`;
  const row = el('div', 'sg srv');
  for (const [cls, v] of [['r bold', r.pid], ['b', r.build], ['r up', r.up], ['r', r.active], ['r cpu', r.cpu], ['r bold', r.mem]]) row.append(el('span', cls, String(v)));
  row.children[1].title = r.path;
  const armed = confirming === key && !r.stopping;
  const b = el('button', 'btn sm' + (armed ? ' primary' : ''), r.stopping ? 'Quitting…' : armed ? 'Confirm' : 'Quit');
  b.type = 'button';
  b.dataset.key = 'quit-' + key;
  b.disabled = r.stopping;
  b.setAttribute('aria-label', `${armed ? 'Confirm quit of' : 'Quit'} trello-mcp server ${r.pid}`);
  b.addEventListener('click', () => quit(r, key));
  row.append(b);
  return row;
}

// The first click arms a red Confirm for 3 s; the second stops the server.
async function quit(r, key) {
  if (confirming !== key) {
    confirming = key;
    clearTimeout(confirmTimer);
    confirmTimer = setTimeout(() => { if (confirming === key) { confirming = null; if (last) renderServers(last); } }, 3000);
    if (last) renderServers(last);
    return;
  }
  confirming = null;
  clearTimeout(confirmTimer);
  serversError = '';
  try {
    const res = await fetch(`/api/servers/${r.pid}/${r.start}/stop`, { method: 'POST', headers: { 'X-Trello-MCP': '1' } });
    if (!res.ok && res.status !== 404) serversError = (await res.text()).trim();
  } catch (e) { serversError = e.message; }
  loadServers();
}

function groupHead(g) {
  const isOpen = open.has(g.label);
  const head = el('button', 'sg');
  head.type = 'button';
  head.dataset.key = 'group-' + g.label;
  head.setAttribute('aria-expanded', String(isOpen));
  const lab = el('span', 'lab');
  const count = plural(g.count, 'server') + (g.sessions ? ' · ' + plural(g.sessions, 'session') : '');
  lab.append(el('span', 'chev', isOpen ? '▼' : '▶'), el('span', 't', g.label), el('span', 'n', count));
  head.append(lab, el('span', 'r', g.active), el('span', 'r cpu', g.cpu), el('span', 'r bold', g.mem), el('span'));
  head.addEventListener('click', () => {
    if (open.has(g.label)) open.delete(g.label); else open.add(g.label);
    store.set('open-groups', JSON.stringify([...open]));
    renderServers(last);
  });
  return head;
}

function renderServers(d) {
  const box = $('servers');
  if (d.unsupported) { box.replaceChildren(el('p', 'empty', 'The server list is available on macOS only.')); return; }
  const focused = document.activeElement?.dataset?.key;
  const out = serversError ? [errorBox(serversError)] : [];
  const figs = el('div', 'figs');
  for (const [name, v] of [['Servers', d.figures.servers], ['Sessions', d.figures.sessions], ['Memory', d.figures.memory], ['CPU', d.figures.cpu]]) {
    const f = el('div');
    f.append(el('div', 'label', name), el('b', '', v));
    figs.append(f);
  }
  out.push(figs);
  if (!d.groups.length) {
    out.push(el('p', 'empty', 'No servers running. Claude Code and Codex start one trello-mcp per session.'));
  } else {
    const th = el('div', 'sg th label');
    for (const [cls, v] of [['r', 'PID'], ['', 'Build'], ['r up', 'Up'], ['r', 'Active'], ['r cpu', 'CPU'], ['r', 'Mem'], ['', '']]) th.append(el('span', cls, v));
    out.push(th);
    for (const g of d.groups) {
      out.push(groupHead(g));
      if (open.has(g.label)) for (const r of g.rows) out.push(serverRow(r));
    }
  }
  box.replaceChildren(...out);
  // A re-render every 2 s must not take the keyboard focus away.
  if (focused) box.querySelector(`[data-key="${CSS.escape(focused)}"]`)?.focus();
}

async function loadServers() {
  try { last = await getJSON('/api/servers'); renderServers(last); }
  catch (e) { $('servers').replaceChildren(errorBox(e.message)); }
}

loadServers();
loadTrello(false);
setInterval(() => { if (!document.hidden) loadServers(); }, 2000);
setInterval(() => { if (!document.hidden) loadTrello(false); }, 5 * 60 * 1000);
document.addEventListener('visibilitychange', () => { if (!document.hidden) { loadServers(); loadTrello(false); } });
</script>
</body>
</html>
```

- [ ] **Step 4: Run the test and check the script's syntax**

Run: `rtk proxy cargo test --locked page_never_injects_html 2>&1 | tail -5`
Expected: PASS.

Run: `sed -n '/<script>/,/<\/script>/p' web/index.html | sed '1d;$d' > /tmp/trello-dash.js && node --check /tmp/trello-dash.js && echo syntax ok`
Expected: `syntax ok`.

- [ ] **Step 5: Look at it in a browser**

```bash
cargo build --locked && (target/debug/trello-mcp dashboard --addr 127.0.0.1:18790 & echo $! > /tmp/dash.pid; sleep 1)
```

Open http://127.0.0.1:18790 with bsk. Ask the user which browser label to use first, as CLAUDE.md requires; a subagent without a label skips this step and says so. Check against the mockup:
- the masthead, the search box and the four sections all appear;
- AUTO / LIGHT / DARK switch the colors;
- a project fold opens, and stays open after a reload;
- Quit turns into a red Confirm for 3 s;
- at a phone width (390 px) the page is one column, Up and CPU are hidden, and nothing scrolls sideways.

Then run `kill "$(cat /tmp/dash.pid)"`.

- [ ] **Step 6: Commit**

```bash
git add web/index.html src/dashboard/mod.rs
git commit -m "feat(dashboard): Swiss page with search, mentions, lists and servers"
```

---

### Task 6: Tray panel: compact, folding groups, theme switch, dashboard buttons

**Files:**
- Modify: `tray/Panel.swift` (the model, building blocks, panel layout, `ServerLine` geometry, `GroupLine`)
- Modify: `tray/Procs.swift:150-154` (replace `groupMeta` with `groupCells`)
- Modify: `tray/main.swift` (the SELFTEST block: update the old checks, add the new ones)

**Interfaces:**
- Produces, for Task 7 and the selftests:
  - `enum Theme: String, CaseIterable { auto, light, dark }` with `var appearance: NSAppearance?`
  - `let dashboardURL = URL(string: "http://127.0.0.1:8790")!`
  - On `PanelModel`: `open: Set<String>`, `theme: Theme`, `tailnet: URL?`, `onDashboard: (URL) -> Void`, `onTheme: (Theme) -> Void`, `toggle(_:)`
  - `groupCells(_ g: ServerGroup, open: Bool, now: Date) -> [String]`, returning `[count, active, cpu, mem]`
  - `GroupLine(name, cells, open:, p, act)` with `name`, `open`, `cells`
  - `drawText(_:_:_:_:_:_:)`
  - `chip(_:_:on:_:)`

- [ ] **Step 1: Update the selftests first (they fail to compile until Step 3)**

Edit `tray/main.swift`'s SELFTEST block:

1. Replace the line `precondition(gs.map(groupMeta) == ["2 sessions · 23 MB", "1 session · 10 MB", "1 session · 10 MB", "20 MB"], "group meta: \(gs.map(groupMeta))")` with:

```swift
precondition(gs.map { groupCells($0, open: true, now: t0) } == [["3 · 2 sessions", "—", "—", "23 MB"], ["1 · 1 session", "—", "—", "10 MB"],
                                                               ["1 · 1 session", "—", "—", "10 MB"], ["2", "—", "—", "20 MB"]],
             "open group cells: \(gs.map { groupCells($0, open: true, now: t0) })")
precondition(groupCells(gs[0], open: false, now: t0)[0] == "3", "a folded group shows only its server count")
precondition(groupCells(groups(r3, home: h)[0], open: false, now: t0 + 4 + 120) == ["1", "2m ago", "0.1%", "10 MB"], "group totals: latest activity, CPU, memory")
```

2. Run `sed -i '' 's/width == 520/width == 460/g; s/width: 520, height: 800/width: 460, height: 800/' tray/main.swift`, then confirm with `grep -n 520 tray/main.swift` that it prints nothing.

3. After the line `pm.rows = advance([], (1...40).map { i in srv(pid_t(1000 + i), folder: "/Users/me/P/project\(i)", ppid: pid_t(2000 + i), start: s0 + UInt64(i)) }, prevAt: nil, at: t0)`, add:

```swift
pm.open = Set(groups(pm.rows, home: userHome).map(\.label))    // all open: the scroll and tick checks below need the rows
```

4. After the line `precondition(clip().bounds.origin.y == 500, "a tick keeps the scroll offset")`, add:

```swift
// Folding: a folded group hides its rows; a click opens it; a tick updates its totals in place.
let allOpen = pm.open
pm.open = []
RunLoop.main.run(until: Date().addingTimeInterval(0.05))
let folded = views(pc.view).compactMap { $0 as? GroupLine }
precondition(folded.count == 40 && views(pc.view).compactMap { $0 as? ServerLine }.isEmpty && folded.allSatisfy { !$0.open }, "folded: group rows only")
precondition(folded[0].accessibilityPerformPress() && pm.open == [folded[0].name], "a click opens the group")
RunLoop.main.run(until: Date().addingTimeInterval(0.05))
let opened = views(pc.view).compactMap { $0 as? GroupLine }
precondition(views(pc.view).compactMap { $0 as? ServerLine }.count == 1 && opened[0].open && opened[0].cells[0] == "1 · 1 session",
             "the opened group shows its row and sessions: \(opened[0].cells)")
pm.rows = advance(pm.rows, pm.rows.map { r in srv(r.s.id.pid, cpu: 150_000_000, folder: r.s.folder, ppid: r.s.ppid, start: r.s.id.start) }, prevAt: t0 + 2, at: t0 + 4)
precondition(views(pc.view).compactMap { $0 as? GroupLine }[0] === opened[0] && opened[0].cells[2] == "5.0%", "a tick updates a group's totals in place: \(opened[0].cells)")
pm.open = allOpen
RunLoop.main.run(until: Date().addingTimeInterval(0.05))
```

5. Before `print("tray selftest ok")`, add:

```swift
// Footer: Dashboard and via Tailscale open their URLs, no --cli hides both, the chips set the theme.
func presses(_ v: NSView) -> [Press] { ((v as? Press).map { [$0] } ?? []) + v.subviews.flatMap(presses) }
func titled(_ t: String) -> Press? { pc.render(); return presses(pc.view).first { $0.accessibilityLabel() == t } }
/// Sends a button's action to its target, as a click does.
func press(_ b: Press?) {
    guard let b, let t = b.target as? NSObject, let a = b.action else { preconditionFailure("no such button") }
    _ = t.perform(a)
}
var visited: [URL] = []
pm.onDashboard = { visited.append($0) }
pm.tailnet = nil
precondition(titled("via Tailscale") == nil, "no --tailscale: no tailnet button")
press(titled("Dashboard"))
pm.tailnet = URL(string: "https://mac.example.ts.net:8448")
press(titled("via Tailscale"))
precondition(visited == [dashboardURL, URL(string: "https://mac.example.ts.net:8448")!] && dashboardURL.absoluteString == "http://127.0.0.1:8790",
             "dashboard buttons: \(visited)")
let keptCLI = pm.cli
pm.cli = ""
precondition(titled("Dashboard") == nil && titled("via Tailscale") == nil, "no --cli: no dashboard, no buttons")
pm.cli = keptCLI
var themes: [Theme] = []
pm.onTheme = { themes.append($0) }
press(titled("Dark theme"))
precondition(pm.theme == .dark && themes == [.dark], "the DARK chip sets the theme")
precondition(Theme.auto.appearance == nil && Theme.dark.appearance?.name == .darkAqua && Theme.light.appearance?.name == .aqua, "theme appearances")
pm.theme = .auto
precondition(panelSize().width == 460, "the footer with all its buttons keeps the width")
```

- [ ] **Step 2: Run the selftest to verify it fails**

Run: `make tray-test 2>&1 | tail -5`
Expected: compile errors such as "cannot find 'groupCells' in scope" and "value of type 'PanelModel' has no member 'open'".

- [ ] **Step 3: Write the implementation**

`tray/Procs.swift`: replace the `groupMeta` function (its doc comment and body) with:

```swift
/// A group row's cells: the count after its label ("2" folded; "2 · 1 session" open), then its
/// latest ACTIVE, total CPU and total memory, under the table's columns.
func groupCells(_ g: ServerGroup, open: Bool, now: Date) -> [String] {
    let n = sessionCount(g.rows)
    let count = open && n > 0 ? "\(g.rows.count) · \(n) session\(n == 1 ? "" : "s")" : "\(g.rows.count)"
    return [count, activeText(g.rows.compactMap(\.lastActive).max(), now: now), cpuText(totalCPU(g.rows)), memText(g.rows.reduce(0) { $0 + $1.s.mem })]
}
```

`tray/Panel.swift`:

(a) In `PanelModel`, after `var cli = ""`, add:

```swift
    var open: Set<String> = [] { didSet { changed() } }      // open groups, by label; every group starts folded
    var theme: Theme = .auto { didSet { onTheme(theme); changed() } }
    var tailnet: URL?                                        // the dashboard through Tailscale; nil hides its button
```

In the `// Set by Tray.` block, add:

```swift
    var onDashboard: (URL) -> Void = { _ in }
    var onTheme: (Theme) -> Void = { _ in }
```

After `quitClicked`, add:

```swift
    /// A click on a group row folds or opens it.
    func toggle(_ label: String) { if open.contains(label) { open.remove(label) } else { open.insert(label) } }
```

(b) After the `PanelModel` class, before `// MARK: - Palette`, add:

```swift
/// The panel's look: Auto follows macOS.
enum Theme: String, CaseIterable {
    case auto, light, dark
    var appearance: NSAppearance? { self == .auto ? nil : NSAppearance(named: self == .dark ? .darkAqua : .aqua) }
}

/// The dashboard `trello-mcp dashboard` serves on this Mac.
let dashboardURL = URL(string: "http://127.0.0.1:8790")!
```

(c) Replace `txtBtn` with a version that takes a size and a height, and add `chip` and `drawText` after it:

```swift
/// A text-only button (the design's .txt): red ink while hot.
func txtBtn(_ p: Pal, _ title: String, _ w: W, size: CGFloat = 13, minHeight: CGFloat = 36, color: NSColor? = nil,
            pad: (CGFloat, CGFloat), key: String = "", _ act: @escaping () -> Void) -> Press {
    let l = label(title, size, w, color ?? p.ink)
    let b = Press(l, pad: pad, minHeight: minHeight, act)
    b.paint = { _, hot in recolor(l, hot ? p.redInk : color ?? p.ink) }
    b.keyEquivalent = key
    b.keyEquivalentModifierMask = .command
    b.setAccessibilityLabel(title)
    return b
}

/// A square chip of the theme switch (the design's chip): ink-filled when chosen or hot.
func chip(_ p: Pal, _ title: String, on: Bool, _ act: @escaping () -> Void) -> Press {
    let l = label(title, 10, .b, em: 0.06, on ? p.paper : p.ink)
    let b = Press(l, pad: (7, 7), minHeight: 18, act)
    b.layer?.borderWidth = 1
    b.layer?.borderColor = p.ink.cgColor
    b.paint = { b, hot in
        b.layer?.backgroundColor = on || hot ? p.ink.cgColor : nil
        recolor(l, on || hot ? p.paper : p.ink)
    }
    b.setAccessibilityLabel("\(title.capitalized) theme")
    return b
}

/// One line of text in `r`, cut at the tail.
func drawText(_ s: String, _ r: NSRect, _ size: CGFloat, _ w: W, _ c: NSColor, _ align: NSTextAlignment) {
    let ps = NSMutableParagraphStyle()
    ps.alignment = align
    ps.lineBreakMode = .byTruncatingTail
    NSAttributedString(string: s, attributes: [.font: hvFont(size, w), .foregroundColor: c, .paragraphStyle: ps]).draw(in: r)
}
```

(d) `let panelWidth: CGFloat = 520` becomes `460`.

(e) In `PanelController`:
- rename `private var groupMetas: [String: GroupLine] = [:]` to `private var groupLines: [String: GroupLine] = [:]`, and `groupMetas = [:]` in `render()` to `groupLines = [:]`;
- in `render()`, change `vstack(middle(), 16, inset: edges(0, 0, 0, 16))` to `vstack(middle(), 10, inset: edges(0, 0, 0, 16))`;
- change the `col` line to `let col = vstack([pad(header(), edges(0, 0, 0, 16)), scroll, pad(footer(), edges(0, 0, 0, 16))], 10, inset: edges(12, 16, 10, 0))`;
- in `tick()`, replace the `groupMetas` line with `for g in groups(m.rows, home: home) { groupLines[g.label]?.cells = groupCells(g, open: m.open.contains(g.label), now: now) }`.

Replace `header()`, `footer()`, `caps()`, `servers()`, `line()` and `groupHead()` with:

```swift
    func header() -> NSView {
        let top = hstack([hstack([boardMark(16, p.ink), label("Trello MCP", 13, .b, em: -0.02, p.ink)], 7), spacer(),
                          label("live · every 2 s", 11, p.muted)], 12)
        return vstack([top, block(p.ink, h: 4)], 8)
    }

    /// Dashboard links and the theme switch, then the tray's own usage, Open Log and Quit.
    func footer() -> NSView {
        var links: [NSView] = []
        if !m.cli.isEmpty {     // without --cli the tray starts no dashboard
            links.append(txtBtn(p, "Dashboard", .b, size: 12.5, minHeight: 30, pad: (0, 8), key: "d") { [unowned self] in m.onDashboard(dashboardURL) })
            if let u = m.tailnet {
                links.append(txtBtn(p, "via Tailscale", .b, size: 12.5, minHeight: 30, pad: (8, 8)) { [unowned self] in m.onDashboard(u) })
            }
        }
        let themes = hstack(Theme.allCases.map { t in chip(p, t.rawValue.uppercased(), on: m.theme == t) { [unowned self] in m.theme = t } }, 0)
        let s = label(m.stats.isEmpty ? "Tray: —" : m.stats, 11, p.muted)
        statsRef = s
        let tools = hstack([s, spacer(),
                            txtBtn(p, "Open Log", .r, size: 11, minHeight: 24, pad: (8, 8), key: "l") { [unowned self] in m.onLog() },
                            txtBtn(p, "Quit", .r, size: 11, minHeight: 24, pad: (8, 0), key: "q") { [unowned self] in m.onQuit() }], 0)
        return vstack([block(p.ink, h: 1), hstack(links + [spacer(), themes], 4), tools], 2)
    }

    func caps(_ s: String, align: NSTextAlignment = .left) -> NSTextField { label(s, 10, .m, em: 0.06, p.muted, align: align) }

    // MARK: Servers

    /// Figures (servers, sessions, memory, CPU), then the table grouped by session folder: a group
    /// row each, with its servers under it while it is open.
    func servers() -> NSView {
        let now = m.now()
        let figs = hstack(zip(["SERVERS", "SESSIONS", "MEMORY", "CPU"], serverFigures(m.rows)).map { n, v in
            let f = label(v, 17, .b, em: -0.02, p.ink)
            figures.append(f)
            return vstack([caps(n), f], 1)
        } + [spacer()], 22, top: true)
        let head = line(caps("PID", align: .right), caps("BUILD"), caps("UP", align: .right), caps("ACTIVE", align: .right),
                        caps("CPU", align: .right), caps("MEM", align: .right), NSView())
        let rows = Rows(groups(m.rows, home: home).flatMap { g in
            [groupHead(g, now) as NSView] + (m.open.contains(g.label) ? g.rows.map { server($0, now) } : [])
        })
        let s = vstack([figs, vstack([head, block(p.ink, h: 1)], 4), rows])
        s.setCustomSpacing(10, after: figs)
        return s
    }

    /// A table line in fixed columns: PID, Build (flexible), Up, Active, CPU, Mem, action.
    func line(_ pid: NSView, _ build: NSView, _ up: NSView, _ active: NSView, _ cpu: NSView, _ mem: NSView, _ action: NSView) -> NSView {
        build.setContentCompressionResistancePriority(.init(100), for: .horizontal)
        return hstack([fixed(pid, w: 40), stretch(build), fixed(up, w: 44), fixed(active, w: 52), fixed(cpu, w: 40), fixed(mem, w: 46), fixed(action, w: 56)], 6)
    }

    /// A group's row: a button that folds it, with its totals under the table's columns.
    func groupHead(_ g: ServerGroup, _ now: Date) -> GroupLine {
        let open = m.open.contains(g.label)
        let row = GroupLine(g.label, groupCells(g, open: open, now: now), open: open, p) { [unowned self] in m.toggle(g.label) }
        groupLines[g.label] = row
        return row
    }
```

(f) In `ServerLine`:
- `intrinsicContentSize` height `31` → `26`, and the `init` frame height `31` → `26`;
- `buttonRect` becomes `NSRect(x: bounds.width - 56, y: 3.5, width: 56, height: 19)`;
- delete its `private func text(...)`;
- replace `draw` with:

```swift
    /// Column rects from the right: button 56, mem 46, cpu 40, active 52, up 52, with 6 pt gaps; pid 40 at the left, build takes the rest.
    override func draw(_ dirty: NSRect) {
        let w = bounds.width
        let cols: [(CGFloat, CGFloat, NSTextAlignment, W)] = [
            (0, 40, .right, .b), (46, w - 314, .left, .r), (w - 262, 44, .right, .r),
            (w - 212, 52, .right, .r), (w - 154, 40, .right, .r), (w - 108, 46, .right, .b),
        ]
        for (v, c) in zip(values, cols) { drawText(v, NSRect(x: c.0, y: 5, width: c.1, height: 16), 12, c.3, p.ink, c.2) }
        // The button: outline Quit, red Confirm, dim Quitting…; ink fill while hot.
        let b = buttonRect, enabled = action != .stopping
        let (bg, fg, bd) = buttonColors(action, hot: hot, p: p)
        let path = NSBezierPath(rect: b.insetBy(dx: 0.75, dy: 0.75))
        if let bg { bg.setFill(); path.fill() }
        bd.withAlphaComponent(enabled ? 1 : 0.4).setStroke()
        path.lineWidth = 1.5
        path.stroke()
        drawText(title, NSRect(x: b.minX, y: b.minY + 3, width: b.width, height: 13), 10.5, .b, fg.withAlphaComponent(enabled ? 1 : 0.4), .center)
        p.hair.setFill()
        NSRect(x: 0, y: bounds.height - 1, width: w, height: 1).fill()
    }
```

(g) Replace the whole `GroupLine` class with:

```swift
/// A group row drawn in one view, and a button that folds the group: chevron, label (cut in the
/// middle) and count, then the group's ACTIVE, CPU and memory under the table's columns.
final class GroupLine: NSView {
    var cells: [String] { didSet { if cells != oldValue { needsDisplay = true } } }     // count, active, cpu, mem
    let name: String, open: Bool
    private let p: Pal, act: () -> Void
    override var isFlipped: Bool { true }
    override var intrinsicContentSize: NSSize { NSSize(width: NSView.noIntrinsicMetric, height: 28) }

    init(_ name: String, _ cells: [String], open: Bool, _ p: Pal, _ act: @escaping () -> Void) {
        self.name = name
        self.cells = cells
        self.open = open
        self.p = p
        self.act = act
        super.init(frame: NSRect(x: 0, y: 0, width: inner, height: 28))
    }

    required init?(coder: NSCoder) { fatalError() }

    override func draw(_ dirty: NSRect) {
        let w = bounds.width
        drawText(open ? "▼" : "▶", NSRect(x: 0, y: 8, width: 12, height: 12), 8, .r, p.muted, .left)
        let count = NSAttributedString(string: cells[0], attributes: [.font: hvFont(11), .foregroundColor: p.muted])
        let cw = ceil(count.size().width)
        let np = NSMutableParagraphStyle()
        np.lineBreakMode = .byTruncatingMiddle      // a long folder keeps both ends
        let label = NSAttributedString(string: name, attributes: [.font: hvFont(12.5, .b), .foregroundColor: p.ink, .paragraphStyle: np])
        let lw = min(ceil(label.size().width), w - 268 - 16 - cw - 6)    // up to the Up column
        label.draw(in: NSRect(x: 16, y: 6, width: lw, height: 17))
        count.draw(in: NSRect(x: 16 + lw + 6, y: 8, width: cw, height: 15))
        drawText(cells[1], NSRect(x: w - 212, y: 6, width: 52, height: 16), 12, .r, p.muted, .right)
        drawText(cells[2], NSRect(x: w - 154, y: 6, width: 40, height: 16), 12, .r, p.muted, .right)
        drawText(cells[3], NSRect(x: w - 108, y: 6, width: 46, height: 16), 12, .b, p.ink, .right)
        p.hair.setFill()
        NSRect(x: 0, y: bounds.height - 1, width: w, height: 1).fill()
    }

    // Like a button: the click lands on mouse-up inside.
    override func mouseDown(with e: NSEvent) {}
    override func mouseUp(with e: NSEvent) { if bounds.contains(convert(e.locationInWindow, from: nil)) { act() } }

    override func isAccessibilityElement() -> Bool { true }
    override func accessibilityRole() -> NSAccessibility.Role? { .button }
    override func accessibilityLabel() -> String? { "\(name), \(open ? "open" : "folded")" }
    override func accessibilityPerformPress() -> Bool { act(); return true }
}
```

- [ ] **Step 4: Run the selftest to verify it passes**

Run: `make tray-test 2>&1 | tail -5`
Expected: `tray selftest ok`. If `folded.count == 40` fails, check that `pm.open = []` re-rendered: the panel must be hosted (`host.orderFront`) for a queued render to run.

- [ ] **Step 5: Commit**

```bash
git add tray/Panel.swift tray/Procs.swift tray/main.swift
git commit -m "feat(tray): compact Swiss panel, folding projects, theme switch, dashboard links"
```

---

### Task 7: Tray app: start the dashboard, pass --tailscale, keep the theme

**Files:**
- Modify: `tray/Procs.swift` (pure helpers after `cliPath`)
- Modify: `tray/main.swift` (the `Tray` class, plus the SELFTEST block)

**Interfaces:**
- Consumes: `PanelModel.theme/onTheme/tailnet/onDashboard/cli`, `Theme.appearance` and `dashboardURL` from Task 6; `stopGrace`, `args(of:)`, `procName`, `isServer` from `Procs.swift`.
- Produces:
  - `tailscaleArg(_:)`, `tailnetURL(_:)`, `dashboardArgs(_:)`, `restartDelay(_:uptime:)`
  - on `Tray`: `tailscale`, `dashboard: pid_t`, `dashDelay`, `startDashboard()`, `stopDashboard(then:)`

- [ ] **Step 1: Write the failing selftests**

In `tray/main.swift`:

1. Replace the line `if CommandLine.arguments.count > 1, CommandLine.arguments[1] == "serve" { sleep(30); exit(0) }` with:

```swift
if CommandLine.arguments.count > 1, ["serve", "dashboard"].contains(CommandLine.arguments[1]) { sleep(30); exit(0) }
```

and on the comment line above it, change "The fake server below is" to "The fake server and dashboard below are".

2. After the `realPath` precondition line (`--cli resolves symlinks`), add:

```swift
// The tailnet URL, the dashboard's arguments and its restart delay.
precondition(tailscaleArg(["tray", "--cli", "/x", "--tailscale", "h.ts.net:8448=me@x.io"]) == "h.ts.net:8448=me@x.io"
    && tailscaleArg(["tray"]) == "" && tailscaleArg(["tray", "--tailscale"]) == "", "--tailscale")
precondition(tailnetURL("h.ts.net:8448=me@x.io") == URL(string: "https://h.ts.net:8448") && tailnetURL("") == nil && tailnetURL("h:1") == nil
    && tailnetURL("=me@x") == nil && tailnetURL("h:1=") == nil, "tailnet URL")
precondition(dashboardArgs("") == ["dashboard"] && dashboardArgs("h:1=me") == ["dashboard", "--tailscale", "h:1=me"], "dashboard args")
precondition(restartDelay(nil, uptime: 0) == 1 && restartDelay(1, uptime: 2) == 2 && restartDelay(32, uptime: 2) == 60
    && restartDelay(60, uptime: 2) == 60 && restartDelay(8, uptime: 300) == 1, "restart delay: 1 s, doubling to 60 s, 1 s again after 5 minutes up")
precondition(!isServer(["trello-mcp", "dashboard", "--tailscale", "h:1=me"]), "the dashboard is not a server")
```

3. Before the line `try? FileManager.default.removeItem(at: dir)`, add:

```swift
// The dashboard child: the CLI's dashboard arguments, restarted 1 s after it dies, stopped by Quit and not restarted.
tr.model.cli = fake.path
tr.tailscale = "h.ts.net:8448=me@x.io"
tr.startDashboard()
let d1 = tr.dashboard
precondition(d1 > 0, "the dashboard starts")
spinUntil(2) { args(of: d1)?.count == 4 }
precondition(args(of: d1) == [fake.path, "dashboard", "--tailscale", "h.ts.net:8448=me@x.io"], "dashboard args: \(String(describing: args(of: d1)))")
scanNow()
precondition(!tr.model.rows.contains { $0.id.pid == d1 }, "the dashboard is not listed as a server")
kill(d1, SIGKILL)
spinUntil(4) { tr.dashboard != 0 && tr.dashboard != d1 }
let d2 = tr.dashboard
precondition(d2 > 0 && d2 != d1 && tr.dashDelay == 1, "a dead dashboard is restarted after 1 s")
var quitDone = false
tr.stopDashboard { quitDone = true }
spinUntil(3) { quitDone }
precondition(quitDone && tr.dashboard == 0 && procName(d2) == nil, "Quit stops the dashboard")
RunLoop.main.run(until: Date().addingTimeInterval(1.5))
precondition(tr.dashboard == 0, "a stopped dashboard is not restarted")
tr.model.cli = ""
```

- [ ] **Step 2: Run the selftest to verify it fails**

Run: `make tray-test 2>&1 | tail -5`
Expected: compile errors such as "cannot find 'tailscaleArg' in scope" and "value of type 'Tray' has no member 'startDashboard'".

- [ ] **Step 3: Write the implementation**

`tray/Procs.swift`: after `cliPath`, add:

```swift
/// --tailscale host:port=login from the LaunchAgent; "" without one.
func tailscaleArg(_ args: [String]) -> String {
    if let i = args.firstIndex(of: "--tailscale"), i + 1 < args.count { return args[i + 1] }
    return ""
}

/// The dashboard through `tailscale serve`: https://host:port; nil unless --tailscale is host:port=login.
func tailnetURL(_ ts: String) -> URL? {
    let parts = ts.components(separatedBy: "=")
    guard parts.count == 2, !parts[0].isEmpty, !parts[1].isEmpty else { return nil }
    return URL(string: "https://\(parts[0])")
}

/// `trello-mcp dashboard`'s arguments, with the tray's --tailscale.
func dashboardArgs(_ ts: String) -> [String] { ["dashboard"] + (ts.isEmpty ? [] : ["--tailscale", ts]) }

/// Seconds before restarting a dashboard that exited: 1, doubling to 60 while it keeps dying
/// young, 1 again once it ran 5 minutes (a busy port or a bad build must not spin).
func restartDelay(_ last: TimeInterval?, uptime: TimeInterval) -> TimeInterval {
    guard let last, uptime < 300 else { return 1 }
    return min(last * 2, 60)
}
```

`tray/main.swift`, in `final class Tray`:

(a) After `var minuteTimer: Timer?`, add:

```swift
    var tailscale = ""          // --tailscale host:port=login; "" without one
    var dashboard: pid_t = 0    // the dashboard child; 0: not running
    var dashWatch: DispatchSourceProcess?
    var dashDelay: TimeInterval?    // the last restart delay
    var quitting = false        // Quit is stopping the dashboard: no restart
```

(b) In `applicationDidFinishLaunching`, after `model.cli = realPath(cliPath(CommandLine.arguments))`, add:

```swift
        tailscale = tailscaleArg(CommandLine.arguments)
        model.tailnet = tailnetURL(tailscale)
        model.onDashboard = { [unowned self] in closePanel(); NSWorkspace.shared.open($0) }
        model.theme = Theme(rawValue: UserDefaults.standard.string(forKey: "theme") ?? "") ?? .auto
        model.onTheme = { [unowned self] t in
            UserDefaults.standard.set(t.rawValue, forKey: "theme")
            window.appearance = t.appearance
        }
```

Replace `model.onQuit = { exit(0) }      // exit 0: the LaunchAgent leaves it down until the next login` with:

```swift
        model.onQuit = { [unowned self] in stopDashboard { exit(0) } }     // exit 0: the LaunchAgent leaves it down until the next login
```

After `window.onClose = { [unowned self] in closePanel() }`, add:

```swift
        window.appearance = model.theme.appearance
        if !model.cli.isEmpty { startDashboard() }
```

(c) After `show(error:)`, add:

```swift
    // MARK: Dashboard

    /// Starts `<cli> dashboard`. posix_spawn, not Process: Process gives the child its own process
    /// group, and launchd only reaps the job's group, so a kill -9'd tray would leave the dashboard
    /// on :8790 (gws-mcp's tray runs its dashboard the same way). Its output lands in the tray's log.
    func startDashboard() {
        var pid: pid_t = 0
        let argv: [UnsafeMutablePointer<CChar>?] = ([model.cli] + dashboardArgs(tailscale)).map { strdup($0) } + [nil]
        defer { argv.forEach { free($0) } }
        // Only fds 0-2 reach the dashboard: it lives long and must not hold any other descriptor open.
        var attr: posix_spawnattr_t?
        posix_spawnattr_init(&attr)
        posix_spawnattr_setflags(&attr, Int16(POSIX_SPAWN_CLOEXEC_DEFAULT))
        var actions: posix_spawn_file_actions_t?
        posix_spawn_file_actions_init(&actions)
        for fd: Int32 in 0...2 { posix_spawn_file_actions_addinherit_np(&actions, fd) }
        defer {
            posix_spawnattr_destroy(&attr)
            posix_spawn_file_actions_destroy(&actions)
        }
        let rc = posix_spawn(&pid, model.cli, &actions, &attr, argv, environ)
        guard rc == 0 else { return NSLog("trello-mcp dashboard failed to start: \(String(cString: strerror(rc)))") }
        let started = Date()
        let src = DispatchSource.makeProcessSource(identifier: pid, eventMask: .exit, queue: .main)
        src.setEventHandler { [unowned self] in
            var status: Int32 = 0
            waitpid(pid, &status, 0)
            dashboardExited(status: status, uptime: Date().timeIntervalSince(started))
        }
        src.resume()
        dashWatch = src
        dashboard = pid
    }

    /// A dashboard that exited on its own is started again after `restartDelay`.
    func dashboardExited(status: Int32, uptime: TimeInterval) {
        dashWatch?.cancel()
        dashWatch = nil
        dashboard = 0
        guard !quitting else { return }
        let delay = restartDelay(dashDelay, uptime: uptime)
        dashDelay = delay
        NSLog("trello-mcp dashboard exited (wait status \(status)), restarting in \(Int(delay)) s")
        DispatchQueue.main.asyncAfter(deadline: .now() + delay) { [unowned self] in
            if dashboard == 0 && !quitting { startDashboard() }
        }
    }

    /// Quit: SIGTERM, SIGKILL after 5 s; `then` runs once the dashboard is gone, at once when none runs.
    func stopDashboard(then: @escaping () -> Void) {
        quitting = true
        guard dashboard != 0, let src = dashWatch else { return then() }
        let pid = dashboard
        src.setEventHandler { [unowned self] in
            var status: Int32 = 0
            waitpid(pid, &status, 0)
            src.cancel()
            dashWatch = nil
            dashboard = 0
            then()
        }
        kill(pid, SIGTERM)
        DispatchQueue.main.asyncAfter(deadline: .now() + stopGrace) { [unowned self] in
            if dashboard == pid { kill(pid, SIGKILL) }
        }
    }
```

- [ ] **Step 4: Run the selftest to verify it passes**

Run: `make tray-test 2>&1 | tail -5`
Expected: `tray selftest ok`.

- [ ] **Step 5: Commit**

```bash
git add tray/Procs.swift tray/main.swift
git commit -m "feat(tray): start and supervise the dashboard; remember the theme"
```

---

### Task 8: Install wiring, docs, full check

**Files:**
- Modify: `Makefile` (TS vars, `tailnet` target, `@TAILSCALE@` in `tray-install`, `.PHONY`)
- Modify: `tray/com.trellomcp.tray.plist` (`--tailscale @TAILSCALE@`)
- Modify: `README.md` (the "Menu bar app (macOS)" section)
- Modify: `CHANGELOG.md` (`[Unreleased]`)
- Modify: the comment header at the top of `tray/main.swift` (it mentions what the tray does)

**Interfaces:**
- Consumes: everything above.
- Produces: `make tailnet`, and a plist whose `ProgramArguments` index 4 is the `--tailscale` value (the Makefile's `TAILSCALE ?=` reads index 4 back on reinstall).

- [ ] **Step 1: The plist**

In `tray/com.trellomcp.tray.plist`, after `<string>@CLI@</string>`, add:

```xml
		<!-- host:port=login from `make tailnet`; empty: no "via Tailscale" button. -->
		<string>--tailscale</string>
		<string>@TAILSCALE@</string>
```

- [ ] **Step 2: The Makefile**

After the `APP_DIR := ...` line, add:

```make
# make tailnet: this Mac's tailnet name and its owner's login (needs tailscale and jq), read only when used.
TS_PORT   ?= 8448
TS_HOST    = $(shell tailscale status --json 2>/dev/null | jq -r '.Self.DNSName | rtrimstr(".")')$(if $(filter 443,$(TS_PORT)),,:$(TS_PORT))
TS_LOGIN   = $(shell tailscale status --json 2>/dev/null | jq -r '.User[(.Self.UserID|tostring)].LoginName // empty')
# host:port=login for the tray's --tailscale; a reinstall keeps what the installed login item passes.
TAILSCALE ?= $(if $(wildcard $(AGENT_PLIST)),$(shell /usr/libexec/PlistBuddy -c 'Print :ProgramArguments:4' '$(AGENT_PLIST)' 2>/dev/null))
```

Add `tailnet` to the `.PHONY` line. In `tray-install`, change the `sed` line to:

```make
	sed -e 's|@HOME@|$(HOME)|g' -e 's|@CLI@|$(CURDIR)/bin/trello-mcp|g' -e 's|@TAILSCALE@|$(TAILSCALE)|g' tray/$(AGENT).plist > $(AGENT).plist.tmp
```

After `tray-uninstall`, add:

```make
tailnet: ## open the dashboard to your own Tailscale login (https port 8448) and reinstall the tray with its link
	@test -n "$(TS_LOGIN)" || { echo "tailnet: tailscale is not logged in (needs tailscale and jq)" >&2; exit 1; }
	tailscale serve --bg --https=$(TS_PORT) http://127.0.0.1:8791
	$(MAKE) tray-install TAILSCALE='$(TS_HOST)=$(TS_LOGIN)'
```

Check it: run `make -n tailnet | head -4`. It should print the `test -n` guard, then `tailscale serve --bg --https=8448 http://127.0.0.1:8791`, then a `make tray-install TAILSCALE='lis-macbook-pro.<tailnet>.ts.net:8448=<login>'` line with this Mac's real tailnet name and login filled in. `-n` only prints the commands; it doesn't run them.

- [ ] **Step 3: Docs**

In `README.md`, in the "Menu bar app (macOS)" section, replace the sentence `The panel is the same layout as gws-mcp's tray.` with:

```markdown
Projects start folded: a folded row shows its server count, latest activity, CPU and memory; a
click opens it. AUTO / LIGHT / DARK in the footer sets the panel's look (Auto follows macOS).
```

After the line `Log: \`~/Library/Logs/trello-mcp-tray.log\`. \`make tray-test\` runs its selftest.`, add:

```markdown
### Dashboard

`trello-mcp dashboard` serves a page on http://127.0.0.1:8790 with:
- the tray's server table;
- Trello card search, by words, a card number or a card URL;
- the @mentions from the last 30 days you haven't commented after, each with a Copy comment link
  button;
- the cards in every "Ready for dev" and "In progress" list on your boards in the allowed
  workspaces.

The tray starts it from this checkout's `bin/trello-mcp`, restarts it if it dies, and opens it
from Dashboard (⌘D).

    make install          # the dashboard runs bin/trello-mcp: rebuild after pulling
    make tailnet          # open it to your own Tailscale login on https port 8448; reinstalls the tray

Only your Tailscale login gets in. `tailscale serve` points at a second loopback listener,
127.0.0.1:8791, which checks the Host and `Tailscale-User-Login`. Never point `tailscale serve`
at 8790: that listener trusts any request from this Mac.
```

In `CHANGELOG.md`, under `## [Unreleased]`, replace the bullet that starts `- Trello MCP menu bar app (macOS):` with:

```markdown
- Trello MCP menu bar app (macOS): lists running servers grouped by session folder with PID, build,
  uptime, last activity, CPU and memory, and quits one (Quit, then Confirm). Compact Swiss panel;
  projects start folded; AUTO / LIGHT / DARK theme. `make tray-install`.
- `trello-mcp dashboard`: a local page (127.0.0.1:8790) with the server table, Trello card search,
  the @mentions waiting on you (copy the comment link), and the cards in "Ready for dev" and
  "In progress" lists. The tray runs it; `make tailnet` opens it to your own Tailscale login.
```

At the top of `tray/main.swift`, in the header comment, change `// can stop one.` to `// can stop one, and runs \`trello-mcp dashboard\` (the same list plus Trello views, in a browser).`

- [ ] **Step 4: The full check**

Run: `rtk proxy make check 2>&1 | tail -20`
Expected: formatting, clippy and tests are all clean, and the output ends with `tray selftest ok`. Confirm with `git diff --stat master -- tests/fixtures/tools_list.json` that the tool list has not changed (no output).

- [ ] **Step 5: Install and look**

```bash
make install && make tray-install
```

Then:
- open the menu bar panel and compare it with `.superpowers/brainstorm/42452-1792096558/content/tray-v2.html`: 460 pt, folded projects, the footer with Dashboard and the theme chips;
- switch DARK and LIGHT;
- press ⌘D: http://127.0.0.1:8790 opens;
- `pgrep -fl 'trello-mcp dashboard'` shows one dashboard;
- quit the tray from its footer: `pgrep -fl 'trello-mcp dashboard'` then shows none;
- start the tray again with `launchctl kickstart gui/$(id -u)/com.trellomcp.tray`.

Ask the user before running `make tailnet`: it changes the tailnet's `tailscale serve` config, which is outward-facing. Once they say yes, run `make tailnet`. Then ask them to open the "via Tailscale" link on their phone and check the page in one column.

- [ ] **Step 6: Commit**

```bash
git add Makefile tray/com.trellomcp.tray.plist tray/main.swift README.md CHANGELOG.md
git commit -m "feat(tray): make tailnet; docs for the dashboard"
```
