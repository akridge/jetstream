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
    // A dry run records only what it confirmed is in the cloud, so the real sync still uploads.
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

#[tokio::test]
async fn download_folder_and_files_then_upload_files_back() {
    use jetstream_transfer::fileops::{Item, OpProgress};
    use jetstream_transfer::localops;
    use jetstream_transfer::throttle::Throttle;
    let Some(env) = Env::new().await else { return };
    let put = |name: &'static str, data: Vec<u8>| {
        let client = env.client.clone();
        async move { client.upload_multipart("bucket", name, data.clone(), crc32c::crc32c(&data), false).await.unwrap() }
    };
    put("survey/photos/", Vec::new()).await;
    put("survey/photos/a.jpg", big(3000, 1)).await;
    put("survey/photos/deep/b.jpg", big(9_500_000, 2)).await;
    put("survey/notes.txt", b"hello".to_vec()).await;
    put("survey/photos/../escape.txt", b"no".to_vec()).await;

    let dest = env.dir.path().join("downloads");
    std::fs::create_dir_all(&dest).unwrap();
    std::fs::write(dest.join("notes.txt"), b"mine, different").unwrap();
    let items = [Item { path: "survey/photos/".into() }, Item { path: "survey/notes.txt".into() }];
    let progress = OpProgress::default();
    let result = localops::download(&env.client, "bucket", &items, &dest, &Throttle::default(), &progress).await.unwrap();

    assert_eq!(std::fs::read(dest.join("photos/a.jpg")).unwrap(), big(3000, 1));
    assert_eq!(std::fs::read(dest.join("photos/deep/b.jpg")).unwrap(), big(9_500_000, 2));
    assert_eq!(std::fs::read(dest.join("notes.txt")).unwrap(), b"mine, different", "local files are never replaced");
    assert!(!dest.join("escape.txt").exists() && !env.dir.path().join("escape.txt").exists());
    assert_eq!((result.done, result.skipped, result.failed), (2, 0, 2), "{:?}", result.failures);
    assert!(result.failures.iter().any(|f| f.error.contains("unsafe")));
    assert!(std::fs::read_dir(dest.join("photos")).unwrap().all(|e| !e.unwrap().file_name().to_string_lossy().ends_with(".jetstream-part")));

    // Downloading again skips what's already there.
    let again = localops::download(&env.client, "bucket", &items[..1], &dest, &Throttle::default(), &OpProgress::default()).await.unwrap();
    assert_eq!((again.done, again.skipped), (0, 2));

    // Upload files (one small, one resumable) and never overwrite.
    let files = vec![dest.join("photos/a.jpg"), dest.join("photos/deep/b.jpg")];
    let up = localops::upload_files(&env.client, &files, "bucket", "incoming", &Throttle::default(), &OpProgress::default()).await.unwrap();
    assert_eq!((up.done, up.failed), (2, 0), "{:?}", up.failures);
    assert_eq!(env.object("incoming/b.jpg").await.unwrap().1, big(9_500_000, 2));
    let twice = localops::upload_files(&env.client, &files, "bucket", "incoming/", &Throttle::default(), &OpProgress::default()).await.unwrap();
    assert_eq!((twice.done, twice.skipped), (0, 2));
}

// ----- progress across stop/start, and speed -------------------------------------------

mod resume_and_speed {
    use super::*;
    use jetstream_transfer::gcs::UploadExtras;

    fn manifest_at(env: &Env, name: &str) -> Arc<Manifest> {
        Arc::new(Manifest::open(&env.dir.path().join(name)).unwrap())
    }

    #[tokio::test]
    async fn a_restart_with_a_partial_manifest_uploads_nothing_twice_and_reads_nothing() {
        let Some(env) = Env::new().await else { return };
        for i in 0..6 {
            env.write(&format!("photos/{i}.jpg"), &big(3000 + i, i as u8));
        }
        env.write("top.txt", b"hello");
        let (first, _) = env.run(env.options()).await;
        assert_eq!(first.uploaded, 7);

        // A stopped sync: the manifest knows only one file (e.g. the engine was killed).
        let partial = manifest_at(&env, "partial.db");
        partial.record("gs://bucket/data/", "top.txt", &FileRecord { size: 5, mtime_ns: mtime_ns(&env.src().join("top.txt")), crc32c: crc32c::crc32c(b"hello") }, None).unwrap();
        let (again, events) = env.run_with_manifest(env.options(), partial).await;
        assert_eq!((again.uploaded, again.unchanged), (0, 1), "{again:?}");
        assert_eq!((again.already_in_cloud, again.matched_by_time, again.hashed_files), (6, 6, 0), "recognised by size + time");
        assert!(again.listed_objects.is_some());
        let phases: Vec<&str> = events.iter().filter_map(|e| match e { Event::Phase { phase } => Some(*phase), _ => None }).collect();
        assert_eq!(phases, ["scanning", "listing", "uploading"]);
        let plans: Vec<(bool, u64)> = events
            .iter()
            .filter_map(|e| match e { Event::Plan { estimate, unchanged, .. } => Some((*estimate, *unchanged)), _ => None })
            .collect();
        assert_eq!(plans, [(true, 1), (false, 7)], "early estimate, then refined after listing");
    }

