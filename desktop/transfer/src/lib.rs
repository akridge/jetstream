//! JetStream Rust core: the transfer engine (incremental, resumable, checksum-verified folder
//! sync to Google Cloud Storage using the user's own OAuth sign-in), Cloud Storage browsing and
//! junk audits, and fast local folder analysis. Used by the `jetstream-transfer` CLI and the desktop app.

/// The engine's version: "0.2.0 (build 18, d0a5581)" for CI builds, "0.2.0 (local build)" otherwise.
pub const VERSION_TEXT: &str = env!("JETSTREAM_VERSION_TEXT");
/// CI build number and short commit ("" for local builds).
pub const BUILD_NUMBER: &str = env!("JETSTREAM_BUILD_NUMBER");
pub const COMMIT: &str = env!("JETSTREAM_COMMIT_SHORT");

pub mod audit;
pub mod auth;
pub mod browse;
pub mod compare;
pub mod fileops;
pub mod gcs;
pub mod localops;
pub mod manifest;
pub mod onedrive;
pub mod receipt;
pub mod scan;
pub mod sync;
pub mod throttle;
