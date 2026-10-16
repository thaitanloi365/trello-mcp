# Tray v2 and Trello MCP dashboard: design

Date: 2026-10-16. Status: draft for review.

## Goal

Two pieces of work.

1. **Tray v2.** Keep the Swiss panel, but make it cleaner and more compact. Each project (session folder) folds open and shut, and starts folded. Add a theme switch (Auto, Light, Dark; Auto follows macOS), and buttons that open the dashboard locally or over Tailscale.
2. **Dashboard.** A web page that does everything the tray does, plus:
   - search Trello cards;
   - list the cards where someone is waiting for your comment or reply, each with a button that copies the direct comment URL;
   - summarise the cards in "Ready for dev" and "In progress" lists, with each card's URL.

What the user asked for:
- "keep swiss but make it cleaner and compact"
- "collapse/expand for each project, default is collapsed"
- "theme toggle (light/dark/automatic) … default is automatic based on system"
- "button to open dashboard, open dashboard via tailscale"
- a dashboard with "same function as tray menu, plus should allow to search trello ticket"
- "list of ticket it is waiting me to comment or reply allow to copy direct comment url"
- "'ready for dev' and 'in-progress' tickets just summary with ticket url"
- use "the current trello-mcp itself" for Trello

Choices the user made:
- **Waiting on you** means @mentioned with no reply since.
- **Ready for dev and In progress** show every card in those lists, not only yours.
- **Boards in scope** are all your open boards in the allowed workspaces.
- **Server data:** approach A. The dashboard finds and stops servers itself, and the tray keeps its own finder.

Mockups (approved; local, gitignored):
- `.superpowers/brainstorm/42452-1792096558/content/tray-v2.html`
- `.superpowers/brainstorm/42452-1792096558/content/dashboard.html`

Out of scope:
- New MCP tools: the tool list stays at 28.
- Glass or HUD styling.
- Marking mentions read, or posting comments from the dashboard.
- Notifications.
- Quitting several servers at once.
- A dashboard on Linux or Windows.
- A separate dashboard app.

## References

- **Swiss design system:** `/Users/loithai/Projects/ai-tools/claude-usage/DESIGN.md`. It sets the colours, type, 4 px and 1 px rules, square buttons and chips, and the One Voice Rule.
- **Dashboard pattern to copy:** gws-mcp.
  - `/Users/loithai/Projects/ai-tools/gws-mcp/src/dashboard.rs`: axum server, two listeners, single instance by port, Host, login and Origin checks (lines 199-205, 211-267 and 338-365), and the page embedded with `include_bytes!` (line 305).
  - `/Users/loithai/Projects/ai-tools/gws-mcp/web/index.html`: one plain HTML page with no build step.
  - `/Users/loithai/Projects/ai-tools/gws-mcp/tray/main.swift`: the tray starts the dashboard with `posix_spawn`, restarts it after a crash, and stops it with SIGTERM then SIGKILL (lines 1170-1210). `tailnetURL` is at lines 286-291.
  - `/Users/loithai/Projects/ai-tools/gws-mcp/tray/Panel.swift`: the "Open Dashboard" and "Open Dashboard via Tailscale" footer buttons (lines 434 and 654-657).
- **Tailnet Makefile vars:** `/Users/loithai/Projects/ai-tools/claude-usage/Makefile` lines 17-23. `TAILSCALE` is kept across reinstalls; `TS_HOST` and `TS_LOGIN` are read from `tailscale status --json`.
- **Server rules:** `docs/superpowers/specs/2026-10-13-tray-design.md`, section "Finding and measuring servers", and `tray/Procs.swift`.

## Files