    #[tokio::test]
    async fn gcloud_file_times_avoid_reading_files_and_old_uploads_are_hashed() {
        let Some(env) = Env::new().await else { return };
        env.write("a/with_time.bin", &big(5000, 1));
        env.write("a/no_time.bin", &big(6000, 2));
        // Uploaded earlier by another tool: one with gcloud's file-time metadata, one without.
        let with = big(5000, 1);
        let extras = UploadExtras { mtime_secs: Some(mtime_ns(&env.src().join("a/with_time.bin")) / 1_000_000_000), live: None, ..Default::default() };
        env.client.upload_multipart_with("bucket", "data/a/with_time.bin", with.clone(), crc32c::crc32c(&with), false, &extras).await.unwrap();
        let without = big(6000, 2);
        env.client.upload_multipart("bucket", "data/a/no_time.bin", without.clone(), crc32c::crc32c(&without), false).await.unwrap();

        let (summary, _) = env.run(env.options()).await;
        assert_eq!((summary.uploaded, summary.already_in_cloud, summary.matched_by_time), (0, 2, 1));
        assert_eq!(summary.hashed_files, 1, "only the object without a file time is read");

        // The engine records file times too, so gcloud (and a fresh manifest) recognise its uploads.
        env.write("b/new.bin", &big(700, 3));
        env.run(env.options()).await;
        let raw = env.client.get_object_raw("bucket", "data/b/new.bin").await.unwrap().unwrap();
        assert_eq!(
            raw["metadata"]["goog-reserved-file-mtime"].as_str().unwrap(),
            (mtime_ns(&env.src().join("b/new.bin")) / 1_000_000_000).to_string()
        );
    }

    #[tokio::test]
    async fn a_subfolder_sync_reuses_what_the_whole_folder_sync_recorded() {
        let Some(env) = Env::new().await else { return };
        env.write("photos/a.jpg", &big(100, 1));
        env.write("photos/b.jpg", &big(200, 2));
        env.write("video/v.mp4", &big(300, 3));
        env.run(env.options()).await;

        let mut sub = SyncOptions::new(env.src().join("photos"), "gs://bucket/data/photos").unwrap();
        sub.retry_base = Duration::from_millis(20);
        let (summary, _) = env.run(sub).await;
        assert_eq!((summary.unchanged, summary.uploaded, summary.listed_objects), (2, 0, None), "no listing, no reading");
    }

    #[tokio::test]
    async fn stopping_keeps_what_finished_and_the_next_run_continues() {
        let Some(env) = Env::new().await else { return };
        for i in 0..5 {
            env.write(&format!("f{i}.bin"), &big(1000, i as u8));
        }
        let mut opts = env.options();
        opts.concurrency = 1;
        opts.hash_concurrency = 1;
        let cancel = Arc::new(AtomicBool::new(false));
        let stop = cancel.clone();
        let seen = Arc::new(Mutex::new(0));
        let counter = seen.clone();
        let summary = sync(
            env.client.clone(),
            env.manifest.clone(),
            opts,
            "test",
            Arc::new(move |e| {
                if matches!(e, Event::FileUploaded { .. }) {
                    *counter.lock().unwrap() += 1;
                    stop.store(true, std::sync::atomic::Ordering::SeqCst); // "stop" after the first file
                }
            }),
            cancel,
        )
        .await
        .unwrap();
        assert!(summary.cancelled);
        let first = summary.uploaded;
        assert!((1..5).contains(&first), "{first}");
        assert_eq!(env.manifest.count("gs://bucket/data/").unwrap(), first);

        let (rest, _) = env.run(env.options()).await;
        assert_eq!((rest.unchanged, rest.uploaded), (first, 5 - first));
    }

    fn mtime_ns(path: &Path) -> i64 {
        std::fs::metadata(path).unwrap().modified().unwrap().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos() as i64
    }
}

#[tokio::test]
async fn receipt_verifies_every_file_and_learns_checksums() {
    use jetstream_transfer::fileops::OpProgress;
    use jetstream_transfer::receipt::{make_receipt, to_csv, write_receipt, FileStatus};
    let Some(env) = Env::new().await else { return };
    env.write("photos/a.jpg", &big(3000, 1));
    env.write("photos/b.jpg", &big(4000, 2));
    env.write("notes, final.txt", b"hello");
    env.run(env.options()).await;

    let receipt = make_receipt(&env.client, &env.src(), "gs://bucket/data", &[], true, Some(&env.manifest), "tester", &OpProgress::default())
        .await
        .unwrap();
    assert_eq!(receipt.result, "complete");
    assert_eq!((receipt.totals.verified, receipt.totals.files_read), (3, 0), "checksums come from the sync's manifest");
    let a = receipt.files.iter().find(|f| f.path == "photos/a.jpg").unwrap();
    assert_eq!(a.crc32c.as_deref(), Some(jetstream_transfer::gcs::crc32c_b64(crc32c::crc32c(&big(3000, 1))).as_str()));
    assert!(a.generation.is_some());

    // Change, delete and add things.
    env.write("photos/a.jpg", &big(3001, 9)); // changed locally
    env.delete("data/photos/b.jpg").await; // gone from the cloud
    env.client.upload_multipart("bucket", "data/extra.csv", b"x".to_vec(), crc32c::crc32c(b"x"), false).await.unwrap();
    env.client.upload_multipart("bucket", "data/_jetstream/receipts/old.json", b"{}".to_vec(), crc32c::crc32c(b"{}"), false).await.unwrap();
    let receipt = make_receipt(&env.client, &env.src(), "gs://bucket/data", &[], true, Some(&env.manifest), "tester", &OpProgress::default())
        .await
        .unwrap();
    let status = |p: &str| receipt.files.iter().find(|f| f.path == p).map(|f| f.status);
    assert_eq!(receipt.result, "incomplete");
    assert_eq!(status("photos/a.jpg"), Some(FileStatus::Mismatch));
    assert_eq!(status("photos/b.jpg"), Some(FileStatus::MissingInCloud));
    assert_eq!(status("extra.csv"), Some(FileStatus::CloudOnly));
    assert_eq!(status("notes, final.txt"), Some(FileStatus::Verified));
    assert!(status("_jetstream/receipts/old.json").is_none(), "receipts in the cloud aren't data");
    assert_eq!(receipt.totals.files_read, 1, "only the changed file was read");

    let csv = to_csv(&receipt);
    assert!(csv.starts_with("# JetStream upload receipt: incomplete\n"));
    assert!(csv.contains("\n\"notes, final.txt\",verified,5,"), "{csv}");
    let (csv_path, json_path) = write_receipt(&receipt, &env.dir.path().join("receipts")).unwrap();
    assert!(csv_path.exists() && json_path.exists());
    let json: serde_json::Value = serde_json::from_slice(&std::fs::read(json_path).unwrap()).unwrap();
    assert_eq!(json["totals"]["mismatch"], 1);
}

