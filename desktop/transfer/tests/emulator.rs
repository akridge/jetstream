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

#[tokio::test]
async fn browse_lists_one_level_and_totals_a_folder() {
    let Some(env) = Env::new().await else { return };
    env.write("readme.txt", b"hello");
    env.write("photos/a.jpg", &big(1000, 1));
    env.write("photos/raw/b.dng", &big(3000, 2));
    env.write("video/c.mp4", &big(5000, 3));
    env.run(env.options()).await;

    let root = jetstream_transfer::browse::list_folder(&env.client, "bucket", "data", None, 100, false).await.unwrap();
    assert_eq!(root.prefix, "data/");
    let names: Vec<(&str, bool)> = root.entries.iter().map(|e| (e.name.as_str(), e.is_folder)).collect();
    assert_eq!(names, [("photos/", true), ("video/", true), ("readme.txt", false)]);
    assert_eq!(root.entries[2].size, Some(5));

    let photos = jetstream_transfer::browse::list_folder(&env.client, "bucket", "data/photos/", None, 100, false).await.unwrap();
    let names: Vec<&str> = photos.entries.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, ["raw/", "a.jpg"]);

    // Paging with tiny pages still visits every entry exactly once.
    let mut seen = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let page = jetstream_transfer::browse::list_folder(&env.client, "bucket", "data/", token.as_deref(), 1, false).await.unwrap();
        seen.extend(page.entries.into_iter().map(|e| e.name));
        match page.next_page_token {
            Some(next) => token = Some(next),
            None => break,
        }
    }
    seen.sort();
    assert_eq!(seen, ["photos/", "readme.txt", "video/"]);

    let totals = jetstream_transfer::browse::folder_totals(&env.client, "bucket", "data/photos", 1_000).await.unwrap();
    assert_eq!((totals.objects, totals.bytes, totals.truncated), (2, 4000, false));
    // The limit is checked per page of up to 1000 objects, so a small folder is counted fully.
    let small_limit = jetstream_transfer::browse::folder_totals(&env.client, "bucket", "data", 1).await.unwrap();
    assert_eq!((small_limit.objects, small_limit.truncated), (4, false));
}

#[tokio::test]
async fn audit_finds_junk_objects_under_a_prefix() {
    let Some(env) = Env::new().await else { return };
    env.write("photos/a.jpg", &big(1000, 1));
    env.write("photos/Thumbs.db", &big(50, 2));
    env.write("photos/b.jpg.gstmp", &big(700, 3));
    env.write("nas/@eaDir/a.jpg", &big(30, 4));
    env.write("docs/~$draft.docx", &big(10, 5));
    env.run(env.options()).await;

    let patterns: Vec<String> = jetstream_transfer::audit::DEFAULT_PATTERNS.iter().map(|s| s.to_string()).collect();
    let progress = jetstream_transfer::audit::AuditProgress::default();
    let report = jetstream_transfer::audit::audit_bucket(&env.client, "bucket", "data", &patterns, 0, 100, &progress)
        .await
        .unwrap();
    assert_eq!((report.scanned_objects, report.junk_objects), (5, 4));
    assert_eq!(report.junk_bytes, 50 + 700 + 30 + 10);
    assert_eq!(report.findings[0].object_name, "data/photos/b.jpg.gstmp", "largest first");
    assert_eq!(report.by_pattern[0].pattern, r"\.gstmp$");
    assert_eq!(progress.scanned.load(std::sync::atomic::Ordering::Relaxed), 5);

    let limited = jetstream_transfer::audit::audit_bucket(&env.client, "bucket", "data", &patterns, 0, 1, &progress).await.unwrap();
    assert_eq!((limited.findings.len(), limited.findings_truncated, limited.junk_objects), (1, true, 4));
}

// ----- file manager operations ------------------------------------------------------------

mod files {
    use super::*;
    use jetstream_transfer::fileops::{self, Item, OpProgress};

    async fn put(env: &Env, name: &str, data: &[u8]) {
        env.client.upload_multipart("bucket", name, data.to_vec(), crc32c::crc32c(data), false).await.unwrap();
    }

    async fn names(env: &Env, prefix: &str) -> Vec<String> {
        let mut names: Vec<String> = env.client.list("bucket", prefix).await.unwrap().into_iter().map(|o| o.name).collect();
        names.sort();
        names
    }

    fn items(paths: &[&str]) -> Vec<Item> {
        paths.iter().map(|p| Item { path: p.to_string() }).collect()
    }

