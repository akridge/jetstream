//! End-to-end tests against fake-gcs-server (https://github.com/fsouza/fake-gcs-server).
//!
//! Set FAKE_GCS_SERVER to the emulator binary to run them, e.g.
//!   go install github.com/fsouza/fake-gcs-server@latest
//!   FAKE_GCS_SERVER=$(go env GOPATH)/bin/fake-gcs-server cargo test
//! Without it these tests are skipped.

use std::net::TcpListener;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use jetstream_transfer::auth::TokenProvider;
use jetstream_transfer::gcs::{GcsClient, ResumableStatus, CHUNK_ALIGN};
use jetstream_transfer::manifest::{FileRecord, Manifest};
use jetstream_transfer::sync::{sync, Event, Summary, SyncOptions};

struct Emulator {
    child: Child,
    base: String,
}

impl Drop for Emulator {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn start_emulator() -> Option<Emulator> {
    let binary = std::env::var("FAKE_GCS_SERVER").ok()?;
    let port = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let child = Command::new(binary)
        .args(["-scheme", "http", "-host", "127.0.0.1", "-port", &port.to_string()])
        .args(["-public-host", &format!("127.0.0.1:{port}"), "-backend", "memory", "-log-level", "error"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("start fake-gcs-server");
    let deadline = Instant::now() + Duration::from_secs(10);
    while std::net::TcpStream::connect(("127.0.0.1", port)).is_err() {
        assert!(Instant::now() < deadline, "fake-gcs-server did not start");
        std::thread::sleep(Duration::from_millis(50));
    }
    Some(Emulator { child, base: format!("http://127.0.0.1:{port}") })
}

struct Env {
    emulator: Emulator,
    client: GcsClient,
    http: reqwest::Client,
    dir: tempfile::TempDir,
    manifest: Arc<Manifest>,
}

impl Env {
    async fn new() -> Option<Self> {
        let emulator = start_emulator()?;
        let http = GcsClient::http_client();
        let client = GcsClient::with_base(http.clone(), &emulator.base, Arc::new(TokenProvider::none(http.clone())));
        http.post(format!("{}/storage/v1/b", emulator.base))
            .json(&serde_json::json!({ "name": "bucket" }))
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap();
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        let manifest = Arc::new(Manifest::open(&dir.path().join("manifest.db")).unwrap());
        Some(Self { emulator, client, http, dir, manifest })
    }

    fn src(&self) -> std::path::PathBuf {
        self.dir.path().join("src")
    }

    fn write(&self, rel: &str, data: &[u8]) {
        let path = self.src().join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, data).unwrap();
    }

    fn options(&self) -> SyncOptions {
        let mut opts = SyncOptions::new(self.src(), "gs://bucket/data").unwrap();
        opts.resumable_threshold = 1024 * 1024;
        opts.chunk_size = CHUNK_ALIGN;
        opts.retry_base = Duration::from_millis(20);
        opts
    }

    async fn run(&self, opts: SyncOptions) -> (Summary, Vec<Event>) {
        self.run_with_manifest(opts, self.manifest.clone()).await
    }

    async fn run_with_manifest(&self, opts: SyncOptions, manifest: Arc<Manifest>) -> (Summary, Vec<Event>) {
        let events = Arc::new(Mutex::new(Vec::new()));
        let sink = events.clone();
        let summary = sync(
            self.client.clone(),
            manifest,
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

    async fn object(&self, name: &str) -> Option<(u64, Vec<u8>)> {
        let url = format!("{}/storage/v1/b/bucket/o/{}?alt=media", self.emulator.base, jetstream_transfer::gcs::encode(name));
        let response = self.http.get(url).send().await.unwrap();
        if !response.status().is_success() {
            return None;
        }
        let body = response.bytes().await.unwrap().to_vec();
        Some((body.len() as u64, body))
    }

    async fn delete(&self, name: &str) {
        let url = format!("{}/storage/v1/b/bucket/o/{}", self.emulator.base, jetstream_transfer::gcs::encode(name));
        self.http.delete(url).send().await.unwrap();
    }
}

fn big(len: usize, seed: u8) -> Vec<u8> {
    (0..len).map(|i| (i as u8).wrapping_mul(31).wrapping_add(seed)).collect()
}

fn set_mtime(path: &Path, secs_ago: u64) {
    let time = std::time::SystemTime::now() - Duration::from_secs(secs_ago);
    std::fs::File::options().write(true).open(path).unwrap().set_modified(time).unwrap();
}

#[tokio::test]
async fn first_sync_then_incremental_resyncs() {
    let Some(env) = Env::new().await else { return };
    env.write("a.jpg", b"alpha");
    env.write("nested/b.jpg", b"bravo");
    let large = big(3 * 1024 * 1024 + 123, 7);
    env.write("nested/large.bin", &large);

    let (summary, _) = env.run(env.options()).await;
    assert_eq!((summary.uploaded, summary.failed), (3, 0));
    assert_eq!(summary.listed_objects, Some(0), "first sync lists the prefix once");
    assert_eq!(env.object("data/nested/large.bin").await.unwrap().1, large);
    assert_eq!(env.object("data/a.jpg").await.unwrap().1, b"alpha");

    // Nothing changed: no hashing, no listing, no uploads.
    let (summary, _) = env.run(env.options()).await;
    assert_eq!((summary.unchanged, summary.uploaded, summary.listed_objects), (3, 0, None));

    // Only the timestamp changed: hashed, recognised as identical, not uploaded.
    set_mtime(&env.src().join("a.jpg"), 3600);
    let (summary, _) = env.run(env.options()).await;
    assert_eq!((summary.unchanged, summary.uploaded), (3, 0));

    // Content changed: uploaded again.
    env.write("nested/b.jpg", b"bravo v2");
    let (summary, events) = env.run(env.options()).await;
    assert_eq!(summary.uploaded, 1);
    assert!(events.iter().any(|e| matches!(e, Event::FileUploaded { path, .. } if path == "nested/b.jpg")));
    assert_eq!(env.object("data/nested/b.jpg").await.unwrap().1, b"bravo v2");
}

#[tokio::test]
async fn first_sync_skips_files_already_in_the_bucket() {
    let Some(env) = Env::new().await else { return };
    env.write("same.jpg", b"same");
    env.write("different.jpg", b"local version");
    // Simulate an earlier gcloud rsync that this manifest knows nothing about.
    let other = Arc::new(Manifest::open(&env.dir.path().join("other.db")).unwrap());
    env.run_with_manifest(env.options(), other).await;
    env.write("different.jpg", b"newer local version");

    let (summary, _) = env.run(env.options()).await;
    assert_eq!((summary.already_in_cloud, summary.uploaded), (1, 1));
    assert_eq!(env.object("data/different.jpg").await.unwrap().1, b"newer local version");
}

#[tokio::test]
async fn no_clobber_keeps_existing_objects() {
    let Some(env) = Env::new().await else { return };
    env.write("keep.jpg", b"cloud copy");
    env.run(env.options()).await;

    env.write("keep.jpg", b"local edit");
    let mut opts = env.options();
    opts.no_clobber = true;
    let (summary, _) = env.run(opts).await;
    assert_eq!((summary.skipped_exists, summary.uploaded), (1, 0));
    assert_eq!(env.object("data/keep.jpg").await.unwrap().1, b"cloud copy");
}

#[tokio::test]
async fn dry_run_uploads_nothing() {
    let Some(env) = Env::new().await else { return };
    env.write("a.jpg", b"alpha");
    let mut opts = env.options();
    opts.dry_run = true;
    let (summary, _) = env.run(opts).await;
    assert_eq!((summary.would_upload, summary.uploaded), (1, 0));
    assert!(env.object("data/a.jpg").await.is_none());
    // A dry run doesn't record anything, so the real sync still uploads.
    let (summary, _) = env.run(env.options()).await;
    assert_eq!(summary.uploaded, 1);
}

#[tokio::test]
async fn verify_reuploads_objects_removed_from_the_bucket() {
    let Some(env) = Env::new().await else { return };
    env.write("a.jpg", b"alpha");
    env.write("b.jpg", b"bravo");
    env.run(env.options()).await;
    env.delete("data/b.jpg").await;

    // The manifest alone thinks everything is synced...
    let (summary, _) = env.run(env.options()).await;
    assert_eq!(summary.uploaded, 0);
    // ...a verify pass compares against the bucket and repairs it.
    let mut opts = env.options();
    opts.verify = true;
    let (summary, _) = env.run(opts).await;
    assert_eq!((summary.unchanged, summary.uploaded), (1, 1));
    assert_eq!(env.object("data/b.jpg").await.unwrap().1, b"bravo");
}

#[tokio::test]
async fn large_files_upload_in_chunks_and_partial_objects_are_never_accepted() {
    let Some(env) = Env::new().await else { return };
    let data = big(4 * CHUNK_ALIGN + 1000, 3);
    env.write("video.bin", &data);
    let meta = std::fs::metadata(env.src().join("video.bin")).unwrap();
    let record = FileRecord {
        size: data.len() as u64,
        mtime_ns: meta.modified().unwrap().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos() as i64,
        crc32c: crc32c::crc32c(&data),
    };

    // A previous run sent two chunks and was killed. fake-gcs-server (unlike Cloud Storage)
    // finalizes a truncated object when asked for the upload status; the engine must reject
    // it (size/crc32c mismatch) and upload the whole file again rather than keep it.
    let session = env.client.start_resumable("bucket", "data/video.bin", record.size, record.crc32c, false).await.unwrap();
    env.manifest.save_session("gs://bucket/data/", "video.bin", &record, &session).unwrap();
    match env.client.put_chunk(&session, 0, data[..2 * CHUNK_ALIGN].to_vec(), record.size, record.crc32c).await.unwrap() {
        ResumableStatus::Incomplete { offset } => assert_eq!(offset, 2 * CHUNK_ALIGN as u64),
        ResumableStatus::Complete(_) => panic!("upload should not be complete yet"),
    }

    let (summary, _) = env.run(env.options()).await;
    assert_eq!((summary.uploaded, summary.failed), (1, 0));
    assert_eq!(env.object("data/video.bin").await.unwrap().1, data);
    assert!(env.manifest.session("gs://bucket/data/", "video.bin").unwrap().is_none(), "session cleared");
}
