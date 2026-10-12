# trello-mcp Rust rewrite: design

Date: 2026-10-12. Status: approved in chat, pending spec review.

## Goal

Rewrite trello-mcp from Go to Rust with strict 1:1 behavior parity. Rust is
now the default language for personal projects and AI tools (MCP servers,
CLIs, servers). Only trello-mcp is in scope. The other Go projects in
`/Users/loithai/Projects/ai-tools` (claude-usage, clockify-agent, device-hub,
gws-mcp) stay in Go.

Success means every existing MCP client keeps working with no config change.
About 10 project configs launch
`/Users/loithai/Projects/ai-tools/trello-mcp/bin/trello-mcp serve` (some with
`--config /Users/loithai/.trello-mcp/config.json`). After `trello-mcp update`
and a client restart, they run the Rust binary and see the same tools and
output.

## Parity contract

These stay identical to the Go version at commit `c1a3946`:

- Binary name and location: `bin/trello-mcp`.
- CLI commands and flags: `serve` (also the default with no subcommand),
  `config set|show|init|validate|path|help`, `client-config <client>`,
  `setup [client] --all --scope --project-dir --command`, `update --no-pull`,
  `version`, and the global `--config` flag accepted before or after the
  subcommand. Client aliases (`claude`, `antigrav`, `open-code`) stay.
- Config: `~/.trello-mcp/config.json` path and JSON shape, `TRELLO_MCP_CONFIG`,
  every `TRELLO_*` env var, precedence (defaults, then file, then env), env
  values never written back, atomic write with mode `0600`, masked
  `config show` unless `--reveal-secrets`.
- MCP surface: the 28 tool names, descriptions, input schemas and
  `readOnlyHint` annotations, and the `draft_reply` prompt with its arguments.
- Tool behavior: name-or-ID resolution, workspace allow-list checks before
  every read and write, Markdown `get_card` output, `delivery` modes with the
  16 KiB `auto` threshold and `0600` file under the user cache directory,
  compact JSON results without empty fields and without HTML escaping, short
  write acknowledgements, attachment URL to Markdown link rewriting, upload
  and download size limits, error message text.
- Project setup merge rules: JSON clients merge into `mcpServers` (or `mcp`
  for OpenCode) and keep every other key and server; Codex uses the managed
  block markers and refuses an unmanaged `[mcp_servers.trello]` section.
  Output stays sorted-key, two-space-indented JSON with a trailing newline.
- `client-config` output for all 5 clients.

Not covered by parity: help-text layout (clap and cobra format differently;
command and flag names and short descriptions still match).

Intentional differences:

- MCP implementation name changes from `trello-mcp-go` to `trello-mcp`, and
  the HTTP User-Agent from `trello-mcp-go/1.0.0` to `trello-mcp/2.0.0`.
- Network-error text no longer includes the request URL. Go's did, and that
  URL carries the API key and token.
- Cobra's auto-generated `completion` command is dropped.
- `update` runs `git pull --ff-only` (unless `--no-pull`), then
  `cargo build --release --locked`. It copies `target/release/trello-mcp`
  next to the running binary and renames it into place, then runs
  `version` on the result.
- `scripts/trello-mcp-dev` runs `cargo run --quiet --release --`.
- Version becomes `2.0.0`. `go install` no longer works, and the unreleased
  tool redesign is already breaking.

## Approach

A single binary crate. Tools stay table-driven, as in Go: one table of tool
and field definitions builds the JSON Schemas by hand and validates
arguments. `rmcp`'s `ServerHandler` is implemented by hand (`list_tools`,
`call_tool`, `list_prompts`, `get_prompt`) rather than with `#[tool]` macros.
schemars-generated schemas would differ from the Go schemas and break the
`tools/list` parity check.

Rejected: rmcp macros plus schemars (breaks schema parity); a Cargo workspace
with several crates (too much structure for about 3k lines of non-test code).

## Layout

