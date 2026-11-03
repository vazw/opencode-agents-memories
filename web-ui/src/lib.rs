//! The `web-ui` app shell for the OpenCode Memory Explorer.
//!
//! A Leptos 0.7 CSR application (no server functions, no hydration) that talks
//! to the `mem-web` axum REST surface over a single same-origin `fetch` seam.
//! The module root is the public interface the integration tests link against.

pub mod agents;
pub mod api;
pub mod api_request;
pub mod confirm;
pub mod core_screens;
pub mod export_import;
pub mod layout;
pub mod nav;
pub mod paths;
pub mod routes;
pub mod screens;
pub mod shell;
pub mod skill_library;
pub mod state_block;
pub mod theme;
