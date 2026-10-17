//! streagit – on-demand, streaming, stagit-compatible git web renderer.
//!
//! See `SPEC.md` for the full specification. The crate is a library so that
//! integration tests can drive the router directly.

pub mod auth;
pub mod cache;
pub mod config;
pub mod exclude;
pub mod git;
pub mod http;
pub mod index;
pub mod render;
pub mod server;
