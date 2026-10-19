//! Resumable-upload recovery against a small in-process server that follows Cloud Storage's
//! documented protocol (https://cloud.google.com/storage/docs/performing-resumable-uploads):
//! a status query (`Content-Range: bytes */N`) answers 308 with the persisted range.
//! (fake-gcs-server finalizes the object on a status query, so it can't test this path.)

use std::collections::HashMap;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use jetstream_transfer::auth::TokenProvider;
use jetstream_transfer::gcs::{crc32c_b64, GcsClient, CHUNK_ALIGN};
use jetstream_transfer::manifest::{FileRecord, Manifest};
use jetstream_transfer::sync::{sync, Event, SyncOptions};

#[derive(Default)]
struct State {
    expected_size: u64,
    received: Vec<u8>,
    /// Fail the Nth chunk PUT (1-based) with a 503 after keeping half of it.
    fail_chunk: Option<usize>,
    chunk_puts: usize,
    /// (first byte offset of each chunk PUT)
    chunk_offsets: Vec<u64>,
    status_queries: usize,
    sessions_started: usize,
}

struct Mock {
    base: String,
    state: Arc<Mutex<State>>,
}

async fn start_mock() -> Mock {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let state = Arc::new(Mutex::new(State::default()));
    let (server_state, server_base) = (state.clone(), base.clone());
    tokio::spawn(async move {
        loop {
            let (socket, _) = listener.accept().await.unwrap();
            tokio::spawn(handle(socket, server_state.clone(), server_base.clone()));
        }
    });
    Mock { base, state }
}

async fn handle(mut socket: TcpStream, state: Arc<Mutex<State>>, base: String) {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 65536];
    let header_end = loop {
        let n = socket.read(&mut chunk).await.unwrap_or(0);
        if n == 0 {
            return;
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break pos + 4;
        }
    };
    let head = String::from_utf8_lossy(&buf[..header_end]).to_string();
    let mut lines = head.lines();
    let request_line = lines.next().unwrap_or_default().to_string();
    let headers: HashMap<String, String> = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string()))
        .collect();
    let length: usize = headers.get("content-length").and_then(|v| v.parse().ok()).unwrap_or(0);
    let mut body = buf[header_end..].to_vec();
    while body.len() < length {
        let n = socket.read(&mut chunk).await.unwrap_or(0);
        if n == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..n]);
    }

    let (status, extra, payload) = respond(&request_line, &headers, body, &state, &base);
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n{extra}\r\n",
        payload.len()
    );
    let _ = socket.write_all(response.as_bytes()).await;
    let _ = socket.write_all(payload.as_bytes()).await;
    let _ = socket.shutdown().await;
}

fn range_header(have: usize) -> String {
    if have == 0 {
        String::new()
    } else {
        format!("Range: bytes=0-{}\r\n", have - 1)
    }
}