/// Pause, stop and a dropped internet connection, through a proxy that can be unplugged.
mod pause_and_reconnect {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    use jetstream_transfer::throttle::Throttle;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// Forwards to the emulator; `unplug` drops every connection and refuses new ones, like a
    /// lost internet connection. Counts the bytes sent upstream.
    pub(super) struct Proxy {
        pub(super) base: String,
        down: Arc<AtomicBool>,
        pub(super) sent: Arc<AtomicU64>,
        /// Bytes sent back by the server (downloads).
        pub(super) received: Arc<AtomicU64>,
        conns: Arc<Mutex<Vec<tokio::task::JoinHandle<()>>>>,
    }

    impl Proxy {
        pub(super) async fn start(upstream: &str) -> Proxy {
            let upstream = upstream.trim_start_matches("http://").to_string();
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let base = format!("http://{}", listener.local_addr().unwrap());
            let (down, sent, conns) = (Arc::new(AtomicBool::new(false)), Arc::new(AtomicU64::new(0)), Arc::new(Mutex::new(Vec::new())));
            let received = Arc::new(AtomicU64::new(0));
            let (d, s, c, r) = (down.clone(), sent.clone(), conns.clone(), received.clone());
            tokio::spawn(async move {
                loop {
                    let (client, _) = listener.accept().await.unwrap();
                    if d.load(Ordering::Relaxed) {
                        drop(client);
                        continue;
                    }
                    let (upstream, sent, received) = (upstream.clone(), s.clone(), r.clone());
                    let handle = tokio::spawn(async move {
                        let Ok(server) = tokio::net::TcpStream::connect(&upstream).await else { return };
                        let (mut cr, mut cw) = client.into_split();
                        let (mut sr, mut sw) = server.into_split();
                        let up = async {
                            let mut buf = vec![0u8; 64 * 1024];
                            loop {
                                let n = match cr.read(&mut buf).await { Ok(0) | Err(_) => break, Ok(n) => n };
                                sent.fetch_add(n as u64, Ordering::Relaxed);
                                if sw.write_all(&buf[..n]).await.is_err() { break; }
                            }
                            let _ = sw.shutdown().await;
                        };
                        let downstream = async {
                            let mut buf = vec![0u8; 64 * 1024];
                            loop {
                                let n = match sr.read(&mut buf).await { Ok(0) | Err(_) => break, Ok(n) => n };
                                received.fetch_add(n as u64, Ordering::Relaxed);
                                if cw.write_all(&buf[..n]).await.is_err() { break; }
                            }
                            let _ = cw.shutdown().await;
                        };
                        tokio::join!(up, downstream);
                    });
                    c.lock().unwrap().push(handle);
                }
            });
            Proxy { base, down, sent, received, conns }
        }

        fn unplug(&self) {
            self.down.store(true, Ordering::Relaxed);
            for handle in self.conns.lock().unwrap().drain(..) {
                handle.abort();
            }
        }

        fn plug(&self) {
            self.down.store(false, Ordering::Relaxed);
        }
    }

    fn spawn_sync(env: &Env, client: GcsClient, opts: SyncOptions, cancel: Arc<AtomicBool>) -> (tokio::task::JoinHandle<Summary>, Arc<Mutex<Vec<Event>>>) {
        let events = Arc::new(Mutex::new(Vec::new()));
        let sink = events.clone();
        let manifest = env.manifest.clone();
        let handle = tokio::spawn(async move {
            sync(client, manifest, opts, "test", Arc::new(move |e| sink.lock().unwrap().push(e)), cancel).await.unwrap()
        });
        (handle, events)
    }

    #[tokio::test]
    async fn a_dropped_connection_waits_and_carries_on_without_failing_files() {
        let Some(env) = Env::new().await else { return };
        let proxy = Proxy::start(&env.emulator.base).await;
        let http = GcsClient::http_client();
        let client = GcsClient::with_base(http.clone(), &proxy.base, Arc::new(TokenProvider::none(http)));
        for i in 0..3 {
            env.write(&format!("big/{i}.bin"), &big(3 * 1024 * 1024, i as u8));
        }
        for i in 0..20 {
            env.write(&format!("small/{i}.txt"), &big(40_000, i as u8));
        }
        let mut opts = env.options();
        opts.throttle = Arc::new(Throttle::new(4_000_000)); // ~3 s of uploading
        let (handle, events) = spawn_sync(&env, client, opts, Arc::new(AtomicBool::new(false)));
        tokio::time::sleep(Duration::from_millis(800)).await;
        proxy.unplug();
        tokio::time::sleep(Duration::from_secs(5)).await; // longer than all the retries together
        proxy.plug();
        let summary = tokio::time::timeout(Duration::from_secs(60), handle).await.expect("finished").unwrap();
        assert_eq!((summary.uploaded, summary.failed), (23, 0), "{summary:?}");
        {
            let events = events.lock().unwrap();
            assert!(events.iter().any(|e| matches!(e, Event::Offline { .. })), "reported offline");
            assert!(events.iter().any(|e| matches!(e, Event::Online { offline_seconds } if *offline_seconds >= 3)), "and back online");
        }
        for i in 0..3 {
            assert_eq!(env.object(&format!("data/big/{i}.bin")).await.unwrap().1, big(3 * 1024 * 1024, i as u8));
        }
    }

