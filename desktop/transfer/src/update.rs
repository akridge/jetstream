//! "Is there a newer JetStream Desktop?" One unauthenticated request to GitHub's latest-release
//! endpoint (which leaves out drafts and pre-releases). Nothing is downloaded or installed.

use std::time::Duration;

use anyhow::Result;
use serde::{Deserialize, Serialize};

pub const LATEST_RELEASE_URL: &str = "https://api.github.com/repos/akridge/jetstream/releases/latest";
/// Desktop releases are tagged desktop-vX.Y.Z (other tags are the Python package's).
const TAG_PREFIX: &str = "desktop-v";
const NOTES_LIMIT: usize = 4000;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct UpdateInfo {
    /// The newer version ("0.2.1").
    pub latest: String,
    /// Its release page.
    pub url: String,
    /// Its release notes (Markdown, shortened).
    pub notes: String,
    /// The portable download, when the release has one.
    pub download: Option<String>,
}

#[derive(Deserialize)]
struct Release {
    tag_name: String,
    html_url: String,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    assets: Vec<Asset>,
}

#[derive(Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
}

/// "desktop-v0.2.1", "v0.2.1" or "0.2.1" as numbers; None for anything else.
pub fn parse_version(text: &str) -> Option<(u64, u64, u64)> {
    let text = text.trim();
    let text = text.strip_prefix(TAG_PREFIX).or_else(|| text.strip_prefix('v')).unwrap_or(text);
    let mut parts = text.split('.').map(|p| p.parse::<u64>().ok());
    let version = (parts.next()??, parts.next()??, parts.next()??);
    parts.next().is_none().then_some(version)
}

/// The update offered by a release JSON, if it's a desktop release newer than `current`.
pub fn newer_than(current: &str, release_json: &str) -> Option<UpdateInfo> {
    let release: Release = serde_json::from_str(release_json).ok()?;
    if release.draft || release.prerelease || !release.tag_name.starts_with(TAG_PREFIX) {
        return None;
    }
    let latest = parse_version(&release.tag_name)?;
    if latest <= parse_version(current)? {
        return None;
    }
    let mut notes = release.body.unwrap_or_default();
    if notes.len() > NOTES_LIMIT {
        let mut cut = NOTES_LIMIT;
        while !notes.is_char_boundary(cut) {
            cut -= 1;
        }
        notes.truncate(cut);
        notes.push('…');
    }
    let download = release
        .assets
        .iter()
        .find(|a| a.name.ends_with("-portable.zip") && (cfg!(windows) || !a.name.contains("windows")))
        .map(|a| a.browser_download_url.clone());
    Some(UpdateInfo { latest: format!("{}.{}.{}", latest.0, latest.1, latest.2), url: release.html_url, notes, download })
}

/// Ask GitHub whether a newer version than `current` is out. Ok(None): up to date, or no
/// desktop release is published. Errors (offline, rate limited) are for the caller to ignore.
pub async fn check(current: &str, url: Option<&str>) -> Result<Option<UpdateInfo>> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .user_agent(format!("JetStream-Desktop/{current}"))
        .build()?;
    let response = client
        .get(url.unwrap_or(LATEST_RELEASE_URL))
        .header("Accept", "application/vnd.github+json")
        .send()
        .await?;
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None); // nothing published yet (or the repository isn't public)
    }
    let text = response.error_for_status()?.text().await?;
    Ok(newer_than(current, &text))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release(tag: &str, draft: bool, prerelease: bool) -> String {
        serde_json::json!({
            "tag_name": tag,
            "html_url": format!("https://github.com/akridge/jetstream/releases/tag/{tag}"),
            "body": "## Notes",
            "draft": draft,
            "prerelease": prerelease,
            "assets": [
                {"name": "JetStream_0.2.1_x64-setup.exe", "browser_download_url": "https://example.com/setup.exe"},
                {"name": "JetStream-0.2.1-build30-windows-x64-portable.zip", "browser_download_url": "https://example.com/portable.zip"}
            ]
        })
        .to_string()
    }

    #[test]
    fn versions() {
        assert_eq!(parse_version("desktop-v0.2.1"), Some((0, 2, 1)));
        assert_eq!(parse_version("v1.0.0"), Some((1, 0, 0)));
        assert_eq!(parse_version("0.10.0"), Some((0, 10, 0)));
        assert_eq!(parse_version("0.2"), None);
        assert_eq!(parse_version("0.2.1.4"), None);
        assert_eq!(parse_version("0.2.x"), None);
        assert!(parse_version("0.2.0") < parse_version("0.2.1"));
        assert!(parse_version("0.2.1") < parse_version("0.10.0"), "numbers, not text");
    }

    #[test]
    fn offers_only_newer_desktop_releases() {
        let update = newer_than("0.2.0", &release("desktop-v0.2.1", false, false)).unwrap();
        assert_eq!(update.latest, "0.2.1");
        assert!(update.url.ends_with("desktop-v0.2.1"));
        assert_eq!(update.notes, "## Notes");
        if cfg!(windows) {
            assert_eq!(update.download.as_deref(), Some("https://example.com/portable.zip"));
        }
        assert!(newer_than("0.2.1", &release("desktop-v0.2.1", false, false)).is_none(), "same version");
        assert!(newer_than("0.3.0", &release("desktop-v0.2.1", false, false)).is_none(), "older");
        assert!(newer_than("0.2.0", &release("desktop-v0.2.1", true, false)).is_none(), "draft");
        assert!(newer_than("0.2.0", &release("desktop-v0.2.1", false, true)).is_none(), "pre-release");
        assert!(newer_than("0.2.0", &release("v0.1.30", false, false)).is_none(), "Python package tag");
        assert!(newer_than("0.2.0", "not json").is_none());
    }

    #[tokio::test]
    async fn not_found_means_nothing_published() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let body = release("desktop-v0.3.0", false, false);
        tokio::spawn(async move {
            for status in ["404 Not Found", "200 OK"] {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut buf = [0u8; 2048];
                let _ = socket.read(&mut buf).await;
                let payload = if status.starts_with("200") { body.as_str() } else { "{}" };
                let reply = format!("HTTP/1.1 {status}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{payload}", payload.len());
                socket.write_all(reply.as_bytes()).await.unwrap();
            }
        });
        let url = format!("http://{addr}/latest");
        assert_eq!(check("0.2.0", Some(&url)).await.unwrap(), None);
        assert_eq!(check("0.2.0", Some(&url)).await.unwrap().unwrap().latest, "0.3.0");
    }
}
