# Trello MCP Server for Go

A Go implementation of a Model Context Protocol server for Trello, inspired by
[`delorenj/mcp-server-trello`](https://github.com/delorenj/mcp-server-trello).
It exposes 28 token-lean MCP tools, runs over stdio, and adds a first-class
CLI for persistent configuration.

## Install

Install the latest source directly with Go 1.26.5 or newer:

```bash
go install github.com/thaitanloi365/trello-mcp/cmd/trello-mcp@latest
```

For Homebrew, source builds, PATH setup, upgrades, and platform-specific
instructions for macOS, Linux, and Windows, see the
[installation guide](docs/installation.md).

## Configure from the CLI

The default config file is `~/.trello-mcp/config.json`. The directory and file
are created when configuration is first saved, and the file uses mode `0600`.

```bash
bin/trello-mcp config set \
  --api-key "your-api-key" \
  --token "your-api-token" \
  --board-id "optional-default-board-id" \
  --workspace-id "optional-workspace-id"

bin/trello-mcp config show
bin/trello-mcp config validate
bin/trello-mcp config path
```

Additional persisted options:

```bash
bin/trello-mcp config set \
  --allowed-workspaces "workspace-id-1,workspace-id-2" \
  --api-base-url "https://api.trello.com/1" \
  --timeout-seconds 30 \
  --max-upload-bytes 10485760
```

Use `--config /path/to/config.json` on any `serve` or `config` command to use a
different file. `TRELLO_MCP_CONFIG` provides the same override. To remove
values, use `--clear-credentials`, `--clear-board`, `--clear-workspace`, or
`--clear-allowed-workspaces` with `config set`.

`config show` masks credentials. Only use `--reveal-secrets` when it is safe to
print them.

### Configuration precedence

Runtime configuration is resolved in this order, from lowest to highest:

1. Built-in defaults
2. The JSON config file
3. Environment variables

Environment overrides are never written back to the config file. Active board
and workspace changes made through `set_active_board` and
`set_active_workspace` are persisted to the JSON file.

| JSON field | Environment variable | Default |
| --- | --- | --- |
| `api_key` | `TRELLO_API_KEY` | required |
| `token` | `TRELLO_TOKEN` | required |
| `default_board_id` | `TRELLO_BOARD_ID` | empty |
| `workspace_id` | `TRELLO_WORKSPACE_ID` | empty |
| `allowed_workspace_ids` | `TRELLO_ALLOWED_WORKSPACES` | unrestricted |
| `api_base_url` | `TRELLO_API_BASE_URL` | `https://api.trello.com/1` |
| `request_timeout_seconds` | `TRELLO_REQUEST_TIMEOUT_SECONDS` | `30` |
| `max_upload_bytes` | `TRELLO_MAX_UPLOAD_BYTES` | `10485760` |

See [`config.example.json`](config.example.json) for the complete file shape.

## Connect AI coding clients

Running without a subcommand is equivalent to `serve`:

```bash
bin/trello-mcp
# or explicitly
bin/trello-mcp serve
```

The server uses the standard MCP stdio transport and supports Codex, Claude
Code, Claude Desktop, Google Antigravity, and OpenCode. Generate a ready-to-paste
client configuration with the absolute executable path filled in:

```bash
bin/trello-mcp client-config codex
bin/trello-mcp client-config claude-code
bin/trello-mcp client-config claude-desktop
bin/trello-mcp client-config antigravity
bin/trello-mcp client-config opencode
```

`claude`, `antigrav`, and `open-code` are accepted as short client aliases. Use
`--command /installed/path/to/trello-mcp` when generating configuration for a
binary that will be installed somewhere else. Generated configurations omit
`--config` by default because trello-mcp automatically loads
`~/.trello-mcp/config.json`. Pass `--config /path/to/config.json` only when the
client should use a non-default config file.

### Project-scoped setup

From a project root, `setup --scope project` safely creates or merges the
project-local MCP configuration while preserving unrelated settings and MCP
servers:

```bash
trello-mcp setup --all --scope project
# or configure one client
trello-mcp setup codex --scope project
trello-mcp setup claude-code --scope project
trello-mcp setup antigravity --scope project
trello-mcp setup opencode --scope project
```

`--all` configures every client with project-scoped MCP support: Codex, Claude
Code, Google Antigravity, and OpenCode.

The target files are:

| Client | Project configuration |
| --- | --- |
| Codex | `.codex/config.toml` |
| Claude Code | `.mcp.json` |
| Google Antigravity | `.agents/mcp_config.json` |
| OpenCode | `opencode.json` |

Use `--project-dir /path/to/project` to configure another project. Claude
Desktop does not provide an official project-scoped MCP configuration; use its
desktop-level configuration described below.

### zsh shortcuts and no-rebuild development

Add the following to `~/.zshrc`, replacing `/absolute/path/to/trello-mcp` with
this repository's path:

```zsh
# Fast normal use of the last built binary.
alias trello-mcp='/absolute/path/to/trello-mcp/bin/trello-mcp'

# Always run the latest source; no manual go build is needed.
alias trello-mcp-dev='/absolute/path/to/trello-mcp/scripts/trello-mcp-dev'
```

Reload zsh with `source ~/.zshrc`. For a no-rebuild project integration, run:

```bash
trello-mcp-dev setup opencode --scope project \
  --command /absolute/path/to/trello-mcp/scripts/trello-mcp-dev
```

MCP clients launch executables directly and do not load zsh aliases, which is
why their `--command` must point to the real binary or `scripts/trello-mcp-dev`.

### Codex

Paste the generated TOML into `~/.codex/config.toml`, or register the server
with the [Codex CLI](https://learn.chatgpt.com/docs/mcp):

```bash
codex mcp add trello -- /absolute/path/to/trello-mcp serve
```

Codex CLI and the Codex IDE extension share this MCP configuration.

### Claude Code

Paste the generated JSON into a project `.mcp.json`, or add a user-scoped
server with the [Claude Code CLI](https://code.claude.com/docs/en/mcp):

```bash
claude mcp add --transport stdio --scope user trello -- \
  /absolute/path/to/trello-mcp serve
```

### Claude Desktop

Merge the generated JSON into `claude_desktop_config.json`, then completely
restart Claude Desktop. The [official MCP guide](https://modelcontextprotocol.io/docs/develop/connect-local-servers)
uses these locations:

- macOS: `~/Library/Application Support/Claude/claude_desktop_config.json`
- Windows: `%APPDATA%\Claude\claude_desktop_config.json`

### Google Antigravity

In Antigravity, open **MCP Servers > Manage MCP Servers > View raw config** and
merge the generated JSON. The [Antigravity MCP documentation](https://antigravity.google/docs/mcp)
lists these locations:

- Global: `~/.gemini/config/mcp_config.json`
- Workspace: `.agents/mcp_config.json`

### OpenCode

Merge the generated JSON into the [OpenCode configuration](https://opencode.ai/docs/mcp-servers/).
The renderer uses OpenCode's local MCP schema with `type: "local"` and a command
array. Supported locations are:

- Global: `~/.config/opencode/opencode.json`
- Project: `opencode.json` in the project root

Credentials remain in trello-mcp's `0600` config file, so they do not need to
be duplicated in any client configuration. Environment variables remain
available for secret-manager and container workflows.

## Tools

28 tools, built for an AI reader: few tools, names instead of IDs, and short
responses.

- Read: `get_card`, `list_cards`, `get_activity`, `get_board`,
  `list_boards`, `list_workspaces`, `find_checklist_items`, `get_health`
- Cards: `create_cards`, `update_card`, `set_custom_field`, `copy_card`,
  `add_attachment`, `download_attachment`
- Comments: `add_comment`, `update_comment`, `delete_comment`
- Checklists: `create_checklist`, `add_checklist_item`,
  `update_checklist_item`, `delete_checklist_item`
- Lists, boards, and labels: `create_list`, `update_list`, `create_board`,
  `set_active`, `create_label`, `update_label`, `delete_label`

Read tools carry the MCP `readOnlyHint` annotation. Board tools take an
optional `boardId` and otherwise use the active board. Workspace allow-list
checks run before every read and write.

### Names in, names out

Cards take a full ID, an 8-character short link, or a `trello.com/c/...` URL.
Lists, labels, members, and custom fields take a name or an ID: names match
without regard to case, members also match by username or `me`, and unnamed
labels match by color. An unknown or ambiguous name fails before anything is
written and lists the options.

Results use the same names. `list_cards` returns each card's short link,
title, list, dates, labels, and members, not raw Trello IDs. `get_board`
returns the board's lists, labels, members, and custom fields with their IDs
in one call.

`update_card` changes any card field in one call: title, description, dates,
reminder (`dueReminder` in minutes, `-1` for none), list, position, archive,
watch, and labels or members to add or remove. `create_checklist` takes its
`items`. `set_custom_field` takes the field name and a plain value, or `clear: true`
to clear it.

### Output format

`get_card` returns Markdown: board, list, dates, labels, members, custom
fields, link, description, checklists, attachment links, and the latest
comments. Each checklist, item, attachment, and comment shows the ID a write
tool needs. `commentsLimit` defaults to `10` (maximum `100`). `delivery` is
`auto` (default), `inline`, or `file`; `auto` writes cards over 16 KiB to a
private file under the user cache directory (mode `0600`) and returns its path.

Other tools return compact JSON without HTML escaping and without empty fields.
Write tools return a short acknowledgement (ID, name, link, dates, state)
instead of the full Trello object. `get_activity` returns one flat entry per
action: who, what, which card, and which fields changed.

### Replies

`add_comment` and `update_comment` tell the model to write plain English,
main point first, in short bullets, and to link files as `[name](url)`. Bare
Trello attachment URLs in comment text become `[file name](url)` links before
posting. Tools that write text other people see tell the model to show you the
exact text and wait for your approval.

The `draft_reply` prompt (`/mcp__trello__draft_reply` in Claude Code) reads a
card, drafts a reply in that style, and posts it only after you approve.

## Security notes

- The persistent config contains Trello credentials. Keep it private and out of
  source control.
- `attach_file_to_card` can read a local file explicitly named by the MCP
  caller. `download_attachment` can write to an explicit destination whose
  parent directory already exists. Both enforce `max_upload_bytes`.
- Trello credentials are never included in HTTP error messages or health tool
  output.
- `TRELLO_ALLOWED_WORKSPACES` or `allowed_workspace_ids` can restrict the
  server to approved workspaces.

## Development

```bash
go test ./...
go vet ./...
go build ./cmd/trello-mcp
```

The Trello client has tests for credential injection, response decoding, and
secret-safe errors. Configuration tests cover atomic persistence, permissions,
environment precedence, and keeping environment secrets out of the file.
