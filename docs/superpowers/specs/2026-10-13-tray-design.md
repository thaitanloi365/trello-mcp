# Trello MCP tray: design

Date: 2026-10-13. Status: draft for review.

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
  (`Pal`, `hvFont`, `label`, `vstack`/`hstack`, `block`, `Press`, `btn`, `txtBtn`, `spinner`,
  `PanelWindow`, `usage(of:)`, `cpuPercent`). Copy what the panel uses; leave out the rest.
- Packaging: claude-usage's and gws-mcp's Makefiles (team signing, `make icon`, LaunchAgent).

## Files

| File | Holds |
| --- | --- |
| `tray/Procs.swift` | Finding servers and measuring them with libproc; pure helpers (argument rule, labels, formatting). No AppKit. |
| `tray/Panel.swift` | Panel model, palette, building blocks and the panel view. |
| `tray/main.swift` | App delegate, status item, panel window, sampling timer, stop action, SELFTEST block. |
| `tray/Info.plist` | Bundle `com.trellomcp.tray`, name "Trello MCP", executable `trello-mcp-tray`, `LSUIElement`. |
| `tray/com.trellomcp.tray.plist` | LaunchAgent, rendered into `~/Library/LaunchAgents` with `@HOME@` filled in. |
| `tray/AppIcon.svg`, `tray/AppIcon.icns` | App icon: the board mark (three ink columns of different heights) on a dark rounded square. |

## Finding and measuring servers

Every sample runs these steps on the main queue. A sample takes a few milliseconds (about 2,000 pids
on this Mac), and staying on the main queue avoids threading the panel's state. What never changes
for a process (its arguments, binary and folder) is read once per (pid, start time) and cached.
A tick reads only names, parents and usage.

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
   - `proc_pidinfo(PROC_PIDVNODEPATHINFO)`: working folder.
   - `proc_pidpath`: binary path.
   - `proc_pid_rusage(RUSAGE_INFO_V2)`: CPU time (Mach ticks converted to ns) and
     `ri_phys_footprint`, the memory figure Activity Monitor shows.
   - `proc_name(ppid)`: the client name, such as `claude` or `codex`.

A server is identified by (pid, start time), so a reused PID never inherits another server's
history or a Stop click.

CPU % is the CPU time used between two samples divided by the wall time between them, ×100 (100% is
one full core). It is the same formula as claude-usage's `cpuPercent`. A server seen for the
first time shows `—` until its second sample.

Any failing call for one pid (it just exited) drops that pid from the sample; it is not an error.

## Labels and rules

- **Project label:** the working folder's last component. Put the parent folder in front
  (`tonotopia/app`) when the name is shorter than 4 characters, or when another server's
  different folder has the same name. The home folder is `~`. An unknown folder shows
  `trello-mcp`.
- **Order:** by project label, then by start time. The order doesn't move as numbers tick.
- **Status of a row**, first match wins:
  - `STOPPING…`: spinner, red ink.
  - `ORPHANED`: amber square. The parent is launchd (ppid 1), so the client that started it is gone.
  - `BUSY`: ink square. CPU above 0.05% in the last interval.
  - `IDLE`: muted square.
  - The status word is followed by `· PID n · UP t`.
- **Uptime:** `4 MIN`, `2H 14M`, `3 DAYS` (`1 DAY`).
- **Threshold Rule for a row's CPU figure:** ink below 80%, amber from 80%, red from 95%. The
  totals block is always ink (several servers can add up past 100%). Memory has no thresholds.
- **Memory text:** `8.2 MB` under 10 MB, `49 MB` from 10 MB, `1.2 GB` from 1024 MB.
- **Totals note:** `5 idle · 1 busy · 1 orphaned · oldest up 3 days`. Counts of zero are left out.

## Panel (layout A)

The panel is a borderless non-activating `NSPanel` under the status item, 360 pt wide, as in
claude-usage. Esc, a click elsewhere or another app taking the keyboard closes it. It follows the
system's light and dark appearance.

From top to bottom:

1. **Header:** the board mark (22 pt), "Trello MCP" (15 bold, -0.02 em), then flush right
   `live · every 2 s` (12, muted). A 4 pt ink rule closes it.
2. **Totals block,** inverted (ink ground, paper text), 16 pt padding. Three figures, each with an
   11 pt uppercase muted label above a 28 pt bold tabular figure: SERVERS (64 pt column), CPU (sum)
   and MEMORY (sum), which share the rest equally. The figures are 28 pt, not claude-usage's 34, so
   `100.0%` and `1.2 GB` fit in 100 pt columns. Below them, a hairline and the totals note (12,
   muted).