| File | Change |
| --- | --- |
| `src/main.rs` | New `Dashboard` subcommand: `trello-mcp dashboard [--addr] [--tailnet-addr] [--tailscale host:port=login] [--config]`. |
| `src/dashboard/mod.rs` | New. axum app: listeners, auth middleware, routes, the 60 s Trello cache, and the embedded page. |
| `src/dashboard/procs.rs` | New, `cfg(target_os = "macos")`. Finds, measures and stops servers with libc's `proc_*` and `sysctl` calls, using the tray's rules. |
| `src/dashboard/servers.rs` | New. The tray's pure server rules and text in Rust (`isServer`, `parseProcArgs`, `advance`, `groups`, `activeText`, `buildLabel`, `groupLabel`, `countdown`, `cpuText`, `memText`) and the `/api/servers` JSON. |
| `src/util.rs` | `ago(seconds)`: "now", "12m ago", "3h ago", "2d ago"; shared by server activity and card activity. |
| `src/mcp/feeds.rs` | New `Server` methods for the dashboard's Trello data: `waiting`, `named_lists`, `search_cards`. They are not MCP tools. |
| `src/mcp/mod.rs` | `mod feeds;` |
| `web/index.html` | New. The dashboard page: plain HTML, CSS and JS. |
| `Cargo.toml` | Adds `axum = "0.8"` and `libc = "0.2"` (already in the lock tree), and the tokio `net` and `sync` features. |
| `tray/Panel.swift` | Compact sizes, folding groups, theme switch, dashboard buttons, the new footer. |
| `tray/main.swift` | `--tailscale` argument, starting and supervising the dashboard, theme preference, ⌘D, selftest updates. |
| `tray/com.trellomcp.tray.plist` | Passes `--tailscale @TAILSCALE@`. |
| `Makefile` | `TAILSCALE`, `TS_PORT := 8448`, `TS_HOST`, `TS_LOGIN`; a `tailnet` target; `tray-install` fills `@TAILSCALE@`. |
| `README.md`, `CHANGELOG.md` | A dashboard section, and the tray changes. |

## Tray v2

### Layout

All sizes come from the approved mockup `tray-v2.html`.

- **Panel:** 460 pt wide, with 16 pt side margins and 12 pt top padding.
- **Header:** a 16 pt board mark, "Trello MCP" at 13 pt bold, and "live · every 2 s" at 11 pt muted, flush right. A 4 pt ink rule sits below it.
- **Figures:** one row of four (Servers, Sessions, Memory, CPU). Each has a 10 pt uppercase muted label over a 17 pt bold value.
- **Table:** columns PID 40, Build (flexible), Up 52, Active 52, CPU 40, Mem 46 and action 56, with 6 pt gaps. The header row is followed by a 1 pt ink rule.
- **Group row (28 pt, hairline below):**
  - a chevron (▶ folded, ▼ open), then the label in bold, cut in the middle;
  - after the label: the server count when folded, or "N · M sessions" when open;
  - then, in the table's columns: the latest Active of its servers, the sum of their CPU and the sum of their memory (bold);
  - Up and the action column stay empty.
- **Server row (26 pt, hairline below):** as today, at the smaller height. The button is 19 pt tall and reads Quit, Confirm or Quitting…, styled as today.
- **Footer:** a 1 pt ink rule, then two lines.
  - **First line (30 pt):** "Dashboard ⌘D" and "via Tailscale" as bold text buttons, and the AUTO / LIGHT / DARK switch flush right. The switch is three square chips; the chosen one is ink-filled.
  - **Second line (11 pt muted):** "Tray: CPU … · Memory …" on the left, and Open Log ⌘L and Quit ⌘Q on the right.
- **Empty state, error box and scrolling:** behaviour unchanged. Sizes scale down to match.

### Folding

- **Default:** every group starts folded. `PanelModel.open: Set<String>` holds the labels of open groups, in memory only, so a relaunch folds everything again.
- **Toggling:** a click anywhere on a group row toggles it and calls `changed()`; the shape changes, so the panel re-renders. A group row is a button for accessibility: its label is "label, folded" or "label, open", and its action toggles it.
- **Ticks:** a tick updates folded group rows in place, alongside the server rows. `GroupLine` gets `active`, `cpu` and `mem` values, and `groupFigures(g)` computes them from the rows.
- **Confirm:** a Confirm in a group that gets folded still times out after 3 s as today; folding does not stop a server.

### Theme

- **Stored value:** UserDefaults key `theme`, set to `auto`, `light` or `dark`. The default is `auto`.
- **Effect:** `window.appearance` is nil for auto, `.aqua` for light and `.darkAqua` for dark. `Root.viewDidChangeEffectiveAppearance` already re-renders.
- **Switch:** a click on a chip saves the choice and applies it at once.

### Dashboard buttons and supervision

