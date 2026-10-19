//! OAuth access tokens for Cloud Storage, using the user's own Google sign-in.
//!
//! Order of precedence:
//! 1. `STORAGE_EMULATOR_HOST` set: no auth (local emulator / tests).
//! 2. `JETSTREAM_ACCESS_TOKEN`: a token supplied by the caller (e.g. the JetStream app).
//! 3. Application Default Credentials of type `authorized_user`
//!    (`gcloud auth application-default login`, or JetStream's "Sign in"), refreshed directly
//!    against Google's token endpoint, so the SDK isn't needed at upload time.
//! 4. `gcloud auth print-access-token` as a last resort.
//!
//! No HMAC keys or other long-lived shared secrets are used.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};
use serde::Deserialize;
use tokio::sync::Mutex;

const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
/// Refresh this long before the token actually expires.
const EXPIRY_MARGIN: Duration = Duration::from_secs(300);

#[derive(Debug, Clone)]
enum Source {
    None,
    Static(String),
    AuthorizedUser { client_id: String, client_secret: String, refresh_token: String },
    GcloudCli(PathBuf),
}

#[derive(Deserialize)]
struct AdcFile {
    #[serde(rename = "type")]
    kind: String,
    client_id: Option<String>,
    client_secret: Option<String>,
    refresh_token: Option<String>,
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    expires_in: Option<u64>,
}

pub struct TokenProvider {
    source: Source,
    http: reqwest::Client,
    cached: Mutex<Option<(String, Instant)>>,
}

impl TokenProvider {
    pub fn from_environment(http: reqwest::Client) -> Result<Self> {
        Ok(Self { source: detect_source()?, http, cached: Mutex::new(None) })
    }

    pub fn none(http: reqwest::Client) -> Self {
        Self { source: Source::None, http, cached: Mutex::new(None) }
    }

    pub fn describe(&self) -> &'static str {
        match self.source {
            Source::None => "none (emulator)",
            Source::Static(_) => "JETSTREAM_ACCESS_TOKEN",
            Source::AuthorizedUser { .. } => "application default credentials",
            Source::GcloudCli(_) => "gcloud auth print-access-token",
        }
    }

    /// Bearer token, or None when talking to an emulator.
    pub async fn token(&self) -> Result<Option<String>> {
        if let Source::None = self.source {
            return Ok(None);
        }
        let mut cached = self.cached.lock().await;
        if let Some((token, expires)) = cached.as_ref() {
            if Instant::now() + EXPIRY_MARGIN < *expires {
                return Ok(Some(token.clone()));
            }
        }
        let (token, lifetime) = self.fetch().await?;
        *cached = Some((token.clone(), Instant::now() + lifetime));
        Ok(Some(token))
    }

    /// Drop the cached token (after a 401) so the next call fetches a new one.
    pub async fn invalidate(&self) {
        *self.cached.lock().await = None;
    }

    async fn fetch(&self) -> Result<(String, Duration)> {
        match &self.source {
            Source::None => unreachable!(),
            // Caller-managed token; assume it is fresh for the life of this process.
            Source::Static(token) => Ok((token.clone(), Duration::from_secs(3600))),
            Source::AuthorizedUser { client_id, client_secret, refresh_token } => {
                let response = self
                    .http
                    .post(TOKEN_URL)
                    .form(&[
                        ("grant_type", "refresh_token"),
                        ("client_id", client_id),
                        ("client_secret", client_secret),
                        ("refresh_token", refresh_token),
                    ])
                    .send()
                    .await
                    .context("could not reach Google's token endpoint")?;
                if !response.status().is_success() {
                    let status = response.status();
                    let body = response.text().await.unwrap_or_default();
                    bail!(
                        "Google sign-in has expired or was revoked ({status}). Sign in again \
                         (JetStream \"Sign in\", or `gcloud auth application-default login`). {body}"
                    );
                }
                let token: TokenResponse = response.json().await?;
                Ok((token.access_token, Duration::from_secs(token.expires_in.unwrap_or(3600))))
            }
            Source::GcloudCli(gcloud) => {
                let output = tokio::process::Command::new(gcloud)
                    .args(["auth", "print-access-token"])
                    .output()
                    .await
                    .context("could not run gcloud")?;
                if !output.status.success() {
                    bail!(
                        "gcloud could not provide an access token; run `gcloud auth login`. {}",
                        String::from_utf8_lossy(&output.stderr).trim()
                    );
                }
                let token = String::from_utf8(output.stdout)?.trim().to_string();
                // gcloud tokens last an hour; refresh well before that.
                Ok((token, Duration::from_secs(45 * 60)))
            }
        }
    }
}

fn detect_source() -> Result<Source> {
    if std::env::var_os("STORAGE_EMULATOR_HOST").is_some() {
        return Ok(Source::None);
    }
    if let Ok(token) = std::env::var("JETSTREAM_ACCESS_TOKEN") {
        if !token.trim().is_empty() {
            return Ok(Source::Static(token.trim().to_string()));
        }
    }
    for path in adc_paths() {
        if !path.is_file() {
            continue;
        }
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("could not read credentials file {}", path.display()))?;
        let adc: AdcFile = serde_json::from_str(&text)
            .with_context(|| format!("credentials file {} is not valid JSON", path.display()))?;
        if adc.kind == "authorized_user" {
            return Ok(Source::AuthorizedUser {
                client_id: adc.client_id.ok_or_else(|| anyhow!("credentials missing client_id"))?,
                client_secret: adc.client_secret.ok_or_else(|| anyhow!("credentials missing client_secret"))?,
                refresh_token: adc.refresh_token.ok_or_else(|| anyhow!("credentials missing refresh_token"))?,
            });
        }
        // Service-account and other credential types: fall through to gcloud for now.
        break;
    }
    if let Some(gcloud) = find_on_path(if cfg!(windows) { &["gcloud.cmd", "gcloud.exe"] } else { &["gcloud"] }) {
        return Ok(Source::GcloudCli(gcloud));
    }
    bail!(
        "Not signed in to Google Cloud. Use JetStream's \"Sign in\" button or run \
         `gcloud auth application-default login`."
    )
}

fn adc_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Some(path) = std::env::var_os("GOOGLE_APPLICATION_CREDENTIALS") {
        paths.push(PathBuf::from(path));
    }
    let well_known = if cfg!(windows) {
        std::env::var_os("APPDATA").map(|d| PathBuf::from(d).join("gcloud"))
    } else {
        std::env::var_os("CLOUDSDK_CONFIG")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config").join("gcloud")))
    };
    if let Some(dir) = well_known {
        paths.push(dir.join("application_default_credentials.json"));
    }
    paths
}

fn find_on_path(names: &[&str]) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .flat_map(|dir| names.iter().map(move |name| dir.join(name)))
        .find(|candidate| candidate.is_file())
}