    #[tokio::test]
    async fn rename_folder_moves_everything_keeps_metadata_and_undoes() {
        let Some(env) = Env::new().await else { return };
        put(&env, "data/photos/", b"").await; // placeholder object, as the Console creates
        put(&env, "data/photos/a.jpg", &big(3000, 1)).await;
        put(&env, "data/photos/sub/b.jpg", &big(500, 2)).await;
        put(&env, "data/photos-raw/c.jpg", &big(10, 3)).await; // same prefix text, different folder
        let patch = format!("{}/storage/v1/b/bucket/o/{}", env.emulator.base, jetstream_transfer::gcs::encode("data/photos/a.jpg"));
        env.http.patch(patch).json(&serde_json::json!({"contentType": "image/jpeg", "metadata": {"survey": "kona"}})).send().await.unwrap();

        let bucket = env.client.get_bucket("bucket").await.unwrap();
        let plan = fileops::plan_transfer(&env.client, &bucket, &items(&["data/photos/"]), "bucket", "data/", Some("pics"), true)
            .await
            .unwrap();
        assert_eq!((plan.summary.objects, plan.summary.bytes, plan.summary.conflicts), (3, 3500, 0));
        assert_eq!(plan.summary.example.as_deref(), Some("gs://bucket/data/pics/"));

        let result = fileops::run(&env.client, &plan, false, &OpProgress::default()).await;
        assert_eq!((result.done, result.failed, result.skipped), (3, 0, 0), "{:?}", result.failures);
        assert_eq!(
            names(&env, "data/").await,
            ["data/photos-raw/c.jpg", "data/pics/", "data/pics/a.jpg", "data/pics/sub/b.jpg"]
        );
        assert_eq!(env.object("data/pics/a.jpg").await.unwrap().1, big(3000, 1));
        let moved = env.client.get_object_raw("bucket", "data/pics/a.jpg").await.unwrap().unwrap();
        assert_eq!(moved["contentType"], "image/jpeg");
        assert_eq!(moved["metadata"]["survey"], "kona");

        assert!(result.undoable);
        let undo = fileops::plan_undo(result.undo.as_ref().unwrap());
        let undone = fileops::run(&env.client, &undo, false, &OpProgress::default()).await;
        assert_eq!((undone.done, undone.failed), (3, 0), "{:?}", undone.failures);
        assert_eq!(
            names(&env, "data/").await,
            ["data/photos-raw/c.jpg", "data/photos/", "data/photos/a.jpg", "data/photos/sub/b.jpg"]
        );
    }

    #[tokio::test]
    async fn move_leaves_conflicts_alone_and_finishes_interrupted_moves() {
        let Some(env) = Env::new().await else { return };
        put(&env, "in/x.csv", b"new x").await;
        put(&env, "in/y.csv", b"same y").await;
        put(&env, "in/z.csv", b"z").await;
        put(&env, "out/in/x.csv", b"different x").await; // a real clash
        put(&env, "out/in/y.csv", b"same y").await; // copied by a move that was interrupted

        let bucket = env.client.get_bucket("bucket").await.unwrap();
        let plan = fileops::plan_transfer(&env.client, &bucket, &items(&["in/"]), "bucket", "out/", None, true).await.unwrap();
        assert_eq!((plan.summary.objects, plan.summary.conflicts, plan.summary.already_copied), (3, 1, 1));
        assert_eq!(plan.summary.conflict_examples, ["out/in/x.csv"]);

        let result = fileops::run(&env.client, &plan, false, &OpProgress::default()).await;
        assert_eq!((result.done, result.skipped, result.failed), (2, 1, 0));
        assert_eq!(names(&env, "in/").await, ["in/x.csv"], "the clashing file stays where it was");
        assert_eq!(env.object("out/in/x.csv").await.unwrap().1, b"different x", "never overwritten");
        assert_eq!(env.object("out/in/z.csv").await.unwrap().1, b"z");
    }