- **Arguments:** the tray takes `--tailscale host:port=login` from the LaunchAgent, as gws-mcp does. `tailnetURL` turns it into `https://host:port`; with no value, "via Tailscale" is hidden.
- **Dashboard ⌘D** opens `http://127.0.0.1:8790`. **via Tailscale** opens the tailnet URL. Both close the panel and call `NSWorkspace.shared.open`.
- **Starting the dashboard:** at launch, when `--cli` is set, the tray starts `<cli> dashboard`, adding `--tailscale <value>` when it has one. It uses `posix_spawn` with inherited stdio, so the dashboard's output lands in the tray's log, copied from gws-mcp.
- **Crashes:** the tray restarts a crashed dashboard after 1 s, doubling the wait up to 60 s, and resets the wait after 5 minutes of uptime.
- **Quit:** the tray sends SIGTERM, then SIGKILL after 5 s, before exiting.
- **No `--cli`:** no dashboard is started, and both dashboard buttons are hidden.
- **Not a server:** the dashboard process is named `trello-mcp`, but its first argument is `dashboard`, so `isServer` already rejects it. A selftest checks this.

## Dashboard

### Command and listeners

`trello-mcp dashboard` runs a current-thread tokio runtime, as `serve` does.

- **Local listener:** `--addr`, default `127.0.0.1:8790`.
- **Tailnet listener:** `--tailnet-addr`, default `127.0.0.1:8791`. It runs only with `--tailscale host:port=login`.
- **Single instance:** if a port is already bound, the command exits 1 with "dashboard already running on 127.0.0.1:8790"; the bound port is the single-instance lock.
- **Trello client:** built as `serve` builds it: `config::Manager::new(config_path)`, then `trello::Client`, then `mcp::Server::new`. The config, credentials, allowed workspaces and request pacing are therefore the same.
- **Bad or missing config:** the dashboard still starts. The servers section works, and the Trello sections show the config error.

### Access control

Copied from gws-mcp `dashboard.rs`. Every rule applies to every route, including `/`.

- **Local listener:** accepts only a loopback Host (`127.0.0.1:8790` or `localhost:8790`), and rejects any request that has an `X-Forwarded-*` header.
- **Tailnet listener:** accepts only Host equal to the tailnet host, and a `Tailscale-User-Login` header equal to the owner login from `--tailscale`.
- **POST requests:** need the header `X-Trello-MCP: 1`, and an `Origin` equal to the listener's own origin.
- **Every response** carries `X-Frame-Options: DENY`, `Content-Security-Policy: frame-ancestors 'none'` and `Cache-Control: no-store`.
- **Tailscale setup:** `make tailnet` runs `tailscale serve --bg --https=8448 http://127.0.0.1:8791`. It must never point at 8790: that listener has no login check. The README says so.

### Routes

| Route | Returns |
| --- | --- |
| `GET /` | `web/index.html`, embedded with `include_bytes!`. |
| `GET /api/servers` | `{figures: {servers, sessions, memory, cpu}, groups: [{label, count, sessions, active, cpu, mem, rows: [{pid, start, build, up, active, cpu, mem, path, stopping}]}]}`. All values are display text from `servers.rs`; `pid` and `start` are numbers. Off macOS: `{unsupported: true}`. |
| `POST /api/servers/{pid}/{start}/stop` | 202 once SIGTERM is sent; 404 when no server matches both pid and start; 403 on EPERM, with the tray's error text. SIGKILL follows after 5 s if the process is still the same (pid, start). |
| `GET /api/trello` | `{boards, updatedAt, waiting: {rows: [...], error?}, ready: {boards: [{board, url, cards: [...]}], error?}, progress: {boards: [...], error?}}`, or `{error}` when the boards can't be read. `updatedAt` is seconds since 1970. Served from a 60 s cache (errors are not cached); `?fresh=1` refetches. A section's own `error` leaves the other sections rendering. |
| `GET /api/search?q=` | `{cards: [...]}`, or `{error}`. Not cached. |

A card in any list has this shape: `{id, number, name, url, board, list, members: ["MT", ...], activity}`. `activity` is display text such as "2h ago".

### Trello data (`src/mcp/feeds.rs`)

Every call goes through `self.client.call`, so it is paced and checked against the allowed workspaces.

**Boards in scope**
- `self.list_boards("")`: open boards in the allowed workspaces.
- The result is part of the 60 s snapshot.

**Board snapshot** (one call per board, part of the 60 s snapshot)
- Call: `GET /boards/{id}` with these query fields:
  - `fields=name,url`
  - `lists=open&list_fields=name`
  - `cards=open&card_fields=name,idShort,shortUrl,shortLink,idList,idMembers,dateLastActivity`
  - `members=all&member_fields=initials,fullName`
