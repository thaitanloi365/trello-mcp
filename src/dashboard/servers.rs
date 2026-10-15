//! The tray's server rules and text (tray/Procs.swift) for the dashboard: which trello-mcp
//! processes are servers, CPU and activity across scans, grouping by session folder, and the
//! strings the page shows. Pure; procs.rs reads the processes.

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
                return Row {
                    s,
                    cpu: None,
                    last_active: None,
                };
            };
            match prev_at.filter(|p| at > *p) {
                None => {
                    let last_active = if s.cpu_ns < o.s.cpu_ns + ACTIVE_CPU_NS {
                        o.last_active
                    } else {
                        None
                    };
                    Row {
                        s,
                        cpu: None,
                        last_active,
                    }
                }
                Some(p) => {
                    let d = s.cpu_ns.saturating_sub(o.s.cpu_ns);
                    let cpu = d as f64 / 1e9 / (at - p) * 100.0;
                    let last_active = if d >= ACTIVE_CPU_NS {
                        Some(at)
                    } else {
                        o.last_active
                    };
                    Row {
                        s,
                        cpu: Some(cpu),
                        last_active,
                    }
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
            if parts.is_empty() {
                f.into()
            } else {
                parts[parts.len().saturating_sub(2)..].join("/")
            }
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
    r.s.parent_started
        .map_or_else(|| "—".into(), |p| countdown(now - p as f64 / 1e6))
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
        by.entry(group_label(r.s.folder.as_deref(), home))
            .or_default()
            .push(r);
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
pub(crate) fn view(
    rows: &[Row],
    stopping: &HashSet<ServerId>,
    now: f64,
    cli: &str,
    home: &str,
) -> Value {
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
        Sample {
            cpu_ns,
            ..srv(1, Some("/Users/me/work/app"), 900, Some(PS0), 10 << 20)
        }
    }

    #[test]
    fn which_processes_are_servers() {
        assert!(
            is_server(&args(&["/x/trello-mcp", "serve"])) && is_server(&args(&["/x/trello-mcp"]))
        );
        assert!(is_server(&args(&[
            "trello-mcp",
            "--config",
            "/a b/c.json",
            "serve"
        ])));
        assert!(is_server(&args(&[
            "trello-mcp",
            "serve",
            "--config",
            "/c.json"
        ])));
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
        assert_eq!(
            parse_proc_args(&buf),
            Some(args(&["trello-mcp", "serve", "--config", "/a b/c.json"]))
        );
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
        assert!(
            r2[0].last_active.is_none() && (r2[0].cpu.unwrap() - 0.1).abs() < 1e-6,
            "under 2 ms is idle"
        );
        let r3 = advance(&r2, vec![one(4_000_000)], Some(t0 + 2.0), t0 + 4.0);
        assert_eq!(
            r3[0].last_active,
            Some(t0 + 4.0),
            "2 ms or more is activity"
        );
        let r4 = advance(&r3, vec![one(4_000_000)], None, t0 + 9000.0);
        assert_eq!(
            (r4[0].cpu, r4[0].last_active),
            (None, Some(t0 + 4.0)),
            "no baseline: no CPU, activity kept"
        );
        let r5 = advance(&r3, vec![one(4_000_000 + ACTIVE_CPU_NS)], None, t0 + 9000.0);
        assert_eq!(
            r5[0].last_active, None,
            "worked in a gap with no baseline: when is unknown"
        );
        let mut reused = one(9_000_000);
        reused.id.start += 1;
        assert_eq!(
            advance(&r3, vec![reused], Some(t0 + 4.0), t0 + 6.0)[0].last_active,
            None,
            "a reused pid starts fresh"
        );
        assert_eq!(baseline(Some(t0), t0 + 180.0), Some(t0));
        assert_eq!(
            baseline(Some(t0), t0 + 181.0),
            None,
            "older than three minute scans"
        );
        assert_eq!(baseline(None, t0), None);
    }

    #[test]
    fn labels_and_text() {
        assert_eq!(
            build_label(
                "/r/trello-mcp/.claude/worktrees/feat-x/bin/trello-mcp",
                "/c"
            ),
            "feat-x"
        );
        assert_eq!(
            [
                build_label("/Users/me/old/bin/trello-mcp", "/c"),
                build_label("trello-mcp", "/c"),
                build_label("/c", "/c")
            ],
            ["old/bin", "trello-mcp", "installed"]
        );
        assert_eq!(
            build_label("/Users/me/old/bin/trello-mcp", ""),
            "old/bin",
            "no --cli never matches"
        );
        let h = "/Users/me";
        assert_eq!(
            group_label(Some("/Users/me/Projects/eventcreate/eventcreate_app"), h),
            "eventcreate/eventcreate_app"
        );
        assert_eq!(
            [
                group_label(Some(h), h),
                group_label(Some("/"), h),
                group_label(Some("/tmp"), h)
            ],
            ["~", "/", "tmp"]
        );
        assert_eq!(
            [group_label(None, h), group_label(Some(""), h)],
            [NO_SESSION, NO_SESSION]
        );
        assert_eq!(
            [
                mem_text(8_598_323),
                mem_text(49 << 20),
                mem_text(1229 << 20)
            ],
            ["8.2 MB", "49 MB", "1.2 GB"]
        );
        assert_eq!(
            [
                cpu_text(None),
                cpu_text(Some(0.04)),
                cpu_text(Some(96.84)),
                cpu_text(Some(143.2))
            ],
            ["—", "0.0%", "96.8%", "143%"]
        );
        assert_eq!(
            [
                countdown(45.0 * 60.0),
                countdown(5.0 * 3600.0 + 12.0 * 60.0),
                countdown(6.0 * 86400.0 + 21.0 * 3600.0)
            ],
            ["45m", "5h 12m", "6d 21h"]
        );
        assert_eq!(
            [
                active_text(None, 0.0),
                active_text(Some(0.0), 59.0),
                active_text(Some(0.0), 720.0)
            ],
            ["—", "now", "12m ago"]
        );
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
        assert_eq!(
            gs.iter().map(|g| g.label.as_str()).collect::<Vec<_>>(),
            ["a/Alpha", "a/zeta", "~", NO_SESSION]
        );
        assert_eq!(
            gs[0].rows.iter().map(|r| r.s.id.pid).collect::<Vec<_>>(),
            [2, 6, 4],
            "oldest session first, then pid"
        );
        assert_eq!(
            gs[3].rows.iter().map(|r| r.s.id.pid).collect::<Vec<_>>(),
            [3, 7]
        );
        let stopping: HashSet<ServerId> = [ServerId { pid: 6, start: S0 }].into();
        let v = view(&mix, &stopping, 1_000_000.0, "/x/trello-mcp", h);
        assert_eq!(
            v["figures"],
            json!({"servers": "7", "sessions": "4", "memory": "63 MB", "cpu": "—"})
        );
        let alpha = &v["groups"][0];
        assert_eq!(
            (
                &alpha["label"],
                &alpha["count"],
                &alpha["sessions"],
                &alpha["active"],
                &alpha["cpu"],
                &alpha["mem"]
            ),
            (
                &json!("a/Alpha"),
                &json!(3),
                &json!(2),
                &json!("—"),
                &json!("—"),
                &json!("23 MB")
            )
        );
        let six = &alpha["rows"][1];
        assert_eq!(
            (
                &six["pid"],
                &six["start"],
                &six["build"],
                &six["mem"],
                &six["stopping"],
                &six["path"]
            ),
            (
                &json!(6),
                &json!(S0),
                &json!("installed"),
                &json!("3.0 MB"),
                &json!(true),
                &json!("/x/trello-mcp")
            )
        );
        assert_eq!(v["groups"][3]["sessions"], 0);
    }
}
