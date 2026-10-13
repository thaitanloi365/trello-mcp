# Trello MCP tray: design

Date: 2026-10-13. Status: draft for review. The panel layout, labels and stop action were ported from gws-mcp's tray on the `feat/tray-gws-layout` branch; those sections describe the result.

## Goal

A macOS menu bar app, Trello MCP, that shows every running trello-mcp server, with its CPU and
memory, and can stop one. About ten Claude Code and Codex sessions each start their own
`bin/trello-mcp serve`. The tray makes busy, stuck and leftover servers easy to spot and to stop.

The tray follows the claude-usage tray: the same Swiss design and the same AppKit panel technique.
It must cost almost nothing while closed and very little while open. The server also gets cheaper
per process.

What the user asked for: a tray like gws-mcp's that shows the servers' own CPU and memory; the UI
of claude-usage; CPU and memory kept low; layout A from the mockup
(`.superpowers/brainstorm/70415-1791856783/content/panel-layout.html`); an action to stop a server.

Out of scope: Trello credential checks, notifications, numbers in the menu bar, stopping several
servers at once, Linux and Windows.

## References

- Design system: `/Users/loithai/Projects/ai-tools/claude-usage/DESIGN.md` (colors, type, rules,
  buttons, status squares, Threshold Rule). The tray uses it as is; this repo gets no DESIGN.md.
- Panel technique and building blocks:
  `/Users/loithai/Projects/ai-tools/claude-usage/tray/Panel.swift` and `tray/main.swift`
  (`Pal`, `hvFont`, `label`, `vstack`/`hstack`, `block`, `Press`, `txtBtn`, `PanelWindow`,
  `usage(of:)`). Copy what the panel uses; leave out the rest.
- Packaging: claude-usage's and gws-mcp's Makefiles (team signing, `make icon`, LaunchAgent).

## Files

