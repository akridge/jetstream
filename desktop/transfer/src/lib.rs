//! JetStream Rust core: the transfer engine (incremental, resumable, checksum-verified folder
//! sync to Google Cloud Storage using the user's own OAuth sign-in), Cloud Storage browsing and
//! junk audits, and fast local folder analysis. Used by the `jetstream-transfer` CLI and the desktop app.

pub mod audit;
pub mod auth;
pub mod browse;
pub mod compare;
pub mod fileops;
pub mod gcs;
pub mod localops;
pub mod manifest;
pub mod scan;
pub mod sync;
pub mod throttle;