    #[tokio::test]
    async fn copy_rename_file_delete_and_new_folder() {
        let Some(env) = Env::new().await else { return };
        put(&env, "docs/report.pdf", &big(800, 4)).await;
        let bucket = env.client.get_bucket("bucket").await.unwrap();

        // Copy keeps the original.
        let copy = fileops::plan_transfer(&env.client, &bucket, &items(&["docs/report.pdf"]), "bucket", "archive/", None, false)
            .await
            .unwrap();
        let result = fileops::run(&env.client, &copy, false, &OpProgress::default()).await;
        assert_eq!((result.done, result.undoable), (1, false));

        // Rename a file in place.
        let rename = fileops::plan_transfer(&env.client, &bucket, &items(&["docs/report.pdf"]), "bucket", "docs/", Some("final.pdf"), true)
            .await
            .unwrap();
        fileops::run(&env.client, &rename, false, &OpProgress::default()).await;
        assert_eq!(names(&env, "").await, ["archive/report.pdf", "docs/final.pdf"]);

        // Guard rails.
        let into_itself = fileops::plan_transfer(&env.client, &bucket, &items(&["docs/"]), "bucket", "docs/inner/", None, true).await;
        assert!(into_itself.err().unwrap().to_string().contains("inside itself"));
        put(&env, "docs/old/a.txt", b"a").await;
        put(&env, "docs/new/b.txt", b"b").await;
        let onto_existing = fileops::plan_transfer(&env.client, &bucket, &items(&["docs/old/"]), "bucket", "docs/", Some("new"), true).await;
        assert!(onto_existing.err().unwrap().to_string().contains("already exists"), "renaming never merges folders");
        let bad_name = fileops::plan_transfer(&env.client, &bucket, &items(&["docs/"]), "bucket", "", Some("a/b"), true).await;
        assert!(bad_name.is_err());

        // New folder, then a duplicate.
        assert_eq!(fileops::new_folder(&env.client, &bucket, "docs", "2026").await.unwrap(), "docs/2026/");
        assert!(fileops::new_folder(&env.client, &bucket, "docs/", "2026").await.is_err());
        let listing = jetstream_transfer::browse::list_folder(&env.client, "bucket", "docs/", None, 100, false).await.unwrap();
        assert!(listing.entries.iter().any(|e| e.is_folder && e.name == "2026/"));

        // Delete a folder: everything under it, including the placeholder.
        let delete = fileops::plan_delete(&env.client, &bucket, &items(&["docs/"])).await.unwrap();
        assert_eq!(delete.summary.objects, 4);
        let deleted = fileops::run(&env.client, &delete, false, &OpProgress::default()).await;
        assert_eq!((deleted.done, deleted.undoable), (4, false), "no soft delete, so no undo");
        assert_eq!(names(&env, "").await, ["archive/report.pdf"]);
    }
}

#[tokio::test]
async fn compare_shows_what_is_synced_local_only_cloud_only_and_changed() {
    let Some(env) = Env::new().await else { return };
    env.write("photos/a.jpg", &big(1000, 1));
    env.write("photos/b.jpg", &big(2000, 2));
    env.write("docs/c.pdf", &big(300, 3));
    env.write("top.txt", &big(10, 4));
    env.run(env.options()).await;

    env.write("photos/new.jpg", &big(500, 5)); // not uploaded yet
    env.write("docs/c.pdf", &big(301, 6)); // changed locally
    env.write("skip.tmp", &big(5, 7)); // filtered out
    env.delete("data/top.txt").await; // removed from the cloud
    env.client.upload_multipart("bucket", "data/cloud_only.csv", b"x,y".to_vec(), crc32c::crc32c(b"x,y"), false).await.unwrap();
    env.client.upload_multipart("bucket", "data/empty_folder/", Vec::new(), 0, false).await.unwrap();

    let excludes = jetstream_transfer::compare::exclude_regexes(&["*.tmp".into()], &[]).unwrap();
    let report = jetstream_transfer::compare::compare(&env.client, &env.src(), "gs://bucket/data", &excludes, true, Some(&env.manifest))
        .await
        .unwrap();
    let paths = |g: &jetstream_transfer::compare::Group| g.list.iter().map(|d| d.path.clone()).collect::<Vec<_>>();
    assert_eq!(paths(&report.same), ["photos/b.jpg", "photos/a.jpg"], "largest first");
    assert_eq!(report.verified, 2, "both confirmed by the manifest's checksums");
    assert_eq!(paths(&report.different), ["docs/c.pdf"]);
    assert_eq!(paths(&report.local_only), ["photos/new.jpg", "top.txt"]);
    assert_eq!(paths(&report.cloud_only), ["cloud_only.csv"], "folder placeholders aren't files");
    assert_eq!((report.local_files, report.excluded, report.cloud_files), (5, 1, 4));
    let photos = report.folders.iter().find(|f| f.folder == "photos").unwrap();
    assert_eq!((photos.same, photos.local_only), (2, 1));
    let top = report.folders.iter().find(|f| f.folder.is_empty()).unwrap();
    assert_eq!((top.local_only, top.cloud_only), (1, 1));
}
