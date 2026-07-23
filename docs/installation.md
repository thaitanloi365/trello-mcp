# Installing trello-mcp

`trello-mcp` supports macOS, Linux, and Windows. Go 1.26.5 or newer is
required when installing from Go source. After any installation, verify the
binary before configuring credentials:

```text
trello-mcp version
```

The expected version is currently `1.0.0`.

## Choose an installation method

| Method | Platforms | Best for |
| --- | --- | --- |
| Release archive | macOS, Linux, Windows | A prebuilt binary without installing Go |
| Homebrew HEAD formula | macOS, Linux | A Brew-managed installation built from the latest `master` source |
| `go install` | macOS, Linux, Windows | The shortest cross-platform installation |
| Source checkout | macOS, Linux, Windows | Development, auditing, and running tests before installation |

The Homebrew formula remains HEAD-only and must be installed with `--HEAD`.

## Prebuilt release archives

Download `v1.0.0` from the
[GitHub release page](https://github.com/thaitanloi365/trello-mcp/releases/tag/v1.0.0).
Choose the archive matching the operating system and CPU:

| Operating system | amd64 | arm64 |
| --- | --- | --- |
| macOS | `trello-mcp_1.0.0_darwin_amd64.tar.gz` | `trello-mcp_1.0.0_darwin_arm64.tar.gz` |
| Linux | `trello-mcp_1.0.0_linux_amd64.tar.gz` | `trello-mcp_1.0.0_linux_arm64.tar.gz` |
| Windows | `trello-mcp_1.0.0_windows_amd64.zip` | `trello-mcp_1.0.0_windows_arm64.zip` |

Download `checksums.txt` from the same release and verify the selected archive
before extracting it. On macOS or Linux:

```bash
grep 'trello-mcp_1.0.0_darwin_arm64.tar.gz' checksums.txt |
  shasum -a 256 -c -
```

Replace the filename with the archive that was downloaded. Linux users can
use `sha256sum -c -` instead of `shasum -a 256 -c -`.

On Windows:

```powershell
(Get-FileHash .\trello-mcp_1.0.0_windows_amd64.zip -Algorithm SHA256).Hash
```

Compare the printed hash with the corresponding line in `checksums.txt`, then
expand the archive and move `trello-mcp` or `trello-mcp.exe` to a directory on
PATH.

## Homebrew

The repository includes a HEAD formula in `Formula/trello-mcp.rb`. Add this
repository as an explicit tap, then install the formula:

```bash
brew tap thaitanloi365/trello-mcp \
  https://github.com/thaitanloi365/trello-mcp
brew install --HEAD thaitanloi365/trello-mcp/trello-mcp
trello-mcp version
```

This works with Homebrew on macOS and Linux. Homebrew installs Go as a build
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

## Install with Go

Install Go 1.26.5 or newer, confirm `go version`, then run:

```text
go install github.com/thaitanloi365/trello-mcp/cmd/trello-mcp@latest
```

Go writes the executable to `GOBIN` when it is set. Otherwise it uses the
`bin` directory under `GOPATH`.

### macOS

Homebrew is one way to install the Go toolchain:

```bash
brew install go
go version
go install github.com/thaitanloi365/trello-mcp/cmd/trello-mcp@latest
```

If the command is not found afterward, add Go's default binary directory to
your shell PATH:

```bash
echo 'export PATH="$(go env GOPATH)/bin:$PATH"' >> ~/.zshrc
source ~/.zshrc
trello-mcp version
```

If `go env GOBIN` returns a non-empty path, add that directory instead.

### Linux

Install a recent Go toolchain using the
[official Go installation guide](https://go.dev/doc/install) or your
distribution's package manager. Distribution packages can lag behind the
required Go version, so verify it before continuing:

```bash
go version
go install github.com/thaitanloi365/trello-mcp/cmd/trello-mcp@latest
```

Add the default Go binary directory to PATH when necessary:

```bash
echo 'export PATH="$(go env GOPATH)/bin:$PATH"' >> ~/.profile
. ~/.profile
trello-mcp version
```

If `go env GOBIN` returns a non-empty path, add that directory instead.

### Windows

Install Go using WinGet or the installer from
[go.dev/dl](https://go.dev/dl/):

```powershell
winget install --exact --id GoLang.Go
go version
go install github.com/thaitanloi365/trello-mcp/cmd/trello-mcp@latest
```

The default executable location is the `bin` directory under `GOPATH`. Inspect
it in PowerShell:

```powershell
$goBin = go env GOBIN
if (-not $goBin) {
    $goBin = Join-Path (go env GOPATH) "bin"
}
$goBin
& (Join-Path $goBin "trello-mcp.exe") version
```

Add the printed directory to the user PATH through **Settings > System >
About > Advanced system settings > Environment Variables**, then open a new
terminal.

## Build from a source checkout

### macOS and Linux

```bash
git clone https://github.com/thaitanloi365/trello-mcp.git
cd trello-mcp
go mod verify
go test ./...
go build -trimpath -o bin/trello-mcp ./cmd/trello-mcp
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
go mod verify
go test ./...
go build -trimpath -o bin\trello-mcp.exe .\cmd\trello-mcp
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

For Go installations, rerun:

```text
go install github.com/thaitanloi365/trello-mcp/cmd/trello-mcp@latest
```

For a source checkout, pull, retest, and rebuild:

```bash
git pull --ff-only
go mod verify
go test ./...
go build -trimpath -o bin/trello-mcp ./cmd/trello-mcp
```

For Homebrew, use the `brew reinstall --HEAD` command shown above.

## Uninstall and retained data

Remove the executable using the package manager that installed it, or delete
the executable from `GOBIN`, `GOPATH/bin`, `$HOME/.local/bin`, or the chosen
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