    #[tokio::test]
    async fn pause_stops_sending_within_a_second_and_resume_finishes() {
        let Some(env) = Env::new().await else { return };
        let proxy = Proxy::start(&env.emulator.base).await;
        let http = GcsClient::http_client();
        let client = GcsClient::with_base(http.clone(), &proxy.base, Arc::new(TokenProvider::none(http)));
        let data = big(8 * 1024 * 1024, 9);
        env.write("video.bin", &data);
        let mut opts = env.options();
        opts.chunk_size = 8 * 1024 * 1024; // one long request: pause must interrupt it
        let throttle = Arc::new(Throttle::new(2_000_000));
        opts.throttle = throttle.clone();
        let (handle, events) = spawn_sync(&env, client, opts, Arc::new(AtomicBool::new(false)));
        tokio::time::sleep(Duration::from_millis(1200)).await;
        throttle.set_paused(true);
        tokio::time::sleep(Duration::from_millis(1000)).await;
        let at_pause = proxy.sent.load(Ordering::Relaxed);
        tokio::time::sleep(Duration::from_millis(2000)).await;
        assert!(proxy.sent.load(Ordering::Relaxed) - at_pause < 64 * 1024, "nothing more sent while paused");
        throttle.set_paused(false);
        let summary = tokio::time::timeout(Duration::from_secs(60), handle).await.expect("finished").unwrap();
        assert_eq!((summary.uploaded, summary.failed), (1, 0), "{summary:?}");
        assert_eq!(env.object("data/video.bin").await.unwrap().1, data);
        assert!(!events.lock().unwrap().iter().any(|e| matches!(e, Event::FileRetrying { .. })), "a pause isn't a failure");
    }

    #[tokio::test]
    async fn parallel_parts_wait_out_a_dropped_connection_and_a_pause() {
        let Some(env) = Env::new().await else { return };
        let proxy = Proxy::start(&env.emulator.base).await;
        let http = GcsClient::http_client();
        let client = GcsClient::with_base(http.clone(), &proxy.base, Arc::new(TokenProvider::none(http)));
        let data = big(12 * 1024 * 1024, 5);
        env.write("big.bin", &data);
        let mut opts = env.options();
        opts.composite_threshold = 4 * 1024 * 1024;
        opts.composite_part_size = 1024 * 1024;
        let throttle = Arc::new(Throttle::new(3_000_000)); // ~4 s
        opts.throttle = throttle.clone();
        let (handle, events) = spawn_sync(&env, client, opts, Arc::new(AtomicBool::new(false)));
        tokio::time::sleep(Duration::from_millis(900)).await;
        proxy.unplug();
        tokio::time::sleep(Duration::from_secs(3)).await;
        proxy.plug();
        tokio::time::sleep(Duration::from_millis(800)).await;
        throttle.set_paused(true);
        tokio::time::sleep(Duration::from_millis(1000)).await;
        let at_pause = proxy.sent.load(Ordering::Relaxed);
        tokio::time::sleep(Duration::from_millis(1500)).await;
        assert!(proxy.sent.load(Ordering::Relaxed) - at_pause < 64 * 1024, "no part sends while paused");
        throttle.set_paused(false);
        let summary = tokio::time::timeout(Duration::from_secs(60), handle).await.expect("finished").unwrap();
        assert_eq!((summary.uploaded, summary.failed, summary.composite_files, summary.parts_left), (1, 0, 1, 0), "{summary:?}");
        assert!(events.lock().unwrap().iter().any(|e| matches!(e, Event::Offline { .. })));
        assert_eq!(env.object("data/big.bin").await.unwrap().1, data);
    }

    #[tokio::test]
    async fn stop_ends_a_long_request_at_once() {
        let Some(env) = Env::new().await else { return };
        env.write("video.bin", &big(8 * 1024 * 1024, 3));
        let mut opts = env.options();
        opts.chunk_size = 8 * 1024 * 1024;
        opts.throttle = Arc::new(Throttle::new(1_000_000)); // the chunk alone would take 8 s
        let cancel = Arc::new(AtomicBool::new(false));
        let (handle, _) = spawn_sync(&env, env.client.clone(), opts, cancel.clone());
        tokio::time::sleep(Duration::from_millis(1000)).await;
        let stopped = Instant::now();
        cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        let summary = tokio::time::timeout(Duration::from_secs(10), handle).await.expect("finished").unwrap();
        assert!(stopped.elapsed() < Duration::from_secs(2), "stopped after {:?}", stopped.elapsed());
        assert_eq!(summary.uploaded, 0);
    }
}

#[tokio::test]
async fn a_preview_reads_only_what_it_must_and_the_upload_after_it_reads_nothing_twice() {
    let Some(env) = Env::new().await else { return };
    let same = big(50_000, 1);
    env.write("same.bin", &same); // already in the cloud (uploaded by another tool)
    env.write("new.bin", &big(60_000, 2)); // not in the cloud
    env.write("changed.bin", &big(70_000, 3)); // in the cloud with another size
    env.client.upload_multipart("bucket", "data/same.bin", same.clone(), crc32c::crc32c(&same), false).await.unwrap();
    env.client.upload_multipart("bucket", "data/changed.bin", big(10, 9), crc32c::crc32c(&big(10, 9)), false).await.unwrap();

    let mut opts = env.options();
    opts.dry_run = true;
    let (preview, _) = env.run(opts).await;
    assert_eq!((preview.would_upload, preview.already_in_cloud), (2, 1), "{preview:?}");
    assert_eq!((preview.hashed_files, preview.hashed_bytes), (1, 50_000), "only the same-size file is read");

    let (sync, _) = env.run(env.options()).await;
    assert_eq!((sync.uploaded, sync.unchanged), (2, 1), "the preview recorded what it confirmed: {sync:?}");
    assert_eq!(sync.hashed_files, 2, "same.bin isn't read again");
}

/// Large files as parallel parts joined in the bucket.
mod composite {
    use super::*;

    const MIB: usize = 1024 * 1024;

    fn options(env: &Env) -> SyncOptions {
        let mut opts = env.options();
        opts.composite_threshold = 4 * MIB as u64;
        opts.composite_part_size = MIB as u64;
        opts
    }

    async fn names_under(env: &Env, prefix: &str) -> Vec<String> {
        env.client.list("bucket", prefix).await.unwrap().into_iter().map(|o| o.name).collect()
    }