3. **Servers table:** a header row (`SERVERS`, then `CPU` and `MEMORY` right-aligned in 56 pt
   columns), then a 1 pt ink rule. Each server row has two lines:
   - chevron, project label (13 bold, truncated in the middle), CPU and memory (15 bold, right-aligned);
   - the status line (11 bold, 0.04 em, indented 16 pt).

   A 1 pt ink rule separates rows. Clicking a row expands it (only one at a time) to show
   label/value lines (12 pt, label muted, value right-aligned):
   - Folder: full path, with `~` for home.
   - Binary: the `proc_pidpath` path.
   - Config: the `--config` value, else `default`.
   - Started by: `claude · PID 3614`, or `launchd · orphaned`.
   - Started: `Today 9:12 AM`, `Mon 9:12 AM` within a week, else `Oct 6 9:12 AM`.

   Under those lines sits a **Stop server** button and a muted note: "Its session loses Trello
   tools until it reconnects (/mcp in Claude Code)." The button uses the red style when the
   server is orphaned or at 95% CPU or more, the outline style otherwise.
4. **Error box,** only after a failed stop, above the totals: 2 pt red-ink border, bold red text.
   It clears when the panel closes.
5. **Empty state,** in place of the totals and table: a large board mark in track grey, then "No
   servers running" (17 bold) and "Claude Code and Codex start one trello-mcp per session. Open a
   project that uses it and it shows here." (13, muted).
6. **Footer:** a 1 pt ink rule. Below it, Open Log (bold, ⌘L) on the left and Quit (muted, ⌘Q) on
   the right. Under them, the tray's own usage: `Tray: CPU 0.1% · Memory 21 MB` (12, muted).

The middle (totals and table) scrolls once it is taller than the screen allows, as in claude-usage.

**Menu bar item:** the board mark as a template image, about 16 pt. Tooltip and VoiceOver label
"Trello MCP". Clicking it toggles the panel. There is no menu.

**Accessibility:**
- Rows are buttons labeled with the project and status, with value expanded or collapsed.
- Figures carry labels such as "CPU 0.6%".
- A failed stop is announced, as claude-usage does with its errors.
- Reduce Motion stops the spinner.

## Stop action

1. Clicking Stop server sets the row to `STOPPING…`.
2. Right before signalling, the tray reads the pid's name and start time again. If either no
   longer matches the sampled server, it does not signal, and the row goes away on the next sample.
3. It sends `SIGTERM`. trello-mcp exits on SIGTERM through its `shutdown_signal`.
4. After 3 s, if the same (pid, start time) is still alive, it sends `SIGKILL`.
5. `ESRCH` means the server is already gone and is not an error. `EPERM` or another error shows the
   error box: "Could not stop PID 4648: Operation not permitted", with the row's status restored.
6. Stops in progress are tracked by (pid, start time). Two servers can stop at the same time.

There is no confirmation dialog. Expanding the row first is the deliberate step. A dialog would
also take focus from the non-activating panel.

## Keeping the tray light

- **Closed panel:** no timers and no sampling. The tray sits idle at about 0.0% CPU.
- **Open panel:**
  - The first sample runs on open, the second 0.5 s later so CPU % shows almost at once, then one
    every 2 s.
  - Each sample makes about one `proc_name` call per process of this user, plus a few calls per
    server. The 1 MB `KERN_PROCARGS2` read happens once per new process, not every tick.
  - The timer has a 0.2 s tolerance, so macOS can batch its wakeups.
  - It never starts `ps` or any other child process.
- **Rendering:**
  - The panel is rebuilt only when the set of servers changes, a row opens or closes, a stop starts
    or ends, an error shows, or the appearance changes.
  - Each tick only updates the text of the existing figure labels, status lines and the footer's
    stats line. This keeps hover and scroll position, and avoids layout work.
  - The status item image is drawn once at launch.
- **Frameworks:** plain AppKit. No SwiftUI, no `NSPopover` (claude-usage measured 25 MB that way
  against 34 MB), no `UserNotifications`.
- **Targets, measured with `footprint` and Activity Monitor:**
  - closed: CPU 0.0% and memory at or below claude-usage's tray;
  - open with 10 servers: under 0.5% CPU on average.

## Lighter server

`serve()` in `src/main.rs` builds `tokio::runtime::Runtime::new()`, a multi-thread runtime with one
worker per core. The proposed change is
`tokio::runtime::Builder::new_current_thread().enable_all().build()`.

- MCP over stdio has a single client, and nothing in `src/` calls `block_in_place`, which panics
  on a current-thread runtime.
- The blocking pool remains for the stdin read.
- The baseline, measured today on an idle `bin/trello-mcp serve` on a 16-core Mac:
  - footprint 8.0 MB;
  - RSS 3.9 MB;
  - 9 threads.

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
- Project labels: a plain folder, a short name, a name clash, home, unknown.
- Order, status (including orphaned and stopping), uptime text, memory text and totals note.
- CPU % from two samples, and `—` for one.
- The Threshold Rule edges at 80 and 95.
- A live call against the selftest's own pid: `proc_name`, rusage and start time.
- A spawned `sleep` child stopped through the stop path, including the identity re-check.

**Live checks:**
- `make tray-install`.
- Start two servers, one with `--config`, and confirm the panel lists both with the folders and
  numbers that `footprint` and `ps` show.
- Stop one and watch it go.
- Check light and dark mode.
- Measure the tray closed and open.
- Visual pass through the `e2e-tester` agent with aura-pilot, with screenshots of each state.
