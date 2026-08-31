//! Weftwise application and domain boundaries.

pub mod action;
pub mod app;
pub mod cli;
pub mod config;
pub mod context;
pub mod message;
pub mod services;
pub mod shell;
pub mod state;
pub mod supervisor;
pub mod widgets;

pub use app::{StartupError, run};

/// Stable desktop application identifier.
pub const APPLICATION_ID: &str = "io.unfinished_works.weftwise";