    fn parts_dir(name: &str, data: &[u8]) -> String {
        format!("data/_jetstream/parts/{:08x}-{}-{:08x}/", crc32c::crc32c(name.as_bytes()), data.len(), crc32c::crc32c(data))
    }

    #[tokio::test]
    async fn a_big_file_goes_up_in_parallel_parts_and_is_joined_exactly() {
        let Some(env) = Env::new().await else { return };
        let data = big(6 * MIB + 12_345, 4);
        env.write("video/big.bin", &data);
        env.write("small.txt", b"small");
        let (summary, events) = env.run(options(&env)).await;
        assert_eq!((summary.uploaded, summary.failed, summary.composite_files, summary.parts_left), (2, 0, 1, 0), "{summary:?}");
        let parts = events.iter().find_map(|e| match e { Event::FileStarted { path, parts, .. } if path == "video/big.bin" => *parts, _ => None });
        assert_eq!(parts, Some(7));
        let (size, body) = env.object("data/video/big.bin").await.unwrap();
        assert_eq!((size, body == data), (data.len() as u64, true));
        let info = env.client.list("bucket", "data/video/").await.unwrap().remove(0);
        assert_eq!(info.crc32c.as_deref().and_then(jetstream_transfer::gcs::parse_crc32c_b64), Some(crc32c::crc32c(&data)));
        assert!(names_under(&env, "data/_jetstream/").await.is_empty(), "temporary parts removed");
        // Recorded like any upload: the next sync has nothing to do.
        let (again, _) = env.run(options(&env)).await;
        assert_eq!((again.uploaded, again.unchanged), (0, 2));
    }

    #[tokio::test]
    async fn parts_from_an_earlier_run_are_reused() {
        let Some(env) = Env::new().await else { return };
        let data = big(5 * MIB, 7);
        env.write("big.bin", &data);
        // A stopped run had finished the first three 1 MiB parts.
        let dir = parts_dir("data/big.bin", &data);
        for i in 0..3 {
            let part = data[i * MIB..(i + 1) * MIB].to_vec();
            env.client.upload_multipart("bucket", &format!("{dir}{i:02}"), part.clone(), crc32c::crc32c(&part), false).await.unwrap();
        }
        let (summary, _) = env.run(options(&env)).await;
        assert_eq!((summary.uploaded, summary.resumed, summary.composite_files), (1, 1, 1), "{summary:?}");
        assert_eq!(env.object("data/big.bin").await.unwrap().1, data);
        assert!(names_under(&env, "data/_jetstream/").await.is_empty());
    }

    #[tokio::test]
    async fn no_clobber_leaves_an_existing_object_and_cleans_up() {
        let Some(env) = Env::new().await else { return };
        env.write("big.bin", &big(5 * MIB, 1));
        let original = b"already here".to_vec();
        env.client.upload_multipart("bucket", "data/big.bin", original.clone(), crc32c::crc32c(&original), false).await.unwrap();
        let mut opts = options(&env);
        opts.no_clobber = true;
        let (summary, _) = env.run(opts).await;
        assert_eq!((summary.uploaded, summary.skipped_exists), (0, 1), "{summary:?}");
        assert_eq!(env.object("data/big.bin").await.unwrap().1, original);
        assert!(names_under(&env, "data/_jetstream/").await.is_empty());
    }

    #[tokio::test]
    async fn the_off_switch_uses_one_stream() {
        // (Which buckets can take parts is unit-tested: the emulator ignores storage classes.)
        let Some(env) = Env::new().await else { return };
        env.write("big.bin", &big(5 * MIB, 2));
        let mut opts = options(&env);
        opts.composite = false;
        let (summary, events) = env.run(opts).await;
        assert_eq!((summary.uploaded, summary.composite_files), (1, 0));
        assert!(!events.iter().any(|e| matches!(e, Event::CompositeOff { .. })), "switched off by the user: nothing to report");
    }
}


/// Thumbnails for the cloud browser: small range reads for camera JPEGs, capped downloads, a cache.
mod thumbnails {
    use super::pause_and_reconnect::Proxy;
    use super::*;
    use jetstream_transfer::thumbs::{self, test_images, ObjectRef, Skip, Source, ThumbCache, ThumbOptions};
    use std::sync::atomic::Ordering;

    async fn put(env: &Env, name: &str, data: Vec<u8>, content_type: &str) -> ObjectRef {
        let url = format!("{}/upload/storage/v1/b/bucket/o?uploadType=media&name={}", env.emulator.base, jetstream_transfer::gcs::encode(name));
        env.http.post(url).header("Content-Type", content_type).body(data.clone()).send().await.unwrap().error_for_status().unwrap();
        let info = env.client.get_object("bucket", name).await.unwrap().unwrap();
        ObjectRef { bucket: "bucket".into(), name: name.into(), generation: info.generation, size: info.size, content_type: info.content_type, storage_class: info.storage_class }
    }

