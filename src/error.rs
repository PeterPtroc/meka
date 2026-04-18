//! Crate-wide [`AgshError`] enum and [`Result`] alias. All non-binary code
//! paths return `Result<T, AgshError>`; the `main` binary wraps these in
//! `anyhow::Result` for top-level reporting.

use thiserror::Error;

#[derive(Error, Debug)]
pub enum AgshError {
    #[error("configuration error: {0}")]
    Config(String),

    #[error("database error: {0}")]
    Database(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("provider error: {0}")]
    Provider(String),

    #[error("tool execution error: {tool_name}: {message}")]
    ToolExecution { tool_name: String, message: String },

    #[error("session already attached by another process: {0}")]
    SessionLocked(uuid::Uuid),

    #[error("agent interrupted by user")]
    Interrupted,

    #[error("SSE stream error: {0}")]
    StreamError(String),

    #[error("MCP connection error: {server_name}: {message}")]
    McpConnection {
        server_name: String,
        message: String,
    },

    #[error("MCP tool error: {server_name}: {tool_name}: {message}")]
    McpToolExecution {
        server_name: String,
        tool_name: String,
        message: String,
    },

    #[error("MCP authentication error: {server_name}: {message}")]
    McpAuth {
        server_name: String,
        message: String,
    },
}

pub type Result<T> = std::result::Result<T, AgshError>;
