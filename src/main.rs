mod clientconfig;
mod config;
mod dashboard;
mod mcp;
mod trello;
mod util;
mod version;

use std::{
    io::Write,
    path::{Path, PathBuf},
    process::{Command as Process, ExitCode, Stdio},
    sync::Arc,
};

use anyhow::{Context, bail};
use clap::{CommandFactory, Parser, Subcommand, error::ErrorKind};
use rmcp::ServiceExt;

#[derive(Parser)]
#[command(
    name = "trello-mcp",
    about = "Run the Trello MCP server",
    disable_version_flag = true
)]
struct Cli {
    /// path to config file
    #[arg(long, global = true, value_name = "string")]
    config: Option<String>,
    /// version for trello-mcp
    #[arg(short = 'v', long)]
    version: bool,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Run the MCP server over stdio
    Serve,
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
    /// Send Trello cards to Orca as one agent session, in the order given (the planner's tool)
    OrcaSend {
        /// Orca repo id
        #[arg(long)]
        repo: String,
        /// claude, codex or agy
        #[arg(long)]
        agent: String,
        /// card links
        #[arg(required = true)]
        cards: Vec<String>,
    },
    /// Save an Orca agent's result for the dashboard (the agents' tool)
    OrcaReport {
        /// review, merged, done or blocked
        #[arg(long)]
        status: String,
        /// pull request URL
        #[arg(long)]
        pr: Option<String>,
        /// an e2e run's portal link; once per run
        #[arg(long)]
        e2e: Vec<String>,
        /// what was done and how it was checked
        #[arg(long)]
        summary: Option<String>,
        /// a draft comment for the Trello card or the client
        #[arg(long)]
        reply: Option<String>,
        /// card links
        #[arg(required = true)]
        cards: Vec<String>,
    },
    /// Create, edit, show, and validate persistent configuration
    Config {
        #[command(subcommand)]
        command: Option<ConfigCommand>,
    },
    /// Generate MCP configuration for an AI coding client
    ClientConfig {
        client: String,
        /// path clients should use to start trello-mcp
        #[arg(long)]
        command: Option<String>,
    },
    /// Configure trello-mcp in an AI coding client
    Setup {
        client: Option<String>,
        /// configure every supported AI coding client
        #[arg(long)]
        all: bool,
        /// configuration scope: user (home directory) or project
        #[arg(long, default_value = "user")]
        scope: String,
        /// project root to configure (with --scope project; default ".")
        #[arg(long)]
        project_dir: Option<String>,
        /// path clients should use to start trello-mcp
        #[arg(long)]
        command: Option<String>,
    },
    /// Pull the latest source and rebuild this binary
    Update {
        /// rebuild the current checkout without pulling
        #[arg(long)]
        no_pull: bool,
    },
    /// Print the version
    Version,
}

#[derive(Subcommand)]
enum ConfigCommand {
    /// Create the config file with defaults
    Init,
    /// Persist one or more configuration values
    Set(SetArgs),
    /// Print configuration (credentials are masked by default)
    Show {
        /// show API key and token
        #[arg(long)]
        reveal_secrets: bool,
        /// include environment overrides
        #[arg(long)]
        effective: bool,
    },
    /// Validate effective file and environment configuration
    Validate {
        /// validate structure without requiring credentials
        #[arg(long)]
        without_credentials: bool,
    },
    /// Print the resolved config path
    Path,
}

#[derive(clap::Args)]
struct SetArgs {
    /// Trello API key
    #[arg(long)]
    api_key: Option<String>,
    /// Trello API token
    #[arg(long)]
    token: Option<String>,
    /// default/active board ID
    #[arg(long)]
    board_id: Option<String>,
    /// active workspace ID
    #[arg(long)]
    workspace_id: Option<String>,
    /// comma-separated allowed workspace IDs
    #[arg(long)]
    allowed_workspaces: Option<String>,
    /// Trello API base URL
    #[arg(long)]
    api_base_url: Option<String>,
    /// HTTP request timeout in seconds
    #[arg(long)]
    timeout_seconds: Option<i64>,
    /// maximum upload/download size
    #[arg(long)]
    max_upload_bytes: Option<i64>,
    /// remove API key and token from the config file
    #[arg(long)]
    clear_credentials: bool,
    /// clear the active board
    #[arg(long)]
    clear_board: bool,
    /// clear the active workspace
    #[arg(long)]
    clear_workspace: bool,
    /// remove the workspace allow-list
    #[arg(long)]
    clear_allowed_workspaces: bool,
}

