//! The single release version shared by the CLI, MCP metadata, health
//! output, and the Trello HTTP client.

pub const CURRENT: &str = env!("CARGO_PKG_VERSION");
pub const USER_AGENT: &str = concat!("trello-mcp/", env!("CARGO_PKG_VERSION"));
