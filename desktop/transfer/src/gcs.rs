//! Minimal Cloud Storage JSON API client: multipart and resumable uploads, listing, and the
//! object operations behind the cloud file manager (server-side copy, delete, restore).
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

#[derive(Debug, Clone, Default, Deserialize, serde::Serialize)]
pub struct ObjectInfo {
    pub name: String,
    #[serde(default, deserialize_with = "de_u64_string")]
    pub size: u64,
    #[serde(default)]
    pub crc32c: Option<String>,
    #[serde(default)]
    pub generation: Option<String>,
    #[serde(default)]
    pub updated: Option<String>,
    #[serde(default, rename(deserialize = "storageClass"))]
    pub storage_class: Option<String>,
    #[serde(default, rename(deserialize = "contentType"))]
    pub content_type: Option<String>,
    #[serde(default, rename(deserialize = "timeCreated"))]
    pub time_created: Option<String>,
    /// Set on soft-deleted objects (listed with `softDeleted=true`).
    #[serde(default, rename(deserialize = "softDeleteTime"))]
    pub soft_delete_time: Option<String>,
    #[serde(default, rename(deserialize = "hardDeleteTime"))]
    pub hard_delete_time: Option<String>,
    /// When the storage class last changed; the early-deletion clock starts here.
    #[serde(default, rename(deserialize = "timeStorageClassUpdated"))]
    pub time_storage_class_updated: Option<String>,
    #[serde(default, rename(deserialize = "temporaryHold"))]
    pub temporary_hold: Option<bool>,
    #[serde(default, rename(deserialize = "eventBasedHold"))]
    pub event_based_hold: Option<bool>,
    #[serde(default, rename(deserialize = "kmsKeyName"))]
    pub kms_key_name: Option<String>,
}

/// The bucket settings that decide how the file manager behaves.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct BucketInfo {
    pub name: String,
    pub location: Option<String>,
    pub storage_class: Option<String>,
    pub versioning: bool,
    /// Soft delete retention; 0 when soft delete is off (deletes are permanent).
    pub soft_delete_seconds: u64,
    /// Hierarchical namespace: folders are real resources.
    pub hierarchical_namespace: bool,
    /// Autoclass manages storage classes and charges no early-deletion fees.
    pub autoclass: bool,
}

/// Metadata kept when an object is copied with `rewrite` (a rewrite with a request body uses
/// that body instead of the source's metadata, so these are passed through explicitly).
pub const KEPT_METADATA: &[&str] =
    &["contentType", "contentEncoding", "contentDisposition", "contentLanguage", "cacheControl", "metadata", "customTime", "storageClass"];

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
    #[serde(default)]
    prefixes: Vec<String>,
    #[serde(rename = "nextPageToken")]
    next_page_token: Option<String>,
}

