//! nexus-server — HTTP API + connector engine for the NEXUS workbench.
//!
//! Public surface (used by nexus-tauri + tests):
//! - [`Config`] / [`ConnectorCfg`] (config.rs)
//! - [`AppState`] + [`init_state`] + [`build_router`] + [`serve`] (state.rs)

pub mod bus;
pub mod config;
pub mod connectors;
pub mod error;
pub mod routes;
pub mod state;
pub mod tasks;

pub use config::{Config, ConnectorCfg};
pub use state::{build_router, init_state, serve, AppState};
pub use tasks::TaskEngine;
