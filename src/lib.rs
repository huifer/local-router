//! local-router: a local gateway that gives every coding agent every model.

pub mod agents;
pub mod config;
pub mod dashboard;
pub mod gateway;
pub mod oauth;
pub mod presets;
pub mod pricing;
pub mod projects;
pub mod protocol;
pub mod qwen;
pub mod server;
pub mod session_logins;
pub mod sessions;
pub mod store;
#[cfg(test)]
pub(crate) mod testenv;
pub mod web;
pub mod workbuddy;