    #[tokio::test]
    async fn camera_jpegs_use_their_embedded_preview_and_everything_is_cached() {
        let Some(env) = Env::new().await else { return };
        let proxy = Proxy::start(&env.emulator.base).await;
        let http = GcsClient::http_client();
        let client = GcsClient::with_base(http.clone(), &proxy.base, Arc::new(TokenProvider::none(http)));
        let dir = tempfile::tempdir().unwrap();
        let cache = ThumbCache::open(dir.path(), 50 * 1024 * 1024).unwrap();
        let opts = ThumbOptions::default();

        // A 4000 x 3000 "camera" JPEG (several hundred KB) with an embedded 160 x 120 preview.
        let photo = test_images::jpeg_with_exif(4000, 3000, &test_images::jpeg(160, 120), 1);
        assert!(photo.len() > 300_000, "{}", photo.len());
        let object = put(&env, "photos/IMG_0001.JPG", photo.clone(), "image/jpeg").await;
        let before = proxy.received.load(Ordering::Relaxed);
        let thumb = thumbs::thumbnail(&client, &cache, &object, &opts).await.unwrap();
        let fetched = proxy.received.load(Ordering::Relaxed) - before;
        assert_eq!(thumb.source, Source::Embedded);
        assert!(fetched < 140 * 1024, "read only the start of the file: {fetched} of {} bytes", photo.len());

        // The second time comes from the cache: nothing downloaded.
        let before = proxy.received.load(Ordering::Relaxed);
        let again = thumbs::thumbnail(&client, &cache, &object, &opts).await.unwrap();
        assert_eq!((again.source, again.jpeg == thumb.jpeg), (Source::Cache, true));
        assert_eq!(proxy.received.load(Ordering::Relaxed), before);

        // A PNG is downloaded whole (it's under the cap) and scaled down.
        let mut png = Vec::new();
        image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(800, 400, image::Rgb([10, 120, 200])))
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        let object = put(&env, "plots/map.png", png, "image/png").await;
        let thumb = thumbs::thumbnail(&client, &cache, &object, &opts).await.unwrap();
        assert_eq!(thumb.source, Source::Full);
        assert_eq!(image::load_from_memory(&thumb.jpeg).unwrap().width(), 256);
    }

    #[tokio::test]
    async fn big_cold_and_non_image_files_are_not_downloaded() {
        let Some(env) = Env::new().await else { return };
        let proxy = Proxy::start(&env.emulator.base).await;
        let http = GcsClient::http_client();
        let client = GcsClient::with_base(http.clone(), &proxy.base, Arc::new(TokenProvider::none(http)));
        let dir = tempfile::tempdir().unwrap();
        let cache = ThumbCache::open(dir.path(), 50 * 1024 * 1024).unwrap();

        let big = put(&env, "big.png", vec![7u8; 3 * 1024 * 1024], "image/png").await;
        let before = proxy.received.load(Ordering::Relaxed);
        let small_cap = ThumbOptions { max_bytes: 1024 * 1024, ..Default::default() };
        assert_eq!(thumbs::thumbnail(&client, &cache, &big, &small_cap).await.unwrap_err(), Skip::TooLarge { size: 3 * 1024 * 1024 });
        let cold = ObjectRef { storage_class: Some("COLDLINE".into()), ..big.clone() };
        assert!(matches!(thumbs::thumbnail(&client, &cache, &cold, &ThumbOptions::default()).await, Err(Skip::Cold { .. })));
        let csv = ObjectRef { name: "data.csv".into(), content_type: Some("text/csv".into()), ..big.clone() };
        assert_eq!(thumbs::thumbnail(&client, &cache, &csv, &ThumbOptions::default()).await.unwrap_err(), Skip::NotImage);
        assert_eq!(proxy.received.load(Ordering::Relaxed), before, "nothing downloaded");

        // Not a real image: downloaded once, then remembered.
        let broken = put(&env, "broken.jpg", b"not really a jpeg".to_vec(), "image/jpeg").await;
        assert!(matches!(thumbs::thumbnail(&client, &cache, &broken, &ThumbOptions::default()).await, Err(Skip::Unreadable { .. })));
        let before = proxy.received.load(Ordering::Relaxed);
        assert!(matches!(thumbs::thumbnail(&client, &cache, &broken, &ThumbOptions::default()).await, Err(Skip::Unreadable { .. })));
        assert_eq!(proxy.received.load(Ordering::Relaxed), before, "remembered, not downloaded again");
    }

    #[tokio::test]
    async fn the_preview_pane_gets_a_bigger_image_and_the_start_of_text_files() {
        use jetstream_transfer::preview::{self, TextPreview};
        let Some(env) = Env::new().await else { return };
        let proxy = Proxy::start(&env.emulator.base).await;
        let http = GcsClient::http_client();
        let client = GcsClient::with_base(http.clone(), &proxy.base, Arc::new(TokenProvider::none(http)));
        let dir = tempfile::tempdir().unwrap();
        let cache = ThumbCache::open(dir.path(), 50 * 1024 * 1024).unwrap();

        // A camera JPEG whose embedded preview is tile-sized: the tile uses it, the pane doesn't.
        let mut photo = test_images::jpeg_with_exif(2000, 1500, &test_images::jpeg(160, 120), 1);
        photo.resize(photo.len().max(200 * 1024), 0);
        let photo = put(&env, "big_photo.jpg", photo, "image/jpeg").await;
        let tile = thumbs::thumbnail(&client, &cache, &photo, &ThumbOptions::default()).await.unwrap();
        assert_eq!(tile.source, Source::Embedded);
        let pane_opts = ThumbOptions { edge: thumbs::PREVIEW_EDGE, ..Default::default() };
        let pane = thumbs::thumbnail(&client, &cache, &photo, &pane_opts).await.unwrap();
        assert_eq!(pane.source, Source::Full);
        assert_eq!(image::load_from_memory(&pane.jpeg).unwrap().width(), 1024);
        // Both cached separately.
        assert_eq!(thumbs::thumbnail(&client, &cache, &photo, &pane_opts).await.unwrap().source, Source::Cache);
        assert_eq!(image::load_from_memory(&thumbs::thumbnail(&client, &cache, &photo, &ThumbOptions::default()).await.unwrap().jpeg).unwrap().width(), 160);

        // A big CSV: only the first 64 KB is read.
        let row = "2024-03-01T00:00:00Z,19.5,\"Kona, HI\"\n";
        let csv: String = std::iter::once("time,temp,site\n".to_string()).chain(std::iter::repeat_n(row.to_string(), 100_000)).collect();
        let size = csv.len() as u64;
        let object = put(&env, "ctd.csv", csv.into_bytes(), "text/csv").await;
        let before = proxy.received.load(Ordering::Relaxed);
        let TextPreview::Text { text, shown, size: total, truncated } = preview::text(&client, "bucket", "ctd.csv", object.generation.as_deref(), size, None, false).await else {
            panic!("not text")
        };
        assert_eq!((shown, total, truncated), (preview::TEXT_BYTES, size, true));
        assert!(text.starts_with("time,temp,site\n2024-03-01"));
        let read = proxy.received.load(Ordering::Relaxed) - before;
        assert!(read < 2 * preview::TEXT_BYTES, "read {read} bytes of {size}");

        // Binary, cold, small.
        put(&env, "x.bin", vec![0u8, 1, 2, 3, 0, 9], "application/octet-stream").await;
        assert_eq!(preview::text(&client, "bucket", "x.bin", None, 6, None, false).await, TextPreview::Binary);
        assert!(matches!(preview::text(&client, "bucket", "ctd.csv", None, size, Some("ARCHIVE"), false).await, TextPreview::Cold { .. }));
        put(&env, "small.json", b"{\"a\": 1}".to_vec(), "application/json").await;
        assert_eq!(
            preview::text(&client, "bucket", "small.json", None, 8, None, false).await,
            TextPreview::Text { text: "{\"a\": 1}".into(), shown: 8, size: 8, truncated: false }
        );
    }
}

