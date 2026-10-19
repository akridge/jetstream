//! Minimal Cloud Storage JSON API client: multipart and resumable uploads, and listing.
//!
//! Written against the documented REST API (rather than a higher-level SDK) so that a
//! resumable session URI can be saved and continued after the app restarts.

use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Result};
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use percent_encoding::{utf8_percent_encode, AsciiSet, NON_ALPHANUMERIC};
use reqwest::header::{CONTENT_LENGTH, CONTENT_RANGE, CONTENT_TYPE, LOCATION, RANGE};
use reqwest::{Response, StatusCode};
use serde::Deserialize;

use crate::auth::TokenProvider;

/// Characters left unescaped in a URL path/query component (RFC 3986 unreserved).
const COMPONENT: &AsciiSet = &NON_ALPHANUMERIC.remove(b'-').remove(b'.').remove(b'_').remove(b'~');

/// Resumable chunks must be multiples of 256 KiB.
pub const CHUNK_ALIGN: usize = 256 * 1024;

pub fn encode(component: &str) -> String {
    utf8_percent_encode(component, COMPONENT).to_string()
}

/// crc32c in the form Cloud Storage uses: base64 of the big-endian 4 bytes.
pub fn crc32c_b64(crc: u32) -> String {
    B64.encode(crc.to_be_bytes())
}

pub fn parse_crc32c_b64(value: &str) -> Option<u32> {
    let bytes = B64.decode(value).ok()?;
    Some(u32::from_be_bytes(bytes.try_into().ok()?))
}

#[derive(Debug)]
pub enum GcsError {
    /// Object already exists (ifGenerationMatch=0 failed).
    AlreadyExists,
    /// Resumable session no longer valid; start a new one.
    SessionExpired,
    /// Worth retrying (network, 408, 429, 5xx, checksum mismatch).
    Retryable(String),
    /// Retrying won't help (permissions, bad request, ...).
    Fatal(String),
}

impl std::fmt::Display for GcsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GcsError::AlreadyExists => write!(f, "object already exists"),
            GcsError::SessionExpired => write!(f, "upload session expired"),
            GcsError::Retryable(msg) | GcsError::Fatal(msg) => write!(f, "{msg}"),
        }
    }
}

impl std::error::Error for GcsError {}

#[derive(Debug, Clone, Deserialize)]
pub struct ObjectInfo {
    pub name: String,
    #[serde(default, deserialize_with = "de_u64_string")]
    pub size: u64,
    #[serde(default)]
    pub crc32c: Option<String>,
    #[serde(default)]
    pub generation: Option<String>,
}

fn de_u64_string<'de, D: serde::Deserializer<'de>>(d: D) -> std::result::Result<u64, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Num {
        S(String),
        N(u64),
    }
    match Num::deserialize(d)? {
        Num::S(s) => s.parse().map_err(serde::de::Error::custom),
        Num::N(n) => Ok(n),
    }
}

#[derive(Deserialize)]
struct ListPage {
    #[serde(default)]
    items: Vec<ObjectInfo>,
    #[serde(rename = "nextPageToken")]
    next_page_token: Option<String>,
}

/// Progress of a resumable upload after a chunk or status query.
pub enum ResumableStatus {
    /// Server has bytes [0, offset).
    Incomplete { offset: u64 },
    Complete(ObjectInfo),
}

#[derive(Clone)]
pub struct GcsClient {
    http: reqwest::Client,
    base: String,
    auth: Arc<TokenProvider>,
}

impl GcsClient {
    pub fn new(http: reqwest::Client, auth: Arc<TokenProvider>) -> Self {
        let base = match std::env::var("STORAGE_EMULATOR_HOST") {
            Ok(host) if host.starts_with("http") => host.trim_end_matches('/').to_string(),
            Ok(host) => format!("http://{}", host.trim_end_matches('/')),
            Err(_) => "https://storage.googleapis.com".to_string(),
        };
        Self { http, base, auth }
    }

    /// Client for an explicit endpoint (tests, emulators).
    pub fn with_base(http: reqwest::Client, base: &str, auth: Arc<TokenProvider>) -> Self {
        Self { http, base: base.trim_end_matches('/').to_string(), auth }
    }

