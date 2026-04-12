//! koan-server — GraphQL, Subsonic REST, and MCP server for koan.
//!
//! Library crate that exports server entry points. No TUI, no CLI, no clap.
//! Depends only on koan-core.

pub mod auth;
pub mod graphql;
pub mod mcp;
pub mod subsonic;

// Re-exports for downstream convenience.
pub use auth::AuthUser;
pub use graphql::{KoanSchema, build_schema};
pub use mcp::KoanMcpServer;
