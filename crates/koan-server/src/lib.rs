//! koan-server — GraphQL, Subsonic REST, and MCP server for koan.
//!
//! Library crate that exports server entry points. No TUI, no CLI, no clap.
//! Depends only on koan-core.

pub mod auth;
mod base;
pub mod clients;
pub mod covers;
mod demo;
pub mod graphql;
mod html;
pub mod icon;
pub mod mcp;
pub mod pair;
pub mod push;
pub mod share;
pub mod subsonic;
mod transcode;
pub mod ui;

/// Where a server's `server.url` comes from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ServerUrl {
    /// Read once, at start: a server whose configuration changes only with a
    /// restart, and tests, which share one configuration.
    Fixed(Option<String>),
    /// `share.url`, read as asked: an app sharing its library changes it as
    /// it runs, and keeps it apart from a headless server's `server.url`.
    Live,
}

impl ServerUrl {
    pub fn get(&self) -> Option<String> {
        match self {
            Self::Fixed(url) => url.clone(),
            Self::Live => koan_core::config::Config::cached().share_url(),
        }
    }
}

// Re-exports for downstream convenience.
pub use auth::AuthUser;
pub use graphql::{KoanSchema, build_schema};
pub use mcp::KoanMcpServer;