    pub fn http_client() -> reqwest::Client {
        reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(20))
            .read_timeout(Duration::from_secs(120))
            .pool_max_idle_per_host(128)
            .user_agent(concat!("jetstream-transfer/", env!("CARGO_PKG_VERSION")))
            .build()
            .expect("HTTP client")
    }

    async fn authed(&self, builder: reqwest::RequestBuilder) -> Result<reqwest::RequestBuilder, GcsError> {
        match self.auth.token().await {
            Ok(Some(token)) => Ok(builder.bearer_auth(token)),
            Ok(None) => Ok(builder),
            Err(err) => Err(GcsError::Fatal(format!("{err:#}"))),
        }
    }

    async fn send(&self, builder: reqwest::RequestBuilder) -> Result<Response, GcsError> {
        let response = self
            .authed(builder)
            .await?
            .send()
            .await
            .map_err(|e| GcsError::Retryable(format!("network error: {e}")))?;
        if response.status() == StatusCode::UNAUTHORIZED {
            // Token may have been revoked or expired early; next attempt fetches a new one.
            self.auth.invalidate().await;
            return Err(GcsError::Retryable("access token rejected (401); refreshing".into()));
        }
        Ok(response)
    }

    async fn error_for(response: Response) -> GcsError {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        let message = format!("{status}: {}", body.chars().take(500).collect::<String>());
        match status {
            StatusCode::PRECONDITION_FAILED => GcsError::AlreadyExists,
            StatusCode::NOT_FOUND | StatusCode::GONE => GcsError::Fatal(message),
            s if s == StatusCode::REQUEST_TIMEOUT || s == StatusCode::TOO_MANY_REQUESTS || s.is_server_error() => {
                GcsError::Retryable(message)
            }
            _ => GcsError::Fatal(message),
        }
    }

    fn check_crc(info: &ObjectInfo, expected: u32) -> Result<(), GcsError> {
        match info.crc32c.as_deref().and_then(parse_crc32c_b64) {
            Some(actual) if actual != expected => Err(GcsError::Retryable(format!(
                "checksum mismatch for {} (local {}, cloud {})",
                info.name,
                crc32c_b64(expected),
                crc32c_b64(actual)
            ))),
            _ => Ok(()),
        }
    }

    /// Single-request upload with the checksum in the metadata, so the server rejects corrupt data.
    pub async fn upload_multipart(
        &self,
        bucket: &str,
        name: &str,
        data: Vec<u8>,
        crc: u32,
        no_clobber: bool,
    ) -> Result<ObjectInfo, GcsError> {
        let boundary = format!("jetstream-{:016x}", rand::random::<u64>());
        let metadata = serde_json::json!({ "name": name, "crc32c": crc32c_b64(crc) });
        let mut body = Vec::with_capacity(data.len() + 512);
        body.extend_from_slice(
            format!("--{boundary}\r\nContent-Type: application/json; charset=UTF-8\r\n\r\n{metadata}\r\n--{boundary}\r\nContent-Type: application/octet-stream\r\n\r\n")
                .as_bytes(),
        );
        body.extend_from_slice(&data);
        body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());

        let mut url = format!("{}/upload/storage/v1/b/{}/o?uploadType=multipart", self.base, encode(bucket));
        if no_clobber {
            url.push_str("&ifGenerationMatch=0");
        }
        let response = self
            .send(
                self.http
                    .post(url)
                    .header(CONTENT_TYPE, format!("multipart/related; boundary={boundary}"))
                    .body(body),
            )
            .await?;
        if !response.status().is_success() {
            return Err(Self::error_for(response).await);
        }
        let info: ObjectInfo = response.json().await.map_err(|e| GcsError::Retryable(e.to_string()))?;
        Self::check_crc(&info, crc)?;
        Ok(info)
    }

    /// Start a resumable upload; returns the session URI to save for later resumption.
    pub async fn start_resumable(
        &self,
        bucket: &str,
        name: &str,
        size: u64,
        crc: u32,
        no_clobber: bool,
    ) -> Result<String, GcsError> {
        let mut url = format!(
            "{}/upload/storage/v1/b/{}/o?uploadType=resumable&name={}",
            self.base,
            encode(bucket),
            encode(name)
        );
        if no_clobber {
            url.push_str("&ifGenerationMatch=0");
        }
        let metadata = serde_json::json!({ "name": name, "crc32c": crc32c_b64(crc) });
        let response = self
            .send(
                self.http
                    .post(url)
                    .header(CONTENT_TYPE, "application/json; charset=UTF-8")
                    .header("X-Upload-Content-Type", "application/octet-stream")
                    .header("X-Upload-Content-Length", size.to_string())
                    .body(metadata.to_string()),
            )
            .await?;
        if !response.status().is_success() {
            return Err(Self::error_for(response).await);
        }
        response
            .headers()
            .get(LOCATION)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string)
            .ok_or_else(|| GcsError::Retryable("resumable upload response had no session URI".into()))
    }

    async fn resumable_response(response: Response, size: u64, crc: u32) -> Result<ResumableStatus, GcsError> {
        let status = response.status();
        if status.as_u16() == 308 {
            // "Range: bytes=0-N" means the server has N+1 bytes; no header means none.
            let offset = response
                .headers()
                .get(RANGE)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.rsplit('-').next())
                .and_then(|end| end.parse::<u64>().ok())
                .map(|end| end + 1)
                .unwrap_or(0);
            return Ok(ResumableStatus::Incomplete { offset });
        }
        if status.is_success() {
            let info: ObjectInfo = response.json().await.map_err(|e| GcsError::Retryable(e.to_string()))?;
            if info.size != size {
                return Err(GcsError::Retryable(format!(
                    "upload of {} finished with {} bytes instead of {size}",
                    info.name, info.size
                )));
            }
            GcsClient::check_crc(&info, crc)?;
            return Ok(ResumableStatus::Complete(info));
        }
        if status == StatusCode::NOT_FOUND || status == StatusCode::GONE {
            return Err(GcsError::SessionExpired);
        }
        Err(Self::error_for(response).await)
    }

    /// Ask how much of a resumable upload the server already has.
    pub async fn query_resumable(&self, session: &str, size: u64, crc: u32) -> Result<ResumableStatus, GcsError> {
        let response = self
            .send(
                self.http
                    .put(session)
                    .header(CONTENT_RANGE, format!("bytes */{size}"))
                    .header(CONTENT_LENGTH, "0"),
            )
            .await?;
        Self::resumable_response(response, size, crc).await
    }

    /// Send bytes [offset, offset+data.len()) of a `size`-byte object.
    pub async fn put_chunk(
        &self,
        session: &str,
        offset: u64,
        data: Vec<u8>,
        size: u64,
        crc: u32,
    ) -> Result<ResumableStatus, GcsError> {
        let end = offset + data.len() as u64 - 1;
        let response = self
            .send(
                self.http
                    .put(session)
                    .header(CONTENT_RANGE, format!("bytes {offset}-{end}/{size}"))
                    .body(data),
            )
            .await?;
        Self::resumable_response(response, size, crc).await
    }

    /// All objects under a prefix (one request per 1000 objects).
    pub async fn list(&self, bucket: &str, prefix: &str) -> Result<Vec<ObjectInfo>> {
        let mut objects = Vec::new();
        let mut page_token: Option<String> = None;
        loop {
            let mut url = format!(
                "{}/storage/v1/b/{}/o?prefix={}&maxResults=1000&fields=items(name,size,crc32c,generation),nextPageToken",
                self.base,
                encode(bucket),
                encode(prefix)
            );
            if let Some(token) = &page_token {
                url.push_str(&format!("&pageToken={}", encode(token)));
            }
            let response = self.send(self.http.get(url)).await.map_err(|e| anyhow!("{e}"))?;
            if !response.status().is_success() {
                return Err(anyhow!("listing gs://{bucket}/{prefix} failed: {}", Self::error_for(response).await));
            }
            let page: ListPage = response.json().await?;
            objects.extend(page.items);
            match page.next_page_token {
                Some(token) if !token.is_empty() => page_token = Some(token),
                _ => break,
            }
        }
        Ok(objects)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc32c_round_trips_in_gcs_format() {
        // Standard CRC-32C check value for "123456789".
        let crc = crc32c::crc32c(b"123456789");
        assert_eq!(crc, 0xE306_9283);
        assert_eq!(crc32c_b64(crc), "4waSgw==");
        assert_eq!(parse_crc32c_b64("4waSgw=="), Some(crc));
        assert_eq!(parse_crc32c_b64("not base64!"), None);
    }

    #[test]
    fn encodes_object_names_for_urls() {
        assert_eq!(encode("surveys/2026/IMG 0001#1.jpg"), "surveys%2F2026%2FIMG%200001%231.jpg");
        assert_eq!(encode("a-b_c.d~e"), "a-b_c.d~e");
    }

    #[test]
    fn checksum_mismatch_is_retryable() {
        let info = ObjectInfo { name: "x".into(), size: 1, crc32c: Some(crc32c_b64(1)), generation: None };
        assert!(matches!(GcsClient::check_crc(&info, 2), Err(GcsError::Retryable(_))));
        assert!(GcsClient::check_crc(&info, 1).is_ok());
        let no_crc = ObjectInfo { crc32c: None, ..info };
        assert!(GcsClient::check_crc(&no_crc, 2).is_ok());
    }
}
