//! The Trello MCP server as a library, for hosts that serve it in-process (mcp-hub).

// shortcut: the binary compiles these modules again and uses parts this library doesn't;
// move main.rs onto this library once the dashboard's open edits have landed.
#![allow(dead_code)]

mod config;
mod mcp;
mod trello;
mod util;
mod version;

use std::sync::Arc;

/// The Trello MCP handler for the config at `config_path` ("" for the default), checked up
/// front like `trello-mcp serve`. Calls go to calls.log as they do from `serve`, so the
/// dashboard's Logs tab still shows them.
pub fn mcp_server(config_path: &str) -> anyhow::Result<impl rmcp::ServerHandler + Clone> {
    let manager = Arc::new(config::Manager::new(config_path)?);
    manager.effective()?.validate(true)?;
    let mut server = mcp::Server::new(Arc::new(trello::Client::new(manager)));
    if let Some(dir) = config::resolve_path(config_path)?.parent() {
        server = server.with_log(dir);
    }
    Ok(server)
}
