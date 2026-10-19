//! JetStream transfer engine: incremental, resumable, checksum-verified folder sync to
//! Google Cloud Storage using the user's own OAuth sign-in.

pub mod auth;
pub mod gcs;
pub mod manifest;
pub mod sync;