| File | Holds |
| --- | --- |
| `tray/Procs.swift` | Finding servers and measuring them with libproc; pure helpers (argument rule, labels, formatting). No AppKit. |
| `tray/Panel.swift` | Panel model, palette, building blocks and the panel view. |
| `tray/main.swift` | App delegate, status item, panel window, scan queue and timer, stop action, SELFTEST block. |
| `tray/Info.plist` | Bundle `com.trellomcp.tray`, name "Trello MCP", executable `trello-mcp-tray`, `LSUIElement`. |
| `tray/com.trellomcp.tray.plist` | LaunchAgent, rendered into `~/Library/LaunchAgents` with `@HOME@` and `@CLI@` (this checkout's `bin/trello-mcp`, passed as `--cli`) filled in. |
| `tray/AppIcon.svg`, `tray/AppIcon.icns` | App icon: the board mark (three ink columns of different heights) on a dark rounded square. |

## Finding and measuring servers

Every scan runs these steps on one serial background queue, one scan at a time (a tick that finds
one running is skipped); the result is applied on the main queue. The scanner is not thread-safe, so
only that queue touches it. A scan takes a few milliseconds (about 2,000 pids on this Mac). What
never changes for a process (its binary, and its parent's start time and folder) is read once
per (pid, start time) and cached. A tick reads only names, parents and usage.

1. `proc_listpids(PROC_UID_ONLY, getuid())`: only this user's processes, which are also the only
   ones the tray may stop.
2. `proc_name(pid)`: keep only pids named `trello-mcp`. The tray's own process is
   `trello-mcp-tray`, so it never matches.
3. For each match, read its arguments with `sysctl([CTL_KERN, KERN_PROCARGS2, pid])`. Arguments
   are NUL-separated, so paths with spaces parse correctly. It is a server when, after skipping
   `--config <path>` and `--config=<path>`, the first remaining argument is `serve` or there is
   none (plain `trello-mcp` also serves). `config show`, `version`, `-v`, `update`, `setup` and
   the rest are not servers.
4. For each server:
   - `proc_pidinfo(PROC_PIDTBSDINFO)`: parent pid and start time (seconds and microseconds).
   - The session is the parent: its start time (`PROC_PIDTBSDINFO` of the ppid) and its working
     folder (`PROC_PIDVNODEPATHINFO` of the ppid). A server with ppid 1, or whose parent is gone,
     has no session. A server whose parent exits later loses its session on the next scan.
   - `proc_pidpath`: binary path.
   - `proc_pid_rusage(RUSAGE_INFO_V2)`: CPU time (Mach ticks converted to ns) and
     `ri_phys_footprint`, the memory figure Activity Monitor shows.

A server is identified by (pid, start time), so a reused PID never inherits another server's
history or a Stop click.

CPU % is the CPU time used between two samples divided by the wall time between them, ×100 (100% is
one full core). It is the same formula as claude-usage's `cpuPercent`. A server seen for the
first time shows `—` until its second sample.

Any failing call for one pid (it just exited) drops that pid from the sample; it is not an error.

## Labels and rules

Ported from gws-mcp's tray (`Servers.swift`).

- **Group label:** the last two folders of the session's working folder, `~` for home, `/` for the
  root. A server with no session goes under `No session`, last.
- **Order:** groups alphabetically by label, `No session` last; rows by session start (oldest
  first), then pid. The order doesn't move as numbers tick.
- **Group meta:** `N sessions · memory`, `1 session · 10 MB`, or only the memory under `No session`.
  A session is a distinct (parent pid, parent start time).
- **BUILD:** `installed` when the server's binary is the tray's CLI (`--cli <path>`, passed by the
  LaunchAgent and resolved through symlinks, as `proc_pidpath` reports binaries; without it
  nothing matches); the worktree's name for a binary under
  `/.claude/worktrees/`; else the last two folders of the binary's path. The row's tooltip is the
  full binary path.
- **UP:** the session's age, from the parent's start time: `3d 9h`, `2h 0m`, `45m`; `—` without a session.
- **ACTIVE:** the last scan in which the server used at least 2 ms of CPU: `now` under a minute,
  `12m ago`, `3h ago`, `2d ago`; `—` until seen. The panel keeps it across closes and reopens by
  (pid, start time), but never counts the CPU of a closed period (the first scan after opening has
  no baseline). If the server used 2 ms or more while the panel was closed, ACTIVE shows `—`: it
  worked, but when is unknown.
- **CPU:** percent of one core between two scans, `—` before the second. `0.6%`, `143%` from 100.
  No threshold colors.
- **Memory text:** `8.2 MB` under 10 MB, `49 MB` from 10 MB, `1.2 GB` from 1024 MB.
- **Figures:** SERVERS, SESSIONS (distinct sessions), MEMORY and CPU (sums; CPU `—` until a server
  has a second scan).

## Panel (layout A)

The panel is a borderless non-activating `NSPanel` under the status item, 520 pt wide (the same
layout as gws-mcp's tray; 32 pt of that is margin). Esc, a click elsewhere or another app taking the
keyboard closes it. It follows the system's light and dark appearance.

From top to bottom:

1. **Header:** the board mark (22 pt), "Trello MCP" (15 bold, -0.02 em), then flush right
   `live · every 2 s` (12, muted). A 4 pt ink rule closes it.
2. **Error box,** only after a failed Quit, above the section: 2 pt red-ink border, bold red text.
   It clears when the panel closes.
3. **Section title** `TRELLO-MCP SERVERS` (11 medium caps, muted) with a 1 pt ink rule.
4. **Figures row:** SERVERS, SESSIONS, MEMORY, CPU, each an 11 pt caps muted label above a 24 pt
   bold figure (-0.02 em), left-aligned, 20 pt apart.
5. **Table:** a header row (`PID`, `BUILD`, `UP`, `ACTIVE`, `CPU`, `MEM`, 11 pt caps, muted) and a
   1 pt ink rule, then the groups. Columns from the left: PID 40 pt (bold), BUILD (the rest), UP 46,
   ACTIVE 56, CPU 40, MEM 46 (bold), and a 62 pt button; 6 pt gaps.
   - A group heading (35 pt): the label (13 bold, cut in the middle) and, flush right, its meta
     (12, muted); a hairline below.
   - A server row (31 pt) is drawn in one view (`ServerLine`), with a hairline below. The button is
     an outline `Quit`, a red `Confirm`, or a dimmed `Quitting…`; it fills ink while the pointer is
     on it. Rows and headings are stacked by frame (`Rows`), not by Auto Layout.
6. **Empty state,** in place of the section: a large board mark in track grey, then "No servers
   running" (17 bold) and "Claude Code and Codex start one trello-mcp per session. Open a project
   that uses it and it shows here." (13, muted).
7. **Footer:** a 1 pt ink rule. Below it, Open Log (bold, ⌘L) on the left and Quit (muted, ⌘Q) on
   the right. Under them, the tray's own usage: `Tray: CPU 0.1% · Memory 21 MB` (12, muted).

The middle (section and error box) scrolls once it is taller than the screen allows. The scroll
view runs into the panel's 16 pt right margin, and its small overlay scroller sits there, so the
scroller never covers the button column and a legacy scroller never narrows the rows.

**Menu bar item:** the board mark as a template image, about 16 pt. Tooltip and VoiceOver label
"Trello MCP". Clicking it toggles the panel. There is no menu.

**Accessibility:**
- A row reads "trello-mcp server 3959, build installed, up 3d 9h, active now, CPU 0.0%, memory 40 MB".
  Its button is a child: "Quit trello-mcp server 3959", or "Confirm quit of trello-mcp server 3959";
  disabled while quitting.
- Quit turning into Confirm is announced to VoiceOver.
- A group heading reads "label, meta".
- A failed Quit is announced, as claude-usage does with its errors.

## Stop action

1. The first click on a row's `Quit` arms its `Confirm` (red) for 3 s. A click on another row's
   Quit moves the Confirm there. After 3 s without a second click the button goes back to `Quit`
   and nothing is stopped. The panel closing disarms it too.
2. A click on `Confirm` sets the row to `Quitting…`.
3. Right before each signal, the tray reads the pid's name and start time again. If either no
   longer matches the scanned server (`trello-mcp`, the start time), it does not signal, and the
   row goes away on the next scan.
4. It sends `SIGTERM`. trello-mcp exits on SIGTERM through its `shutdown_signal`.
5. After 5 s, if the same (pid, start time) is still alive, it sends `SIGKILL`.
6. `ESRCH` means the server is already gone and is not an error. `EPERM` or another error shows the
   error box: "Could not stop PID 4648: Operation not permitted", with the row's button back to `Quit`.
7. Quits in progress are tracked by (pid, start time). Two servers can quit at the same time.

The two-step Quit/Confirm replaces the earlier expand-then-Stop flow, which is gone with the
expandable details. There is no dialog: it would take focus from the non-activating panel.

## Keeping the tray light

- **Closed panel:** no timers and no sampling. The tray sits idle at about 0.0% CPU.
- **Open panel:**
  - The first scan runs on open (the panel appears with it in), the second 0.5 s later so CPU %
    shows almost at once, then one every 2 s.
  - Each scan makes about one `proc_name` call per process of this user, plus a few calls per
    server. The 1 MB `KERN_PROCARGS2` read happens once per new process, not every tick.
  - The timer has a 0.2 s tolerance, so macOS can batch its wakeups.
  - It never starts `ps` or any other child process.
- **Rendering:**
  - The panel is rebuilt only when the servers or their groups change (`sameShape`, keyed by pid,
    start time and folder), an error shows, or the appearance changes. A re-render keeps the scroll offset.
  - Each tick (same servers, same groups) only updates the row values and buttons, the group metas,
    the figures and the footer's stats line, in place. This keeps hover and scroll position, and
    avoids layout work. Quit, Confirm and Quitting… are also changed in place.
  - The status item image is drawn once at launch.
- **Frameworks:** plain AppKit. No SwiftUI, no `NSPopover` (claude-usage measured 25 MB that way
  against 34 MB), no `UserNotifications`.
- **Targets, measured with `footprint` and Activity Monitor:**
  - closed: CPU 0.0% and memory at or below claude-usage's tray;
  - open with 10 servers: under 0.5% CPU on average.
- **Measured (installed tray, 2026-10-13):**
  - closed: 26 MB `phys_footprint`, CPU 0.0% (`ps`, and `top` over 4 samples 2 s apart); claude-usage's
    tray measured 32 MB and 0.0% on the same Mac;
  - open with 11 servers and one row expanded (before the gws-mcp layout): 0.14% CPU over 20 s (1.85-2.42 ms per sample and tick), 26-27 MB,
    measured with a driver that runs the tray's own sample and tick code.

## Lighter server

`serve()` in `src/main.rs` builds `tokio::runtime::Runtime::new()`, a multi-thread runtime with one
worker per core. The proposed change is
`tokio::runtime::Builder::new_current_thread().enable_all().build()`.

- MCP over stdio has a single client, and nothing in `src/` calls `block_in_place`, which panics
  on a current-thread runtime.
- The blocking pool remains for the stdin read.
- Measured on the Rust build (commit c17a6c9), idle `serve`:
  - footprint 3330 -> 2936 KB;
  - threads 20 -> 4;
  - binary 8578816 -> 8330672 bytes.
  The change was kept.

Measure footprint and threads the same way after the change. Keep it only if the footprint drops.
Otherwise revert and record the numbers. `cargo test` and `make lint` must still pass, and a live
check must confirm a `tools/list` call and a real Trello read.

This is the only server change. The `make install` target and the binary path that clients use stay the same.

## Build, install, test

New Makefile targets (`install` stays as it is):

| Target | Does |
| --- | --- |
| `tray` | `swiftc -O tray/*.swift` into `dist/Trello MCP.app/Contents/MacOS/trello-mcp-tray`, copy `Info.plist` and `AppIcon.icns`, sign with `SIGN_ID` (same team lookup as gws-mcp, `-` without one). |
| `tray-test` | `swiftc -D SELFTEST` and run it. |
| `icon` | Render `AppIcon.icns` from `AppIcon.svg` with `rsvg-convert` and `iconutil`. |
| `tray-install` | Build the app, copy it to `~/Applications/Trello MCP.app`, `lsregister`, render and `plutil -lint` the LaunchAgent, then bootout and bootstrap `com.trellomcp.tray`. |
| `tray-uninstall` | Bootout the agent; remove the LaunchAgent plist and the app. |
| `check` | `lint test`, plus `tray-test` when `uname` is Darwin. |

LaunchAgent settings:
- RunAtLoad.
- `ProcessType` `Interactive`, so launchd does not throttle status item clicks.
- KeepAlive only on a crash (`SuccessfulExit` false).
- `AssociatedBundleIdentifiers` `com.trellomcp.tray`.
- stdout and stderr go to `~/Library/Logs/trello-mcp-tray.log`, which Open Log opens.

`.gitignore` gains `.superpowers/` and `*.plist.tmp`. The README gets a "Menu bar app" section, and
CHANGELOG.md gets an entry.

**SELFTEST** (`make tray-test`, preconditions as in claude-usage):
- The server rule:
  - `serve`, no arguments;
  - `--config X serve`, `serve --config X` and `--config=X`;
  - not `config show`, `version`, `-v` or `update`;
  - a path with a space.
- `KERN_PROCARGS2` buffer parsing, from a built buffer.
- `--cli` parsing.
- Group labels (two folders, home, root, none), group order with `No session` last, group meta, figures.
- `advance` and `activeText` across scans: CPU %, `lastActive` set only from 2 ms, kept across a
  reopen, fresh for a reused pid; `sameShape`; `upText`; `buildLabel` (installed, worktree, two folders).
- The Confirm timer: the first click arms, 3 s disarm, a second click calls `onStop`, a click on
  another row re-arms that row.
- A live call against the selftest's own pid: `proc_name`, rusage and start time; a fake server
  whose session folder and start time are its parent's.
- A fake server and a SIGTERM-ignoring process stopped through the stop path, including the identity
  re-check and the SIGKILL after the grace (the tests pass a short grace; the default is 5 s).
- Panel renders at 520 pt: light, dark, empty, error, Confirm and Quitting… rows, 40 rows inside a
  cap; a tick keeps the row views and the scroll offset.

**Live checks:**
- `make tray-install`.
- Start two servers and confirm the panel lists both with the folders and
  numbers that `footprint` and `ps` show.
- Stop one and watch it go.
- Check light and dark mode.
- Measure the tray closed and open.
- Visual pass through the `e2e-tester` agent with aura-pilot, with screenshots of each state.