- Cost: about 14 boards at the client's 105 ms pacing, roughly 1.5 s per refresh.

**Ready for dev / In progress**
- Normalise a list name: lowercase it and keep only `a-z` and `0-9`.
  - A name that starts with `readyfordev` is Ready for dev. This matches "Ready for dev", "Ready for Development" and "READY FOR DEV 🚀".
  - A name that starts with `inprogress` is In progress. This matches "In progress", "In-Progress" and "In progress (BE)".
- Cards come from the board snapshot, grouped by board. Boards are in name order; cards keep the list's order.
- Each card has its number (`#idShort`), name, member initials, `shortUrl` and last activity.

**Waiting on you**
1. `GET /members/me` with `fields=id,username`, cached for the process lifetime.
2. `GET /members/me/notifications` with `filter=mentionedOnCard`, `limit=200`, `memberCreator=true` and `memberCreator_fields=fullName`. Keep the notifications from the last 30 days whose board is in scope.
3. `GET /members/me/actions` with `filter=commentCard`, `since=<30 days ago, ISO>`, `limit=1000` and `fields=date,data`. This maps each card id to your latest comment date.
4. A mention is waiting when you have no comment on that card dated after it.
5. Several waiting mentions on one card collapse into one row: the latest mention, plus "+N earlier mentions".
6. Each row carries:
   - the card's number, name, board and list;
   - the mention text, cut to 280 characters;
   - the mentioner's full name and the mention's age;
   - `commentUrl`: `https://trello.com/c/{shortLink}#comment-{idAction}`;
   - the card URL.
7. Rows are newest first.

**Search** (`search_cards(q)`)
- **Card URL or short link** (`trello.com/c/<shortLink>`, or a bare 8-character short link): resolved from the board snapshot of open cards, with no extra Trello call (archived cards are not found). Only boards in scope are searched, so the board is always allowed.
- **Card number** (`#123` or `123`): every card with that `idShort` in the board snapshot, which can be one per board.
- **Text:** `GET /search` with `query=q`, `modelTypes=cards`, `idBoards=<ids in scope>`, `cards_limit=20`, `partial=true`, the same card fields, `card_board=true`, `board_fields=name`, `card_list=true` and `list_fields=name`.
- An empty or whitespace query returns `{cards: []}` and makes no call. Queries longer than 200 characters are rejected with 400.

### Server finding (`src/dashboard/procs.rs`)

The rules are the tray's, from the "Finding and measuring servers" section of the 2026-10-13 spec. They are ported, not reinvented.

- **Finding:** `proc_listpids(PROC_UID_ONLY, uid)`, then `proc_name == "trello-mcp"`.
- **Server test:** the arguments come from `KERN_PROCARGS2`. After skipping `--config`, the first argument is `serve`, or there is none.
- **Session:** the parent's pid, its start time and its working folder.
- **Per server:** binary path, physical footprint, and CPU nanoseconds.
- **Activity:** CPU % comes from deltas between scans. A server counts as active when it used 2 ms of CPU or more since the last scan. A baseline older than 3 minutes gives no CPU figure.
- **Scan timing:** a scan a minute while the dashboard runs, plus one on each `GET /api/servers` that comes at least 1 s after the previous scan. Scans run one at a time on `spawn_blocking`.
- **Stop:** checks that the (pid, start) pair still matches, sends SIGTERM, and sends SIGKILL after 5 s if it is still the same process. A server being stopped shows Quitting… until a scan finds it gone.

### Page (`web/index.html`)

The page follows the approved mockup `dashboard.html`, and claude-usage's DESIGN.md tokens (copied as in gws-mcp's page).

- **Masthead:** board mark, "Trello MCP" at 40 px, then flush right: "N boards · updated HH:MM:SS", Refresh, and the AUTO / LIGHT / DARK chips. A 4 px rule closes it.
- **Search:**
  - A full-width 48 px field with a 2 px ink border, and a Search button.
  - Typing waits 300 ms, then searches. Enter searches at once.
  - Each result row has the number, name, "board · list · members · active …", Copy link and Open.
