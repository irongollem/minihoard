//! `mmf-core` — the engine behind minihoard.
//!
//! All real logic lives here as a library so that the CLI (`mmf-cli`) and an
//! eventual MCP server are thin facades over the same code. Milestones:
//!
//! - M0  feasibility spike: auth + list one owned item + download it
//! - M1  config + scaffolding (done)
//! - M2  [`auth`] interactive login + token refresh
//! - M3  [`api`] library/collection listing
//! - M4  list command + local manifest
//! - M5  [`download`] resumable downloads
//! - M6  [`unpack`] nested-archive extraction (zip done)
//! - M7  sync orchestration + state manifest

pub mod api;
pub mod auth;
pub mod clean;
pub mod config;
pub mod download;
pub mod error;
pub mod library;
pub mod manifest;
pub mod pipeline;
pub mod unpack;

pub use config::Config;
pub use error::{Error, Result};
