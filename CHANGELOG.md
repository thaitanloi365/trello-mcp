# Changelog

All notable changes to this project are documented in this file.

## [Unreleased]

- `setup` now installs into the home directory by default (`--scope user`); `--scope project` is the
  opt-in for project-local files. Earlier releases set up the project folder by default: pass
  `--scope project` to keep that, and remove stale project entries if clients load trello twice.
  `--project-dir` requires `--scope project`. User scope honors `$CODEX_HOME`, `$CLAUDE_CONFIG_DIR`
  and `$XDG_CONFIG_HOME`.
- JSON client configs, in both scopes, are patched in place: only the `trello` entry changes (your
  own fields on it, such as `enabled`, stay) and comments, key order, line endings, BOM and file
  mode are kept. `$schema` is no longer added to an existing `opencode.json`. Codex TOML is
  checked before and after the edit and an unmanaged `mcp_servers.trello` is refused.
- Trello MCP menu bar app (macOS): lists running servers grouped by session folder with PID, build,
  uptime, last activity, CPU and memory, and quits one (Quit, then Confirm). Same layout as the
  gws-mcp tray. `make tray-install`.
- `serve` runs on a current-thread runtime: an idle server uses less memory and fewer threads.

## [2.0.0] - 2026-10-12

Rewritten in Rust. Tools, the `draft_reply` prompt, and the config file are
unchanged. CLI commands and flags are otherwise unchanged.

Upgrading from 1.x: the Go binary's `update` cannot build the Rust source.
Run once from the checkout:
`cargo build --release --locked && mkdir -p bin && install -m 0755 target/release/trello-mcp bin/trello-mcp`.

### Rust rewrite

- Built with Rust (rmcp, tokio, clap, reqwest). Install with
  `cargo install --git https://github.com/thaitanloi365/trello-mcp`;
  `go install` no longer works.
- `trello-mcp update` rebuilds with `cargo build --release`.
- The MCP server name is `trello-mcp` (was `trello-mcp-go`), and the HTTP
  User-Agent is `trello-mcp/2.0.0`.
- Release archives cover macOS arm64 and amd64. Linux and Windows build
  from source.
- Removed the `completion` command that Cobra generated.
- `scripts/trello-mcp-dev` now runs `cargo run --release` (needs a Rust
  toolchain; first launch compiles).
- Network errors no longer include the request URL, which carried the API
  key and token.

Breaking: the 57 tools from the reference implementation become 28. Rename
calls in prompts and skills that use the old names.

### Changed

- Merged tools: `list_cards` (was `get_cards_by_list_id`, `get_my_cards`),
  `get_activity` (`get_recent_activity`, `get_card_history`), `get_board`
  (`get_lists`, `get_board_labels`, `get_board_members`,
  `get_board_custom_fields`, `get_active_board_info`), `create_cards`
  (`add_card_to_list`, `add_cards_to_list`), `update_card`
  (`update_card_details`, `archive_card`, `move_card`, `watch_card`,
  `assign_member_to_card`, `remove_member_from_card`), `update_list`
  (`archive_list`, `update_list_position`, `watch_list`), `list_boards`
  (`list_boards_in_workspace`), `set_active` (`set_active_board`,
  `set_active_workspace`), `find_checklist_items` (`get_checklist_items`,
  `get_checklist_by_name`, `get_acceptance_criteria`,
  `find_checklist_items_by_description`), `add_attachment` (the four
  `attach_*` tools), and `get_health` (the four other health tools).
- Renamed `update_card_custom_field` to `set_custom_field`, which takes a
  field name and a plain value, and clears only with `clear: true`.
- Removed `get_card_comments` (use `get_card` with `commentsLimit`),
  `copy_checklist`, and `perform_system_repair`.
- Lists, labels, members, and custom fields accept names; results show names
  instead of raw IDs.
- `get_card` returns Markdown only and adds board, list, reminder, custom
  fields, and the IDs write tools need. `detailLevel`, `format`, and
  `includeMarkdown` are gone.
- Write tools return a short acknowledgement instead of the full Trello
  object; activity and card lists are 75 to 93% smaller.
- `get_activity` shows each changed field's old and new value; long text is
  cut to 120 characters.
- `update_card` sends one request, so a failed update changes nothing and a
  repeated one is harmless. Removing a card's last label or member uses
  Trello's DELETE for that item.
- Tool JSON no longer escapes `&`, `<`, and `>`.

### Added

- `update_card` adds and removes labels, which no tool could do before.
- `trello-mcp update` pulls the latest source (`git pull --ff-only`) and
  rebuilds `bin/trello-mcp` in place; `--no-pull` skips the pull.
- `draft_reply` MCP prompt that drafts a card reply and posts it only after
  approval.
- Comments rewrite bare Trello attachment URLs to `[file name](url)` links.
- Comment, card, and checklist write tools ask the model to preview the text
  and wait for approval before calling.
- Read tools carry the MCP `readOnlyHint` annotation.

### Fixed

- Clearing a custom field now uses Trello's empty `PUT`; the old `DELETE`
  request is not supported by Trello.

## [1.0.0] - 2026-07-23

### Added

- A Go MCP server exposing 57 Trello tools over standard input/output.
- Persistent, permission-restricted configuration with environment overrides.
- Project-scoped setup for Codex, Claude Code, Google Antigravity, and
  OpenCode, plus generated configuration for Claude Desktop.
- LLM-oriented card reads with compact Markdown, compact JSON, recent
  comments, and `inline`, `file`, or automatic hybrid delivery.
- Workspace allow-list checks for Trello reads and writes.
- Attachment upload and download size limits with secret-safe errors.
- Homebrew HEAD, `go install`, and source installation documentation for
  macOS, Linux, and Windows.
- Prebuilt release archives for macOS, Linux, and Windows on amd64 and arm64.

[2.0.0]: https://github.com/thaitanloi365/trello-mcp/releases/tag/v2.0.0
[1.0.0]: https://github.com/thaitanloi365/trello-mcp/releases/tag/v1.0.0