- **Waiting on you:**
  - The section title carries the count, in red ink when above 0.
  - Each row has the card, a quoted mention with who and when, **Copy comment link** (the page's one red button) and Open.
  - Empty: "Nothing waiting on you."
- **Ready for dev and In progress:**
  - Two columns of 860 px or more, one column below that.
  - Board sub-heads with counts. Each card row has the number, a linked name, "members · active …" and Copy.
  - Empty: "No cards in Ready for dev lists."
- **Servers:**
  - Four figures, then the table with folding groups. Folded is the default; the open groups are remembered in `localStorage` under `open-groups`.
  - Quit, then Confirm (red, armed for 3 s), then Quitting….
  - Under 860 px, the Up and CPU columns are hidden.
- **Theme:** `localStorage` key `theme` holds `auto`, `light` or `dark`. It sets `data-theme` on `<html>`; auto uses `prefers-color-scheme`. Every storage access sits in try/catch, and the page falls back to auto.
- **Refresh rates:**
  - `/api/servers` every 2 s while the tab is visible; it stops while hidden.
  - `/api/trello` on load, on Refresh (`?fresh=1`), and every 5 minutes while visible.
- **Copying:** `navigator.clipboard.writeText`. Both origins are secure contexts. The button reads "Copied" for 1.2 s.
- **Errors:** a 2 px red-ink box per section, with the message.
- **Accessibility:** group rows are `<button aria-expanded>`, and copy buttons have labels. The focus ring is 2 px red ink.

## Makefile and install

- **Variables:**
  - `TS_PORT ?= 8448`
  - `TS_HOST` and `TS_LOGIN` from `tailscale status --json`, as claude-usage does.
  - `TAILSCALE ?=` the value the installed LaunchAgent already passes, so a reinstall keeps it.
- **`make tailnet`:** runs `tailscale serve --bg --https=$(TS_PORT) http://127.0.0.1:8791`, then prints the `TAILSCALE` value to install with.
- **`make tray-install`:** fills `@TAILSCALE@`. If it is empty, the tray gets `--tailscale ""` and hides the Tailscale button.

## Testing

**Rust unit tests (wiremock, as in `src/mcp/*.rs`):**
- **Waiting:**
  - A mention followed by your later comment on the same card drops out.
  - A mention after your last comment stays.
  - Two mentions on one card become one row with "+1 earlier".
  - A mention older than 30 days drops out.
  - A board outside the allowed workspaces drops out.
  - The comment URL format is right.
- **List names:** the matcher table, with names that must match and must not ("Ready", "Not in progress", "Progress review").
- **Search routing:** a URL, a short link, `#348`, text, an empty query, and a 201-character query.
- **Text helpers:** the same inputs and outputs as the tray's selftest (activity, build label, group label, countdown, CPU, memory), so both languages print the same strings.
- **Access control:** against a real listener on `127.0.0.1:0` with reqwest, as gws-mcp's tests do:
  - local Host accepted, foreign Host rejected, `X-Forwarded-For` rejected;
  - tailnet with the wrong or missing login rejected;
  - POST without the header, or with a foreign Origin, rejected.
- **macOS scan and stop:** a copy of a sleeper binary named `trello-mcp`, started with `serve`, is found. A copy started with `dashboard` is not. Stop ends it with SIGTERM, and a SIGTERM-ignoring one gets SIGKILL after 5 s.

**Tray selftest (`make tray-test`):**
- The panel is 460 wide in every state.
- Groups start folded.
- Toggling opens a group, and a tick keeps it open and updates its totals.
- The theme saves and applies.
- Dashboard ⌘D and via Tailscale open the right URLs, and via Tailscale is hidden without `--tailscale`.
- The dashboard spawn arguments are right.
- A `trello-mcp dashboard` process is not listed as a server.

**Manual:** `make tray-install && make tailnet`. Then open the dashboard locally and on the phone over Tailscale, copy a comment link, and Quit a server from both the tray and the dashboard.

## Risks

- **Rules in two languages:** the Swift and Rust copies of the server rules can drift. The shared text-helper tests and this spec are the guard. If they drift, move the tray to read `/api/servers` (approach B).
- **Mention data:** `mentionedOnCard` notifications are capped by Trello's notification history, and notifications you have deleted are gone. This is not yet checked against the live account; the first build checks it.
- **Binary size:** axum adds about 1 MB to the binary that every MCP session runs. Code pages are shared and loaded on demand, so memory per process should not change. Measure RSS before and after.
