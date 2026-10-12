# Installing trello-mcp

`trello-mcp` supports macOS, Linux, and Windows. Rust 1.88 or newer is
required when installing from source. After any installation, verify the
binary before configuring credentials:

```text
trello-mcp version
```

The expected version is currently `2.0.0`.

## Choose an installation method

| Method | Platforms | Best for |
| --- | --- | --- |
| Release archive | macOS | A prebuilt binary without installing Rust |
| Homebrew HEAD formula | macOS, Linux | A Brew-managed installation built from the latest `master` source |
| `cargo install --git` | macOS, Linux, Windows | The shortest cross-platform installation |
| Source checkout | macOS, Linux, Windows | Development, auditing, and running tests before installation |

The Homebrew formula remains HEAD-only and must be installed with `--HEAD`.

## Prebuilt release archives

Download `v2.0.0` from the
[GitHub release page](https://github.com/thaitanloi365/trello-mcp/releases/tag/v2.0.0).
Archives cover macOS only. Linux and Windows build from source with
`cargo install --git`. Choose the archive matching the CPU:

| Operating system | amd64 | arm64 |
| --- | --- | --- |
| macOS | `trello-mcp_2.0.0_darwin_amd64.tar.gz` | `trello-mcp_2.0.0_darwin_arm64.tar.gz` |

Download `checksums.txt` from the same release and verify the selected archive
before extracting it:

```bash
grep 'trello-mcp_2.0.0_darwin_arm64.tar.gz' checksums.txt |
  shasum -a 256 -c -
```

Replace the filename with the archive that was downloaded, extract it, and
move `trello-mcp` to a directory on PATH.

## Homebrew

The repository includes a HEAD formula in `Formula/trello-mcp.rb`. Add this
repository as an explicit tap, then install the formula:

```bash
brew tap thaitanloi365/trello-mcp \
  https://github.com/thaitanloi365/trello-mcp
brew install --HEAD thaitanloi365/trello-mcp/trello-mcp
trello-mcp version
```

This works with Homebrew on macOS and Linux. Homebrew installs Rust as a build
dependency, compiles `trello-mcp`, and places the resulting executable in
Homebrew's binary directory.

To rebuild from the latest `master` source:

```bash
brew update
brew reinstall --HEAD thaitanloi365/trello-mcp/trello-mcp
```

To uninstall the binary and optionally remove the tap:

```bash
brew uninstall trello-mcp
brew untap thaitanloi365/trello-mcp
```

## Install with cargo

Install Rust 1.88 or newer with [rustup](https://rustup.rs/), confirm
`cargo --version`, then run:

```text
cargo install --git https://github.com/thaitanloi365/trello-mcp --locked
```

Cargo writes the executable to `$CARGO_HOME/bin`, which is `~/.cargo/bin` by
default (`%USERPROFILE%\.cargo\bin` on Windows). rustup adds that directory to
PATH; if the command is not found, add it and open a new terminal:

```bash
trello-mcp version
```

On Linux and Windows, a C compiler and linker must be installed first
(`build-essential` on Debian and Ubuntu, Visual Studio Build Tools on
Windows).

## Build from a source checkout

### macOS and Linux

```bash
git clone https://github.com/thaitanloi365/trello-mcp.git
cd trello-mcp
cargo test
make
./bin/trello-mcp version
```

To make the binary available without keeping the repository on PATH:

```bash
mkdir -p "$HOME/.local/bin"
install -m 0755 bin/trello-mcp "$HOME/.local/bin/trello-mcp"
```

Add `$HOME/.local/bin` to PATH if it is not already present.

### Windows

```powershell
git clone https://github.com/thaitanloi365/trello-mcp.git
Set-Location trello-mcp
cargo test
cargo build --release --locked
New-Item -ItemType Directory -Force bin | Out-Null
Copy-Item .\target\release\trello-mcp.exe .\bin\trello-mcp.exe
.\bin\trello-mcp.exe version
```

To install the executable under your user profile:

```powershell
$target = Join-Path $HOME "bin"
New-Item -ItemType Directory -Force $target | Out-Null
Copy-Item .\bin\trello-mcp.exe $target
```

Add that directory to the user PATH, then open a new terminal.

## Configure and connect an MCP client

After the installed command resolves from PATH:

```bash
trello-mcp config set \
  --api-key "your-api-key" \
  --token "your-api-token"
trello-mcp config validate
trello-mcp config path
```

Command-line arguments can be stored in shell history. Use
`TRELLO_API_KEY` and `TRELLO_TOKEN` from a secret manager when that is a
concern. The persisted configuration is stored at
`~/.trello-mcp/config.json` with mode `0600`.

Generate or install project-scoped MCP client configuration:

```bash
trello-mcp client-config codex
trello-mcp setup --all --scope project
```

See the main [README](../README.md#connect-ai-coding-clients) for individual
Codex, Claude Code, Claude Desktop, Google Antigravity, and OpenCode setup.

## Upgrade

For cargo installations, rerun:

```text
cargo install --git https://github.com/thaitanloi365/trello-mcp --locked --force
```

For a source checkout, pull, retest, and rebuild:

```bash
git pull --ff-only
cargo test
make
```

Upgrading a checkout from 1.x: the Go binary's `update` cannot build the Rust
source, so run `make` once by hand. `trello-mcp update`
works from then on.

For Homebrew, use the `brew reinstall --HEAD` command shown above.

## Uninstall and retained data

Remove the executable using the package manager that installed it, or delete
the executable from `$CARGO_HOME/bin`, `$HOME/.local/bin`, or the chosen
Windows directory.

Uninstalling the binary intentionally leaves configuration and cached card
outputs in place:

- Configuration: `~/.trello-mcp`
- macOS cache: `~/Library/Caches/trello-mcp`
- Linux cache: `$XDG_CACHE_HOME/trello-mcp` when set, otherwise
  `~/.cache/trello-mcp`
- Windows cache: `%LocalAppData%\trello-mcp`

Review those directories before deleting them because the configuration can
contain Trello credentials and cached output can contain private card data.