```
Cargo.toml            package trello-mcp 2.0.0, edition 2024, bin trello-mcp
Cargo.lock            committed
src/main.rs           clap derive CLI; run(args, stdout, stderr) -> exit code
src/version.rs        CURRENT = env!("CARGO_PKG_VERSION")
src/config.rs         Config, defaults, load/resolve/save, validate, mask
src/trello/mod.rs     Client (reqwest), Error, request helpers
src/trello/*.rs       endpoint groups as needed to keep files small
src/mcp/server.rs     ServerHandler impl, schema builder, argument validation,
                      result encoding, tool errors, draft_reply prompt
src/mcp/tools.rs      the 28 tool definitions (port of tools.go)
src/mcp/handlers.rs   card, list and comment handlers (port of handlers.go)
src/mcp/board.rs      board, label, member, custom field resolution
src/mcp/extras.rs     get_card Markdown, delivery, activity, checklists, etc.
src/clientconfig/mod.rs      supported clients, aliases, render
src/clientconfig/project.rs  project paths, JSON merge, Codex block merge
tests/fixtures/tools_list.json      captured from the Go binary
tests/fixtures/client-config/*      captured from the Go binary
```

Module boundaries mirror the Go packages, so each Go file has one obvious
Rust counterpart. That keeps the port reviewable side by side.

## Dependencies

- `rmcp` 3.5.x, `default-features = false`, features `server`, `transport-io`.
- `tokio` (rt-multi-thread, macros, io-std, fs, process, signal).
- `clap` 4 (derive).
- `reqwest` with `rustls-tls`, `json`, `multipart`; no native-tls.
- `serde`, `serde_json`.
- `thiserror` for module errors; `anyhow` only in `main.rs`.
- `tempfile` for atomic writes.
- `dirs` for the home and user cache directories.
- Dev only: `wiremock` (replaces Go `httptest`).

Add nothing else without a reason recorded in the plan.

## Errors

- Each module has a `thiserror` enum. `main.rs` reports with `anyhow` and
  exits non-zero, matching Go exit behavior.
- Tool handler errors become a `CallToolResult` with `isError: true` and the
  same message text as Go. Argument validation errors use the Go wording.
- Trello key and token never appear in error text, logs or `get_health`
  output.
- No `unwrap()` or `expect()` on any input-driven path outside tests.

## Testing

- Port all 58 Go tests one-to-one into `#[cfg(test)]` modules beside the
  code they cover. HTTP tests use `wiremock`; CLI tests call `run` with
  in-memory stdout and stderr, as the Go tests do.
- Parity fixtures, captured from the Go binary before the Go code is deleted:
  1. `tests/fixtures/tools_list.json`: the `tools/list` and `prompts/list`
     results. A Rust test asserts JSON equality after replacing the
     implementation name.
  2. `tests/fixtures/client-config/<client>`: output of
     `client-config <client> --command /x/trello-mcp` for all 5 clients. A
     Rust test asserts byte equality.
- Checks: `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`,
  `cargo test`.
- Final manual smoke test: a real client on the new `bin/trello-mcp serve`
  calls `get_health`, `get_card` and `list_cards`. Read-only, against a real
  board.

## Build, release and install

- `scripts/build-release VERSION [OUTPUT_DIR]`: same arguments and checks.
  It builds `aarch64-apple-darwin` and `x86_64-apple-darwin` with
  `cargo build --release --locked --target`. Archives keep the
  `trello-mcp_<version>_darwin_<arm64|amd64>.tar.gz` names, plus
  `checksums.txt`. Linux and Windows archives are dropped.
- `Formula/trello-mcp.rb`: `depends_on "rust" => :build`,
  `system "cargo", "install", *std_cargo_args`. Test asserts `2.0.0`.
- `README.md` and `docs/installation.md`: Rust toolchain instead of Go;
  `cargo install --git https://github.com/thaitanloi365/trello-mcp` for
  Linux and Windows; Development section uses cargo commands.
- `CHANGELOG.md`: a `2.0.0` entry that records the Rust rewrite, the
  install changes and the implementation name change.
- `.gitignore`: add `target/`.

## Migration order

1. Capture the parity fixtures from the Go binary.
2. Build the Rust crate beside the Go code until every ported test and
   fixture test passes.
3. Delete `cmd/`, `internal/`, `go.mod` and `go.sum` in the same branch.
4. Update scripts, formula and docs; rebuild `bin/trello-mcp`; run the
   manual smoke test.

## Out of scope

- Rewriting the other Go projects.
- New tools, changed tool behavior, or HTTP transport.
- Linux and Windows prebuilt archives and CI release automation.