/// One page of a delimiter listing: "subfolders" (prefixes) and objects directly under a prefix.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct DirPage {
    pub prefixes: Vec<String>,
    pub objects: Vec<ObjectInfo>,
    pub next_page_token: Option<String>,
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

    /// One page of objects and subfolders directly under `prefix` (delimiter "/").
    /// `include_folders` also lists empty folders of hierarchical-namespace buckets.
    pub async fn list_dir(
        &self,
        bucket: &str,
        prefix: &str,
        page_token: Option<&str>,
        max_results: u32,
        include_folders: bool,
    ) -> Result<DirPage> {
        let mut url = format!(
            "{}/storage/v1/b/{}/o?prefix={}&delimiter=%2F&maxResults={max_results}\
             &fields=items(name,size,updated,storageClass,contentType),prefixes,nextPageToken",
            self.base,
            encode(bucket),
            encode(prefix)
        );
        if include_folders {
            url.push_str("&includeFoldersAsPrefixes=true");
        }
        if let Some(token) = page_token {
            url.push_str(&format!("&pageToken={}", encode(token)));
        }
        let response = self.send(self.http.get(url)).await.map_err(|e| anyhow!("{e}"))?;
        if !response.status().is_success() {
            return Err(anyhow!("listing gs://{bucket}/{prefix} failed: {}", Self::error_for(response).await));
        }
        let page: ListPage = response.json().await?;
        Ok(DirPage {
            prefixes: page.prefixes,
            objects: page.items,
            next_page_token: page.next_page_token.filter(|t| !t.is_empty()),
        })
    }

    /// One page (up to 1000) of all objects under `prefix`, recursively, with size details.
    pub async fn list_flat(&self, bucket: &str, prefix: &str, page_token: Option<&str>) -> Result<DirPage> {
        let mut url = format!(
            "{}/storage/v1/b/{}/o?prefix={}&maxResults=1000&fields=items(name,size,updated,storageClass),nextPageToken",
            self.base,
            encode(bucket),
            encode(prefix)
        );
        if let Some(token) = page_token {
            url.push_str(&format!("&pageToken={}", encode(token)));
        }
        let response = self.send(self.http.get(url)).await.map_err(|e| anyhow!("{e}"))?;
        if !response.status().is_success() {
            return Err(anyhow!("listing gs://{bucket}/{prefix} failed: {}", Self::error_for(response).await));
        }
        let page: ListPage = response.json().await?;
        Ok(DirPage { prefixes: Vec::new(), objects: page.items, next_page_token: page.next_page_token.filter(|t| !t.is_empty()) })
    }

    /// One page of objects under `prefix` as raw JSON, with the given `fields` selector for items.
    pub async fn list_raw(
        &self,
        bucket: &str,
        prefix: &str,
        page_token: Option<&str>,
        item_fields: &str,
        soft_deleted: bool,
    ) -> Result<(Vec<serde_json::Value>, Option<String>)> {
        let mut url = format!(
            "{}/storage/v1/b/{}/o?prefix={}&maxResults=1000&fields={}",
            self.base,
            encode(bucket),
            encode(prefix),
            encode(&format!("items({item_fields}),nextPageToken"))
        );
        if soft_deleted {
            url.push_str("&softDeleted=true");
        }
        if let Some(token) = page_token {
            url.push_str(&format!("&pageToken={}", encode(token)));
        }
        let response = self.send(self.http.get(url)).await.map_err(|e| anyhow!("{e}"))?;
        if !response.status().is_success() {
            return Err(anyhow!("listing gs://{bucket}/{prefix} failed: {}", Self::error_for(response).await));
        }
        #[derive(Deserialize)]
        struct RawPage {
            #[serde(default)]
            items: Vec<serde_json::Value>,
            #[serde(rename = "nextPageToken")]
            next_page_token: Option<String>,
        }
        let page: RawPage = response.json().await?;
        Ok((page.items, page.next_page_token.filter(|t| !t.is_empty())))
    }

    /// Metadata for one object, or `None` if it doesn't exist.
    pub async fn get_object(&self, bucket: &str, name: &str) -> Result<Option<ObjectInfo>, GcsError> {
        match self.get_object_raw(bucket, name).await? {
            Some(raw) => Ok(Some(serde_json::from_value(raw).map_err(|e| GcsError::Retryable(e.to_string()))?)),
            None => Ok(None),
        }
    }

    /// Full JSON resource for one object (including custom metadata), or `None` if missing.
    pub async fn get_object_raw(&self, bucket: &str, name: &str) -> Result<Option<serde_json::Value>, GcsError> {
        let url = format!("{}/storage/v1/b/{}/o/{}", self.base, encode(bucket), encode(name));
        let response = self.send(self.http.get(url)).await?;
        if response.status() == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !response.status().is_success() {
            return Err(Self::error_for(response).await);
        }
        Ok(Some(response.json().await.map_err(|e| GcsError::Retryable(e.to_string()))?))
    }

    /// Server-side copy (nothing passes through this computer). Loops on the rewrite token,
    /// which large or cross-location/class copies need. `metadata` becomes the copy's metadata.
    /// With `no_clobber`, an existing destination fails with `AlreadyExists` (which, as with any
    /// 412, can also mean the source generation no longer matches). `kms_key` keeps a
    /// customer-managed encryption key on the copy.
    #[allow(clippy::too_many_arguments)]
    pub async fn rewrite(
        &self,
        src_bucket: &str,
        src: &str,
        src_generation: Option<&str>,
        dst_bucket: &str,
        dst: &str,
        metadata: &serde_json::Value,
        no_clobber: bool,
        kms_key: Option<&str>,
    ) -> Result<ObjectInfo, GcsError> {
        #[derive(Deserialize)]
        struct RewriteResponse {
            #[serde(default)]
            done: bool,
            #[serde(rename = "rewriteToken")]
            rewrite_token: Option<String>,
            resource: Option<ObjectInfo>,
        }
        let mut token: Option<String> = None;
        loop {
            let mut url = format!(
                "{}/storage/v1/b/{}/o/{}/rewriteTo/b/{}/o/{}?fields=done,rewriteToken,resource",
                self.base,
                encode(src_bucket),
                encode(src),
                encode(dst_bucket),
                encode(dst)
            );
            if no_clobber {
                url.push_str("&ifGenerationMatch=0");
            }
            if let Some(generation) = src_generation {
                url.push_str(&format!("&ifSourceGenerationMatch={generation}"));
            }
            if let Some(key) = kms_key {
                // The object reports the key version; the destination takes the key itself.
                let key = key.split("/cryptoKeyVersions/").next().unwrap_or(key);
                url.push_str(&format!("&destinationKmsKeyName={}", encode(key)));
            }
            if let Some(t) = &token {
                url.push_str(&format!("&rewriteToken={}", encode(t)));
            }
            let response = self.send(self.http.post(url).json(metadata)).await?;
            if !response.status().is_success() {
                return Err(Self::error_for(response).await);
            }
            let body: RewriteResponse = response.json().await.map_err(|e| GcsError::Retryable(e.to_string()))?;
            if body.done {
                return body.resource.ok_or_else(|| GcsError::Retryable("rewrite finished without object metadata".into()));
            }
            token = Some(body.rewrite_token.filter(|t| !t.is_empty()).ok_or_else(|| GcsError::Retryable("rewrite returned no token".into()))?);
        }
    }

    /// Delete one object. With `generation`, only that version is deleted, so an object replaced
    /// since it was listed is left alone. A missing object counts as deleted.
    pub async fn delete_object(&self, bucket: &str, name: &str, generation: Option<&str>) -> Result<(), GcsError> {
        let mut url = format!("{}/storage/v1/b/{}/o/{}", self.base, encode(bucket), encode(name));
        if let Some(generation) = generation {
            url.push_str(&format!("?ifGenerationMatch={generation}"));
        }
        let response = self.send(self.http.delete(url)).await?;
        if response.status().is_success() || response.status() == StatusCode::NOT_FOUND {
            return Ok(());
        }
        if response.status() == StatusCode::PRECONDITION_FAILED {
            return Err(GcsError::Fatal("changed since it was listed; left in place".into()));
        }
        Err(Self::error_for(response).await)
    }

    /// Bring back a soft-deleted object. Fails with `AlreadyExists` rather than replacing a live
    /// object of the same name.
    pub async fn restore_object(&self, bucket: &str, name: &str, generation: &str) -> Result<ObjectInfo, GcsError> {
        let url = format!(
            "{}/storage/v1/b/{}/o/{}/restore?generation={generation}&ifGenerationMatch=0",
            self.base,
            encode(bucket),
            encode(name)
        );
        let response = self.send(self.http.post(url)).await?;
        if !response.status().is_success() {
            return Err(Self::error_for(response).await);
        }
        response.json().await.map_err(|e| GcsError::Retryable(e.to_string()))
    }

    pub async fn get_bucket(&self, bucket: &str) -> Result<BucketInfo> {
        let url = format!(
            "{}/storage/v1/b/{}?fields=name,location,storageClass,versioning,softDeletePolicy,hierarchicalNamespace,autoclass",
            self.base,
            encode(bucket)
        );
        let response = self.send(self.http.get(url)).await.map_err(|e| anyhow!("{e}"))?;
        if !response.status().is_success() {
            return Err(anyhow!("reading bucket {bucket} failed: {}", Self::error_for(response).await));
        }
        let raw: serde_json::Value = response.json().await?;
        Ok(BucketInfo {
            name: raw["name"].as_str().unwrap_or(bucket).to_string(),
            location: raw["location"].as_str().map(str::to_string),
            storage_class: raw["storageClass"].as_str().map(str::to_string),
            versioning: raw["versioning"]["enabled"].as_bool().unwrap_or(false),
            soft_delete_seconds: match &raw["softDeletePolicy"]["retentionDurationSeconds"] {
                serde_json::Value::String(s) => s.parse().unwrap_or(0),
                v => v.as_u64().unwrap_or(0),
            },
            hierarchical_namespace: raw["hierarchicalNamespace"]["enabled"].as_bool().unwrap_or(false),
            autoclass: raw["autoclass"]["enabled"].as_bool().unwrap_or(false),
        })
    }

    /// Create a folder in a hierarchical-namespace bucket (and any missing parents).
    pub async fn create_folder(&self, bucket: &str, folder: &str) -> Result<(), GcsError> {
        let url = format!("{}/storage/v1/b/{}/folders?recursive=true", self.base, encode(bucket));
        let response = self.send(self.http.post(url).json(&serde_json::json!({ "name": folder }))).await?;
        if response.status().is_success() {
            return Ok(());
        }
        if response.status() == StatusCode::CONFLICT {
            return Err(GcsError::AlreadyExists);
        }
        Err(Self::error_for(response).await)
    }

    /// Rename a folder in a hierarchical-namespace bucket. Google does this as one operation
    /// (no per-object copies); this waits for it to finish.
    pub async fn rename_folder(&self, bucket: &str, from: &str, to: &str) -> Result<(), GcsError> {
        let url = format!(
            "{}/storage/v1/b/{}/folders/{}/renameTo/folders/{}",
            self.base,
            encode(bucket),
            encode(from),
            encode(to)
        );
        let response = self.send(self.http.post(url)).await?;
        if response.status() == StatusCode::CONFLICT {
            return Err(GcsError::AlreadyExists);
        }
        if !response.status().is_success() {
            return Err(Self::error_for(response).await);
        }
        let mut operation: serde_json::Value = response.json().await.map_err(|e| GcsError::Retryable(e.to_string()))?;
        let mut delay = Duration::from_millis(250);
        loop {
            if operation["done"].as_bool().unwrap_or(false) {
                if let Some(error) = operation.get("error").filter(|e| !e.is_null()) {
                    return Err(GcsError::Fatal(format!("folder rename failed: {}", error["message"].as_str().unwrap_or("unknown error"))));
                }
                return Ok(());
            }
            let name = operation["name"].as_str().unwrap_or_default();
            // "projects/_/buckets/<bucket>/operations/<id>" -> "<id>"
            let id = name.rsplit('/').next().unwrap_or(name);
            if id.is_empty() {
                return Err(GcsError::Fatal("folder rename returned no operation".into()));
            }
            tokio::time::sleep(delay).await;
            delay = (delay * 2).min(Duration::from_secs(5));
            let url = format!("{}/storage/v1/b/{}/operations/{}", self.base, encode(bucket), encode(id));
            let response = self.send(self.http.get(url)).await?;
            if !response.status().is_success() {
                return Err(Self::error_for(response).await);
            }
            operation = response.json().await.map_err(|e| GcsError::Retryable(e.to_string()))?;
        }
    }

    /// Folders under a prefix in a hierarchical-namespace bucket (all levels).
    pub async fn list_folders(&self, bucket: &str, prefix: &str) -> Result<Vec<String>> {
        let mut folders = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut url = format!("{}/storage/v1/b/{}/folders?prefix={}&pageSize=1000", self.base, encode(bucket), encode(prefix));
            if let Some(t) = &token {
                url.push_str(&format!("&pageToken={}", encode(t)));
            }
            let response = self.send(self.http.get(url)).await.map_err(|e| anyhow!("{e}"))?;
            if !response.status().is_success() {
                return Err(anyhow!("listing folders failed: {}", Self::error_for(response).await));
            }
            let page: serde_json::Value = response.json().await?;
            if let Some(items) = page["items"].as_array() {
                folders.extend(items.iter().filter_map(|f| f["name"].as_str().map(str::to_string)));
            }
            match page["nextPageToken"].as_str() {
                Some(t) if !t.is_empty() => token = Some(t.to_string()),
                _ => break,
            }
        }
        Ok(folders)
    }

    /// Delete an (empty) folder in a hierarchical-namespace bucket.
    pub async fn delete_folder(&self, bucket: &str, folder: &str) -> Result<(), GcsError> {
        let url = format!("{}/storage/v1/b/{}/folders/{}", self.base, encode(bucket), encode(folder));
        let response = self.send(self.http.delete(url)).await?;
        if response.status().is_success() || response.status() == StatusCode::NOT_FOUND {
            return Ok(());
        }
        Err(Self::error_for(response).await)
    }

    /// Which of `permissions` the signed-in user holds on the bucket.
    pub async fn test_permissions(&self, bucket: &str, permissions: &[&str]) -> Result<Vec<String>> {
        let query: Vec<String> = permissions.iter().map(|p| format!("permissions={}", encode(p))).collect();
        let url = format!("{}/storage/v1/b/{}/iam/testPermissions?{}", self.base, encode(bucket), query.join("&"));
        let response = self.send(self.http.get(url)).await.map_err(|e| anyhow!("{e}"))?;
        if !response.status().is_success() {
            return Err(anyhow!("permission check failed: {}", Self::error_for(response).await));
        }
        #[derive(Deserialize)]
        struct Granted {
            #[serde(default)]
            permissions: Vec<String>,
        }
        Ok(response.json::<Granted>().await?.permissions)
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
        let info = ObjectInfo { name: "x".into(), size: 1, crc32c: Some(crc32c_b64(1)), ..Default::default() };
        assert!(matches!(GcsClient::check_crc(&info, 2), Err(GcsError::Retryable(_))));
        assert!(GcsClient::check_crc(&info, 1).is_ok());
        let no_crc = ObjectInfo { crc32c: None, ..info };
        assert!(GcsClient::check_crc(&no_crc, 2).is_ok());
    }
}