/// What's recorded on each object: content type and the source file's dates.
mod file_details {
    use super::*;
    use jetstream_transfer::fileops::OpProgress;
    use jetstream_transfer::gcs::{FILE_CREATED_KEY, FILE_MTIME_KEY};
    use jetstream_transfer::localops;
    use jetstream_transfer::throttle::Throttle;

    fn mtime_ns(path: &Path) -> i64 {
        std::fs::metadata(path).unwrap().modified().unwrap().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos() as i64
    }

    async fn raw(env: &Env, name: &str) -> serde_json::Value {
        env.client.get_object_raw("bucket", name).await.unwrap().expect("object")
    }

    #[tokio::test]
    async fn uploads_record_their_type_and_original_dates() {
        let Some(env) = Env::new().await else { return };
        env.write("photos/IMG_1.JPG", &big(5000, 1));
        env.write("casts.csv", b"depth,temp\n1,20\n");
        env.write("sst.nc", &big(3 * 1024 * 1024, 2)); // resumable
        env.write("big.bin", &big(5 * 1024 * 1024, 3)); // parallel parts
        env.write("notes.zzz", b"?");
        set_mtime(&env.src().join("casts.csv"), 86_400 * 400);
        let mut opts = env.options();
        opts.composite_threshold = 4 * 1024 * 1024;
        opts.composite_part_size = 1024 * 1024;
        let (summary, _) = env.run(opts.clone()).await;
        assert_eq!((summary.uploaded, summary.composite_files), (5, 1), "{summary:?}");

        for (name, expected) in [
            ("data/photos/IMG_1.JPG", "image/jpeg"),
            ("data/casts.csv", "text/csv"),
            ("data/sst.nc", "application/x-netcdf"),
            ("data/big.bin", "application/octet-stream"),
            ("data/notes.zzz", "application/octet-stream"),
        ] {
            let object = raw(&env, name).await;
            assert_eq!(object["contentType"], expected, "{name}");
            assert!(object["metadata"][FILE_MTIME_KEY].as_str().is_some(), "{name}: modified time");
        }
        let csv = raw(&env, "data/casts.csv").await;
        let mtime: i64 = csv["metadata"][FILE_MTIME_KEY].as_str().unwrap().parse().unwrap();
        let local = mtime_ns(&env.src().join("casts.csv")) / 1_000_000_000;
        assert_eq!(mtime, local, "the original modified time, not the upload time");
        // Created times where the file system records them (most do; some Linux ones don't).
        if std::fs::metadata(env.src().join("casts.csv")).unwrap().created().is_ok() {
            assert!(csv["metadata"][FILE_CREATED_KEY].as_str().is_some(), "created time recorded");
            assert!(raw(&env, "data/big.bin").await["metadata"][FILE_CREATED_KEY].as_str().is_some(), "parallel parts too");
        }
        // Nothing to do on the next sync.
        let (again, _) = env.run(opts).await;
        assert_eq!(again.uploaded, 0);

        // Files uploaded from the Cloud storage view too.
        let up = localops::upload_files(&env.client, &[env.src().join("photos/IMG_1.JPG")], "bucket", "drop", &Throttle::default(), &OpProgress::default()).await.unwrap();
        assert_eq!(up.done, 1);
        let dropped = raw(&env, "drop/IMG_1.JPG").await;
        assert_eq!(dropped["contentType"], "image/jpeg");
        assert!(dropped["metadata"][FILE_MTIME_KEY].as_str().is_some());
    }

    /// Upload like older versions did: binary type, no dates.
    async fn old_style_upload(env: &Env, name: &str, data: &[u8]) {
        let url = format!("{}/upload/storage/v1/b/bucket/o?uploadType=media&name={}", env.emulator.base, jetstream_transfer::gcs::encode(name));
        env.http.post(url).header("Content-Type", "application/octet-stream").body(data.to_vec()).send().await.unwrap().error_for_status().unwrap();
    }