fn respond(
    request_line: &str,
    headers: &HashMap<String, String>,
    body: Vec<u8>,
    state: &Mutex<State>,
    base: &str,
) -> (String, String, String) {
    let mut s = state.lock().unwrap();
    let (method, target) = request_line.split_once(' ').map(|(m, rest)| (m, rest.split(' ').next().unwrap_or(""))).unwrap();

    if method == "GET" && target.starts_with("/storage/v1/b/bucket/o") {
        return ("200 OK".into(), "Content-Type: application/json\r\n".into(), r#"{"items":[]}"#.into());
    }
    if method == "POST" && target.contains("uploadType=resumable") {
        s.sessions_started += 1;
        s.expected_size = headers["x-upload-content-length"].parse().unwrap();
        s.received.clear();
        return ("200 OK".into(), format!("Location: {base}/session\r\n"), String::new());
    }
    if method == "PUT" && target == "/session" {
        let range = headers.get("content-range").cloned().unwrap_or_default();
        let spec = range.trim_start_matches("bytes ");
        if spec.starts_with("*/") {
            s.status_queries += 1;
            let have = s.received.len();
            return ("308 Resume Incomplete".into(), range_header(have), String::new());
        }
        let (span, _total) = spec.split_once('/').unwrap();
        let start: u64 = span.split_once('-').unwrap().0.parse().unwrap();
        s.chunk_puts += 1;
        s.chunk_offsets.push(start);
        if start as usize != s.received.len() {
            let have = s.received.len();
            return ("308 Resume Incomplete".into(), range_header(have), String::new());
        }
        if s.fail_chunk == Some(s.chunk_puts) {
            // Keep part of the chunk, then fail, like a dropped connection.
            let keep = (body.len() / 2 / CHUNK_ALIGN) * CHUNK_ALIGN;
            s.received.extend_from_slice(&body[..keep]);
            return ("503 Service Unavailable".into(), String::new(), "try again".into());
        }
        s.received.extend_from_slice(&body);
        if s.received.len() as u64 == s.expected_size {
            let json = serde_json::json!({
                "name": "data/video.bin",
                "size": s.expected_size.to_string(),
                "crc32c": crc32c_b64(crc32c::crc32c(&s.received)),
                "generation": "42",
            });
            return ("200 OK".into(), "Content-Type: application/json\r\n".into(), json.to_string());
        }
        let have = s.received.len();
        return ("308 Resume Incomplete".into(), range_header(have), String::new());
    }
    ("404 Not Found".into(), String::new(), String::new())
}

fn video(len: usize) -> Vec<u8> {
    (0..len).map(|i| (i % 251) as u8).collect()
}

struct Setup {
    mock: Mock,
    client: GcsClient,
    dir: tempfile::TempDir,
    manifest: Arc<Manifest>,
    data: Vec<u8>,
    record: FileRecord,
}

async fn setup() -> Setup {
    let mock = start_mock().await;
    let http = GcsClient::http_client();
    let client = GcsClient::with_base(http.clone(), &mock.base, Arc::new(TokenProvider::none(http)));
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    let data = video(6 * CHUNK_ALIGN + 777);
    let path = dir.path().join("src/video.bin");
    std::fs::write(&path, &data).unwrap();
    let mtime_ns = std::fs::metadata(&path)
        .unwrap()
        .modified()
        .unwrap()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos() as i64;
    let record = FileRecord { size: data.len() as u64, mtime_ns, crc32c: crc32c::crc32c(&data) };
    let manifest = Arc::new(Manifest::open(&dir.path().join("m.db")).unwrap());
    Setup { mock, client, dir, manifest, data, record }
}

async fn run(setup: &Setup) -> (jetstream_transfer::sync::Summary, Vec<Event>) {
    let mut opts = SyncOptions::new(setup.dir.path().join("src"), "gs://bucket/data").unwrap();
    opts.resumable_threshold = 1024 * 1024;
    opts.chunk_size = CHUNK_ALIGN;
    opts.retry_base = Duration::from_millis(10);
    let events = Arc::new(Mutex::new(Vec::new()));
    let sink = events.clone();
    let summary = sync(
        setup.client.clone(),
        setup.manifest.clone(),
        opts,
        "test",
        Arc::new(move |e| sink.lock().unwrap().push(e)),
        Arc::new(AtomicBool::new(false)),
    )
    .await
    .unwrap();
    let events = events.lock().unwrap().clone();
    (summary, events)
}

#[tokio::test]
async fn continues_a_saved_session_after_restart() {
    let setup = setup().await;
    // A previous run started the session and sent 2 chunks before the app was closed.
    {
        let mut s = setup.mock.state.lock().unwrap();
        s.expected_size = setup.record.size;
        s.received = setup.data[..2 * CHUNK_ALIGN].to_vec();
    }
    let session = format!("{}/session", setup.mock.base);
    setup.manifest.save_session("gs://bucket/data/", "video.bin", &setup.record, &session).unwrap();

    let (summary, events) = run(&setup).await;

    assert_eq!((summary.uploaded, summary.resumed, summary.failed), (1, 1, 0));
    let resumed_from = events.iter().find_map(|e| match e {
        Event::FileUploaded { resumed_from, .. } => Some(*resumed_from),
        _ => None,
    });
    assert_eq!(resumed_from, Some(Some(2 * CHUNK_ALIGN as u64)));
    let s = setup.mock.state.lock().unwrap();
    assert_eq!(s.sessions_started, 0, "no new session");
    assert_eq!(s.chunk_offsets.first(), Some(&(2 * CHUNK_ALIGN as u64)), "first byte sent is where the server stopped");
    assert_eq!(s.received, setup.data);
    assert!(setup.manifest.session("gs://bucket/data/", "video.bin").unwrap().is_none());
}

#[tokio::test]
async fn recovers_from_a_failed_chunk_without_resending_everything() {
    let setup = setup().await;
    setup.mock.state.lock().unwrap().fail_chunk = Some(3);

    let (summary, _) = run(&setup).await;

    assert_eq!((summary.uploaded, summary.failed), (1, 0));
    let s = setup.mock.state.lock().unwrap();
    assert_eq!(s.received, setup.data);
    assert_eq!(s.sessions_started, 1);
    assert!(s.status_queries >= 1, "asked the server how much it kept");
    // 7 chunks + 1 retry; nothing before the failed chunk was sent twice.
    assert_eq!(s.chunk_puts, 8, "offsets: {:?}", s.chunk_offsets);
}