fn main() -> ExitCode {
    let args = std::env::args().skip(1).collect();
    match run(args, &mut std::io::stdout(), &mut std::io::stderr()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("trello-mcp: {err:#}");
            ExitCode::from(1)
        }
    }
}

fn run(
    mut args: Vec<String>,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> anyhow::Result<()> {
    // Keep the legacy single-dash version flag.
    if args.first().is_some_and(|arg| arg == "-version") {
        args[0] = "--version".into();
    }
    let cli = match Cli::try_parse_from(std::iter::once("trello-mcp".to_string()).chain(args)) {
        Ok(cli) => cli,
        Err(err)
            if matches!(
                err.kind(),
                ErrorKind::DisplayHelp | ErrorKind::DisplayVersion
            ) =>
        {
            write!(stdout, "{}", err.render())?;
            return Ok(());
        }
        Err(err) => bail!("{}", err.render().to_string().trim_end()),
    };
    if cli.version {
        writeln!(stdout, "{}", version::CURRENT)?;
        return Ok(());
    }
    let config_path = cli.config.unwrap_or_default();
    match cli.command {
        None | Some(Command::Serve) => serve(&config_path),
        Some(Command::Dashboard {
            addr,
            tailnet_addr,
            tailscale,
        }) => run_dashboard(&config_path, &addr, &tailnet_addr, &tailscale),
        Some(Command::OrcaSend { repo, agent, cards }) => {
            let sent = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?
                .block_on(dashboard::send_cards(&config_path, &repo, &agent, &cards))?;
            Ok(writeln!(stdout, "{sent}")?)
        }
        Some(Command::OrcaReport {
            status,
            pr,
            e2e,
            summary,
            reply,
            cards,
        }) => {
            // An empty e2e list keeps the links saved before, as a missing flag keeps any field.
            let e2e = (!e2e.is_empty()).then_some(e2e);
            let fields = serde_json::json!({"status": status, "pr": pr, "e2e": e2e,
                "summary": summary, "reply": reply});
            dashboard::report_cards(&config_path, &cards, &fields)?;
            Ok(writeln!(stdout, "saved")?)
        }
        Some(Command::Version) => Ok(writeln!(stdout, "{}", version::CURRENT)?),
        Some(Command::Config { command }) => run_config(command, &config_path, stdout, stderr),
        Some(Command::ClientConfig { client, command }) => {
            let options = resolve_client_options(command.as_deref(), &config_path)?;
            Ok(write!(
                stdout,
                "{}",
                clientconfig::render(&client, &options)?
            )?)
        }
        Some(Command::Setup {
            client,
            all,
            scope,
            project_dir,
            command,
        }) => {
            if all && client.is_some() {
                bail!("--all cannot be combined with a client");
            }
            if scope != "project" && scope != "user" {
                bail!("unsupported setup scope {scope:?} (choose user or project)");
            }
            if scope == "user" && project_dir.is_some() {
                bail!("--project-dir requires --scope project");
            }
            let clients: Vec<String> = match (all, client) {
                (true, _) => clientconfig::PROJECT_CLIENTS
                    .iter()
                    .map(|c| c.to_string())
                    .collect(),
                (false, Some(client)) => vec![client],
                (false, None) => bail!("setup requires a client or --all"),
            };
            let options = resolve_client_options(command.as_deref(), &config_path)?;
            let mut written = Vec::new();
            for client in &clients {
                let target = if scope == "user" {
                    clientconfig::setup_user(client, &options, &config::process_env())?
                } else {
                    let dir = project_dir.as_deref().unwrap_or(".");
                    clientconfig::setup_project(client, dir, &options)?
                };
                writeln!(stdout, "{}", target.display())?;
                written.push(target);
            }
            if scope == "user"
                && let Some(note) = double_load_note(&clients, &written)
            {
                writeln!(stderr, "{note}")?;
            }
            Ok(())
        }
        Some(Command::Update { no_pull }) => update(no_pull, stdout),
    }
}

/// Warns when the current folder already has a project setup of trello-mcp for
/// the clients just configured at user scope: they would load it twice.
fn double_load_note(clients: &[String], written: &[std::path::PathBuf]) -> Option<String> {
    let home = config::home_dir().ok()?;
    note_for(std::path::Path::new("."), &home, written, clients)
}

fn note_for(
    dir: &std::path::Path,
    home: &std::path::Path,
    written: &[std::path::PathBuf],
    clients: &[String],
) -> Option<String> {
    let found = clientconfig::project_setups(dir, home, written, clients);
    if found.is_empty() {
        return None;
    }
    let files: Vec<String> = found.iter().map(|p| p.display().to_string()).collect();
    Some(format!(
        "note: this folder already has a project setup of trello-mcp ({}); AI clients here may load trello twice. Remove those project entries, or keep using --scope project.",
        files.join(", ")
    ))
}

fn run_config(
    command: Option<ConfigCommand>,
    config_path: &str,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> anyhow::Result<()> {
    let Some(command) = command else {
        if let Some(sub) = Cli::command().find_subcommand_mut("config") {
            write!(stderr, "{}", sub.render_help())?;
        }
        bail!("config subcommand is required");
    };
    if matches!(command, ConfigCommand::Path) {
        // Only resolve the path: this is how users locate a broken file.
        writeln!(stdout, "{}", config::resolve_path(config_path)?.display())?;
        return Ok(());
    }
    let manager = config::Manager::new(config_path)?;
    match command {
        ConfigCommand::Path => unreachable!("handled above"),
        ConfigCommand::Init => {
            config::save(manager.path(), &manager.file())?;
            writeln!(stdout, "{}", manager.path().display())?;
        }
        ConfigCommand::Set(args) => {
            manager.update(|cfg| {
                if args.clear_credentials {
                    cfg.api_key.clear();
                    cfg.token.clear();
                }
                if args.clear_board {
                    cfg.default_board_id.clear();
                }
                if args.clear_workspace {
                    cfg.workspace_id.clear();
                }
                if args.clear_allowed_workspaces {
                    cfg.allowed_workspace_ids.clear();
                }
                if let Some(v) = args.api_key {
                    cfg.api_key = v;
                }
                if let Some(v) = args.token {
                    cfg.token = v;
                }
                if let Some(v) = args.board_id {
                    cfg.default_board_id = v;
                }
                if let Some(v) = args.workspace_id {
                    cfg.workspace_id = v;
                }
                if let Some(v) = args.allowed_workspaces {
                    cfg.allowed_workspace_ids = config::parse_workspace_ids(&v);
                }
                if let Some(v) = args.api_base_url {
                    cfg.api_base_url = v;
                }
                if let Some(v) = args.timeout_seconds {
                    cfg.request_timeout_seconds = v;
                }
                if let Some(v) = args.max_upload_bytes {
                    cfg.max_upload_bytes = v;
                }
                cfg.validate(false)
            })?;
            writeln!(stdout, "{}", manager.path().display())?;
        }
        ConfigCommand::Show {
            reveal_secrets,
            effective,
        } => {
            let mut cfg = if effective {
                manager.effective()?
            } else {
                manager.file()
            };
            if !reveal_secrets {
                cfg.api_key = mask(&cfg.api_key);
                cfg.token = mask(&cfg.token);
            }
            writeln!(stdout, "{}", serde_json::to_string_pretty(&cfg)?)?;
        }
        ConfigCommand::Validate {
            without_credentials,
        } => {
            manager.effective()?.validate(!without_credentials)?;
            writeln!(stdout, "ok")?;
        }
    }
    Ok(())
}

fn resolve_client_options(
    command: Option<&str>,
    config_path: &str,
) -> anyhow::Result<clientconfig::Options> {
    let command = match command.filter(|c| !c.is_empty()) {
        Some(command) => PathBuf::from(command),
        None => std::env::current_exe().context("resolve trello-mcp executable")?,
    };
    let command = util::absolute(&command).context("resolve trello-mcp command")?;
    let config_path = if config_path.trim().is_empty() {
        String::new()
    } else {
        config::resolve_path(config_path)?
            .to_string_lossy()
            .into_owned()
    };
    Ok(clientconfig::Options {
        command: command.to_string_lossy().into_owned(),
        config_path,
    })
}

fn mask(value: &str) -> String {
    let chars: Vec<char> = value.chars().collect();
    match chars.len() {
        0 => String::new(),
        1..=8 => "********".into(),
        n => format!(
            "{}...{}",
            chars[..4].iter().collect::<String>(),
            chars[n - 4..].iter().collect::<String>()
        ),
    }
}

fn run_dashboard(
    config_path: &str,
    addr: &str,
    tailnet_addr: &str,
    tailscale: &str,
) -> anyhow::Result<()> {
    // One owner and a few requests a minute: a current-thread runtime, as `serve` uses.
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(dashboard::run(config_path, addr, tailnet_addr, tailscale))
}

fn serve(config_path: &str) -> anyhow::Result<()> {
    // One stdio client: a current-thread runtime is enough and saves a worker thread per core.
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let manager = Arc::new(config::Manager::new(config_path)?);
    manager.effective()?.validate(true)?;
    let result = runtime.block_on(async {
        let server = mcp::Server::new(Arc::new(trello::Client::new(manager)));
        let service = server.serve(rmcp::transport::stdio()).await?;
        tokio::select! {
            result = service.waiting() => { result?; }
            () = shutdown_signal() => {}
        }
        Ok(())
    });
    // The blocking stdin read can't be cancelled; dropping the runtime would hang.
    runtime.shutdown_background();
    result
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        match signal(SignalKind::terminate()) {
            Ok(mut term) => {
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => {}
                    _ = term.recv() => {}
                }
            }
            Err(_) => {
                let _ = tokio::signal::ctrl_c().await;
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

fn update(no_pull: bool, stdout: &mut dyn Write) -> anyhow::Result<()> {
    let executable = std::env::current_exe()
        .context("resolve trello-mcp executable")?
        .canonicalize()
        .context("resolve trello-mcp executable")?;
    let root = source_root(&executable)?;
    if !no_pull {
        let status = Process::new("git")
            .args(["pull", "--ff-only"])
            .current_dir(&root)
            .stdin(Stdio::null())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .status()
            .context("git pull --ff-only")?;
        if !status.success() {
            bail!(
                "git pull --ff-only in {}: {status}; use --no-pull to rebuild the current checkout",
                root.display()
            );
        }
    }
    let status = Process::new("cargo")
        .args(["build", "--release", "--locked", "--target-dir"])
        .arg(root.join("target"))
        .current_dir(&root)
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .context("cargo build")?;
    if !status.success() {
        bail!("cargo build: {status}");
    }
    let built = root
        .join("target")
        .join("release")
        .join(format!("trello-mcp{}", std::env::consts::EXE_SUFFIX));
    let bytes = std::fs::read(&built).with_context(|| format!("read {}", built.display()))?;
    // Rename onto a new inode so the running binary and macOS code signing stay intact.
    util::write_atomic(&executable, &bytes, 0o755, ".trello-mcp-")
        .with_context(|| format!("replace {}", executable.display()))?;
    let output = Process::new(&executable)
        .arg("version")
        .output()
        .context("check rebuilt binary")?;
    if !output.status.success() {
        bail!("check rebuilt binary: {}", output.status);
    }
    write!(
        stdout,
        "updated {} to {}",
        executable.display(),
        String::from_utf8_lossy(&output.stdout)
    )?;
    Ok(())
}

/// Returns the checkout that holds `executable` at `<root>/bin/`.
fn source_root(executable: &Path) -> anyhow::Result<PathBuf> {
    let root = executable
        .parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf);
    if let Some(root) = root
        && std::fs::read_to_string(root.join("Cargo.toml"))
            .is_ok_and(|data| data.starts_with("[package]\nname = \"trello-mcp\"\n"))
    {
        return Ok(root);
    }
    bail!(
        "{} is not in a trello-mcp source checkout's bin/; reinstall with `cargo install --git https://github.com/thaitanloi365/trello-mcp` or `brew upgrade trello-mcp`",
        executable.display()
    )
}

#[cfg(test)]
mod tests {
    #[test]
    fn dashboard_defaults_to_loopback_ports() {
        let cli = Cli::try_parse_from(["trello-mcp", "dashboard"]).unwrap();
        assert!(matches!(
            cli.command,
            Some(Command::Dashboard { ref addr, ref tailnet_addr, ref tailscale })
                if addr == "127.0.0.1:8790" && tailnet_addr == "127.0.0.1:8791" && tailscale.is_empty()
        ));
    }

    use super::*;

    /// The code reads its home from `config::home_dir`, which tests pin to a
    /// temp dir: no env mutation, no way to reach the real home.
    fn isolated_home() -> &'static std::path::Path {
        config::test_home()
    }

    fn run_ok(args: &[&str]) -> (String, String) {
        let (mut out, mut err) = (Vec::new(), Vec::new());
        run(
            args.iter().map(|a| a.to_string()).collect(),
            &mut out,
            &mut err,
        )
        .unwrap_or_else(|e| panic!("run {args:?}: {e:#}"));
        (
            String::from_utf8(out).unwrap(),
            String::from_utf8(err).unwrap(),
        )
    }

    fn run_err(args: &[&str]) -> String {
        let (mut out, mut err) = (Vec::new(), Vec::new());
        format!(
            "{:#}",
            run(
                args.iter().map(|a| a.to_string()).collect(),
                &mut out,
                &mut err
            )
            .unwrap_err()
        )
    }

    #[test]
    fn help_lists_commands() {
        let (out, err) = run_ok(&["--help"]);
        for want in [
            "client-config",
            "setup",
            "--config",
            "Run the Trello MCP server",
        ] {
            assert!(out.contains(want), "help missing {want:?}:\n{out}");
        }
        assert!(err.is_empty());
    }

    #[test]
    fn version_forms() {
        for args in [&["version"][..], &["--version"], &["-version"], &["-v"]] {
            assert_eq!(
                run_ok(args),
                (format!("{}\n", version::CURRENT), String::new()),
                "{args:?}"
            );
        }
    }

    #[test]
    fn config_flag_parses_before_and_after_subcommand() {
        for args in [
            &["trello-mcp", "serve", "--config", "/p.json"][..],
            &["trello-mcp", "--config", "/p.json", "serve"],
        ] {
            let cli = Cli::try_parse_from(args).unwrap();
            assert_eq!(cli.config.as_deref(), Some("/p.json"), "{args:?}");
            assert!(matches!(cli.command, Some(Command::Serve)));
        }
    }

    #[test]
    fn persistent_config_flag_before_subcommands() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        let (out, _) = run_ok(&["--config", path.to_str().unwrap(), "config", "init"]);
        assert_eq!(out.trim(), path.to_str().unwrap());
        config::load(&path).unwrap();
    }

    #[test]
    fn config_set_persists_all_values() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        run_ok(&[
            "config",
            "set",
            "--config",
            path.to_str().unwrap(),
            "--api-key",
            "api-key",
            "--token",
            "api-token",
            "--board-id",
            "board",
            "--workspace-id",
            "workspace",
            "--allowed-workspaces",
            "workspace,second",
            "--api-base-url",
            "https://example.test/1/",
            "--timeout-seconds",
            "45",
            "--max-upload-bytes",
            "2048",
        ]);
        let cfg = config::load(&path).unwrap();
        assert_eq!(
            (
                cfg.api_key.as_str(),
                cfg.token.as_str(),
                cfg.default_board_id.as_str(),
                cfg.workspace_id.as_str()
            ),
            ("api-key", "api-token", "board", "workspace")
        );
        assert_eq!(
            (
                cfg.api_base_url.as_str(),
                cfg.request_timeout_seconds,
                cfg.max_upload_bytes
            ),
            ("https://example.test/1", 45, 2048)
        );
        assert_eq!(cfg.allowed_workspace_ids, vec!["workspace", "second"]);
    }

    #[test]
    fn config_show_masks_secrets() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        let cfg = config::Config {
            api_key: "abcdefghijk".into(),
            token: "super-secret-token".into(),
            ..Default::default()
        };
        config::save(&path, &cfg).unwrap();
        let (out, _) = run_ok(&["config", "show", "--config", path.to_str().unwrap()]);
        assert!(
            !out.contains("abcdefghijk") && !out.contains("super-secret-token"),
            "{out}"
        );
        assert!(out.contains("\"api_key\": \"abcd...hijk\""), "{out}");
    }

    #[test]
    fn mask_handles_multibyte() {
        assert_eq!(mask(""), "");
        assert_eq!(mask("short"), "********");
        assert_eq!(mask("ééééééééé"), "éééé...éééé");
    }

    #[test]
    fn config_path_ignores_broken_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.json");
        std::fs::write(&path, "{bad").unwrap();
        let (out, _) = run_ok(&["config", "path", "--config", path.to_str().unwrap()]);
        assert_eq!(out.trim(), path.to_str().unwrap());
    }

    #[test]
    fn config_error_prints_cause_once() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.json");
        std::fs::write(&path, "{bad").unwrap();
        let msg = run_err(&["config", "show", "--config", path.to_str().unwrap()]);
        assert_eq!(msg.matches("key must be a string").count(), 1, "{msg}");
    }

    #[test]
    fn config_without_subcommand_errors() {
        assert_eq!(run_err(&["config"]), "config subcommand is required");
    }

    #[test]
    fn setup_all_rejects_client_argument() {
        assert!(
            run_err(&["setup", "codex", "--all"])
                .contains("--all cannot be combined with a client")
        );
    }

    #[test]
    fn source_root_requires_rust_checkout() {
        let root = tempfile::tempdir().unwrap();
        let executable = root.path().join("bin").join("trello-mcp");
        assert!(source_root(&executable).is_err());
        std::fs::write(
            root.path().join("Cargo.toml"),
            "[package]\nname = \"trello-mcp\"\n",
        )
        .unwrap();
        assert_eq!(source_root(&executable).unwrap(), root.path());
    }

    #[test]
    fn client_config_command() {
        let command = tempfile::tempdir()
            .unwrap()
            .path()
            .join("bin")
            .join("trello-mcp");
        let command = command.to_str().unwrap();
        let cfg = tempfile::tempdir().unwrap().path().join("config.json");
        let cfg = cfg.to_str().unwrap();

        let (out, _) = run_ok(&[
            "--config",
            cfg,
            "client-config",
            "codex",
            "--command",
            command,
        ]);
        for want in [
            "[mcp_servers.trello]".to_string(),
            format!("command = \"{command}\""),
            format!("\"--config\", \"{cfg}\""),
        ] {
            assert!(out.contains(&want), "missing {want:?}:\n{out}");
        }

        for client in ["claude-code", "claude-desktop", "antigravity"] {
            let (out, _) = run_ok(&[
                "client-config",
                client,
                "--command",
                command,
                "--config",
                cfg,
            ]);
            let doc: serde_json::Value =
                serde_json::from_str(&out).unwrap_or_else(|e| panic!("{e}\n{out}"));
            let server = &doc["mcpServers"]["trello"];
            assert_eq!(server["command"], command);
            assert_eq!(server["args"].as_array().unwrap().len(), 3);
            assert_eq!(server["args"][2], cfg);
            if client == "claude-code" {
                assert_eq!(server["type"], "stdio");
            } else {
                assert!(
                    server.get("type").is_none_or(|t| t == ""),
                    "{client}: {server}"
                );
            }
        }

        let (out, _) = run_ok(&[
            "client-config",
            "opencode",
            "--command",
            command,
            "--config",
            cfg,
        ]);
        let doc: serde_json::Value = serde_json::from_str(&out).unwrap();
        let server = &doc["mcp"]["trello"];
        assert_eq!(doc["$schema"], "https://opencode.ai/config.json");
        assert_eq!(server["type"], "local");
        assert_eq!(server["enabled"], true);
        assert_eq!(
            server["command"],
            serde_json::json!([command, "serve", "--config", cfg])
        );
    }

    #[test]
    fn client_config_command_uses_server_default_config() {
        let command = tempfile::tempdir()
            .unwrap()
            .path()
            .join("bin")
            .join("trello-mcp");
        let (out, _) = run_ok(&[
            "client-config",
            "codex",
            "--command",
            command.to_str().unwrap(),
        ]);
        assert!(
            !out.contains("--config") && !out.contains(".trello-mcp"),
            "{out}"
        );
        assert!(out.contains("args = [\"serve\"]"), "{out}");
    }

    #[test]
    fn setup_project_command() {
        let project = tempfile::tempdir().unwrap();
        let command = tempfile::tempdir()
            .unwrap()
            .path()
            .join("bin")
            .join("trello-mcp");
        let command = command.to_str().unwrap();
        let cfg = tempfile::tempdir().unwrap().path().join("config.json");
        let (out, _) = run_ok(&[
            "setup",
            "opencode",
            "--scope",
            "project",
            "--project-dir",
            project.path().to_str().unwrap(),
            "--command",
            command,
            "--config",
            cfg.to_str().unwrap(),
        ]);
        let target = project.path().join("opencode.json");
        assert_eq!(out.trim(), target.to_str().unwrap());
        let data = std::fs::read_to_string(&target).unwrap();
        assert!(
            data.contains("\"type\": \"local\"") && data.contains(command),
            "{data}"
        );
    }

    #[test]
    fn setup_all_project_clients_command() {
        let project = tempfile::tempdir().unwrap();
        let command = tempfile::tempdir()
            .unwrap()
            .path()
            .join("bin")
            .join("trello-mcp");
        let command = command.to_str().unwrap();
        let cfg = tempfile::tempdir().unwrap().path().join("config.json");
        let (out, _) = run_ok(&[
            "setup",
            "--all",
            "--scope",
            "project",
            "--project-dir",
            project.path().to_str().unwrap(),
            "--command",
            command,
            "--config",
            cfg.to_str().unwrap(),
        ]);
        let want: Vec<String> = [
            ".codex/config.toml",
            ".mcp.json",
            ".agents/mcp_config.json",
            "opencode.json",
        ]
        .iter()
        .map(|rel| {
            let target = project.path().join(rel);
            let data =
                std::fs::read_to_string(&target).unwrap_or_else(|e| panic!("read {target:?}: {e}"));
            assert!(data.contains(command), "{target:?}:\n{data}");
            target.to_str().unwrap().to_string()
        })
        .collect();
        let got: Vec<&str> = out.split_whitespace().collect();
        assert_eq!(got, want);
    }

    #[test]
    fn setup_defaults_to_user_scope() {
        let home = isolated_home();
        let command = home.join("bin").join("trello-mcp");
        let (out, _) = run_ok(&["setup", "opencode", "--command", command.to_str().unwrap()]);
        let target = home.join(".config/opencode/opencode.json");
        assert_eq!(out.trim(), target.to_str().unwrap());
        assert!(
            std::fs::read_to_string(&target)
                .unwrap()
                .contains("\"trello\"")
        );
    }

    #[test]
    fn setup_user_scope_rejects_project_dir() {
        let err = run_err(&["setup", "codex", "--project-dir", "/tmp/x"]);
        assert!(
            err.contains("--project-dir requires --scope project"),
            "{err}"
        );
    }

    #[test]
    fn double_load_note_cases() {
        let (project, home) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let clients = vec!["claude-code".to_string()];
        assert!(note_for(project.path(), home.path(), &[], &clients).is_none());
        std::fs::write(
            project.path().join(".mcp.json"),
            r#"{"mcpServers":{"trello":{"command":"/x/trello-mcp"}}}"#,
        )
        .unwrap();
        let note = note_for(project.path(), home.path(), &[], &clients).unwrap();
        assert!(
            note.contains(".mcp.json") && note.contains("--scope project"),
            "{note}"
        );
        // Not for a client this run did not configure, and quiet in home.
        assert!(note_for(project.path(), home.path(), &[], &["codex".to_string()]).is_none());
        assert!(note_for(project.path(), project.path(), &[], &clients).is_none());
    }
}