    #[tokio::test]
    async fn fix_details_adds_types_and_dates_without_re_uploading() {
        let Some(env) = Env::new().await else { return };
        use jetstream_transfer::details::{apply_fix, plan_fix};
        let files = [("photos/a.jpg", big(4000, 1)), ("tables/casts.csv", b"d,t\n1,2\n".to_vec()), ("raw/blob.zzz", big(100, 2)), ("changed.png", big(300, 3))];
        for (rel, data) in &files {
            env.write(rel, data);
            old_style_upload(&env, &format!("data/{rel}"), data).await;
        }
        env.write("changed.png", &big(301, 4)); // edited since: not the same file, so no dates
        old_style_upload(&env, "data/only_in_cloud.tif", &big(50, 5)).await;
        let generation_before = raw(&env, "data/photos/a.jpg").await["generation"].clone();

        let local = (env.src(), Vec::new(), true);
        let plan = plan_fix(&env.client, "gs://bucket/data", Some((local.0.as_path(), local.1.as_slice(), local.2)), None, &OpProgress::default()).await.unwrap();
        assert_eq!(plan.objects, 5);
        assert_eq!(plan.types, 4, "jpg, csv, png, tif get a type; .zzz stays binary: {plan:?}");
        assert_eq!(plan.dates, 3, "the three files that are the same as on this computer");
        assert_eq!((plan.not_local, plan.different, plan.read_files), (1, 1, 3));
        assert_eq!(plan.by_type.get("image/jpeg"), Some(&1));

        let result = apply_fix(&env.client, &plan, &OpProgress::default()).await;
        assert_eq!((result.done, result.failed, result.skipped), (5, 0, 0), "{result:?}");
        let fixed = raw(&env, "data/photos/a.jpg").await;
        assert_eq!(fixed["contentType"], "image/jpeg");
        assert_eq!(fixed["generation"], generation_before, "same content: not re-uploaded");
        let mtime: i64 = fixed["metadata"][FILE_MTIME_KEY].as_str().unwrap().parse().unwrap();
        assert_eq!(mtime, std::fs::metadata(env.src().join("photos/a.jpg")).unwrap().modified().unwrap().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64);
        assert_eq!(raw(&env, "data/only_in_cloud.tif").await["contentType"], "image/tiff");
        assert!(raw(&env, "data/changed.png").await["metadata"][FILE_MTIME_KEY].is_null(), "no dates for a different file");
        assert_eq!(env.object("data/photos/a.jpg").await.unwrap().1, files[0].1);

        // Nothing left to do.
        let again = plan_fix(&env.client, "gs://bucket/data", Some((local.0.as_path(), local.1.as_slice(), local.2)), None, &OpProgress::default()).await.unwrap();
        assert!(again.items.is_empty(), "{again:?}");

        // "Changed after planning: left alone" is checked in tests/stall.rs: fake-gcs-server
        // reports metagenerations but doesn't enforce ifMetagenerationMatch.
    }
}

#[tokio::test]
async fn speed_check_measures_without_storing_anything() {
    let Some(env) = Env::new().await else { return };
    env.write("big.bin", &vec![7u8; 6 * 1024 * 1024]);
    for i in 0..40 {
        env.write(&format!("small/{i}.txt"), b"hello");
    }
    let progress = jetstream_transfer::fileops::OpProgress::default();
    let report = jetstream_transfer::speedcheck::speed_check(&env.client, "gs://bucket/data", Some(&env.src()), &progress).await.unwrap();
    let folder = report.folder.as_ref().unwrap();
    assert_eq!(folder.files, 41);
    assert_eq!(folder.read_bytes, 6 * 1024 * 1024);
    assert!(folder.small_files_per_sec.is_some());
    assert!(report.single_mb_per_sec > 0.0 && report.parallel_mb_per_sec > 0.0, "{report:?}");
    assert!(report.request_ms >= 0.0);
    assert!(!report.findings.is_empty());
    assert_eq!(progress.done.load(std::sync::atomic::Ordering::Relaxed), jetstream_transfer::speedcheck::STEPS);
    // No objects were created.
    assert!(env.client.list("bucket", "").await.unwrap().is_empty(), "the check left objects behind");

    // Stopped before it starts: returns at once, marked stopped.
    let stopped = jetstream_transfer::fileops::OpProgress::default();
    stopped.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
    let report = jetstream_transfer::speedcheck::speed_check(&env.client, "gs://bucket/data", None, &stopped).await.unwrap();
    assert!(report.cancelled);
}

#[tokio::test]
async fn catalog_lists_the_bucket_with_file_details() {
    use jetstream_transfer::catalog::{self, CatalogRequest};
    use jetstream_transfer::fileops::OpProgress;
    let Some(env) = Env::new().await else { return };
    for name in ["reef/site_A_001.jpg", "reef/site_A_002.JPG", "reef/site_B_001.png", "reef/thumbs/site_B_001.png", "reef/ctd.csv", "reef/sst.nc"] {
        env.write(name, name.as_bytes());
    }
    let (summary, _) = env.run(env.options()).await;
    assert_eq!(summary.uploaded, 6);

    let request: CatalogRequest = serde_json::from_value(serde_json::json!({ "gcs_prefix": "gs://bucket/data/reef", "kinds": ["images"], "ignore_folder_names": ["thumbs"], "filename_token_count": 2 })).unwrap();
    let progress = OpProgress::default();
    let listing = catalog::scan(&env.client, &request, &progress).await.unwrap();
    assert_eq!(listing.files.len(), 3);
    assert_eq!((progress.done.load(std::sync::atomic::Ordering::Relaxed), progress.skipped.load(std::sync::atomic::Ordering::Relaxed)), (6, 3));
    let file = &listing.files[0];
    assert_eq!(file.content_type.as_deref(), Some("image/jpeg"));
    assert!(file.modified.is_some() && file.crc32c.is_some() && file.uploaded.is_some(), "{file:?}");

    let built = catalog::build(&listing, &request.grouping, "now").unwrap();
    assert_eq!(built.grouped_instances.keys().collect::<Vec<_>>(), vec!["site_A", "site_B"]);
    assert!(catalog::oasis_json(&built).contains("\"gs://bucket/data/reef/site_A_001.jpg\""));
    let csv = catalog::detailed_csv(&listing, &built);
    assert_eq!(csv.lines().count(), 4);
    assert!(csv.lines().nth(1).unwrap().contains(",image/jpeg,STANDARD,"), "{csv}");

    // Everything, capped at 2.
    let all: CatalogRequest = serde_json::from_value(serde_json::json!({ "gcs_prefix": "gs://bucket/data/reef/", "kinds": ["all"], "max_results": 2 })).unwrap();
    let capped = catalog::scan(&env.client, &all, &OpProgress::default()).await.unwrap();
    assert_eq!((capped.files.len(), capped.truncated), (2, true));
    // Stopped.
    let stopped = OpProgress::default();
    stopped.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
    assert!(catalog::scan(&env.client, &all, &stopped).await.is_err());
}
