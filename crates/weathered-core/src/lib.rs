//! Weathered's launcher core.
//!
//! This crate talks to Mojang's version manifest, downloads and checks the
//! client, libraries, natives, and assets, finds a Java runtime, builds the
//! version JSON's launch command, and signs in with a Microsoft account.
//! The CLI lives in `weathered`. A future GUI can call the same functions.

#![forbid(unsafe_code)]

pub mod assets;
pub mod auth;
pub mod cancel;
pub mod catalog;
pub mod config;
pub mod content;
pub mod download;
pub mod error;
pub mod install;
pub mod instance;
pub mod java;
pub mod launch;
pub mod library;
pub mod manifest;
pub mod modpack;
pub mod paths;
pub mod platform;
pub mod processors;
pub mod progress;
pub mod rules;
pub mod store;
pub mod version;

pub use error::{Error, Result};
pub use platform::OsInfo;

/// Name reported to the game as `${launcher_name}`.
pub const LAUNCHER_NAME: &str = "weathered";

/// Version reported to the game as `${launcher_version}`.
pub const LAUNCHER_VERSION: &str = env!("CARGO_PKG_VERSION");
