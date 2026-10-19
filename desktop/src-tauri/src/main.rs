// JetStream desktop shell.
//
// Starts the bundled JetStream server (a PyInstaller sidecar) on a private localhost
// port and reports its status to the desktop UI (Svelte, in ../app), which talks to the
// server's HTTP API for jobs and presets. Cloud Storage browsing and file management, and
// local folder analysis, run in-process in the Rust core (../transfer). The original web pages stay
// available in a separate "classic" window. A tray icon keeps uploads running when the
// window is closed.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::collections::{HashMap, VecDeque};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use jetstream_transfer::audit::{self, AuditProgress, AuditReport};
use jetstream_transfer::auth::TokenProvider;
use jetstream_transfer::browse::{self, FolderTotals, Listing};
use jetstream_transfer::compare::{self, CompareReport};
use jetstream_transfer::localops;
use jetstream_transfer::manifest::{default_data_dir, Manifest};
use jetstream_transfer::throttle::Throttle;
use jetstream_transfer::fileops::{self, BucketAccess, DeletedPage, Item, OpProgress, OpResult, Plan, PlanSummary, Undo};
use jetstream_transfer::gcs::GcsClient;
use jetstream_transfer::scan::{self, ScanOptions, ScanProgress, ScanReport};
use serde::{Deserialize, Serialize};
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, RunEvent, Url, WebviewUrl, WebviewWindowBuilder, WindowEvent};
use tauri_plugin_notification::NotificationExt;
use tauri_plugin_shell::process::{CommandChild, CommandEvent};
use tauri_plugin_shell::ShellExt;

const SERVER_START_TIMEOUT: Duration = Duration::from_secs(120);
const LOG_LINES: usize = 200;

/// Origins the desktop UI is served from (they differ by platform).
const APP_ORIGINS: &str = "tauri://localhost,http://tauri.localhost,https://tauri.localhost,http://localhost:5173";

#[derive(Clone, Serialize)]
#[serde(rename_all = "snake_case")]
enum Phase {
    Starting,
    Ready,
    Failed,
}

#[derive(Clone, Serialize)]
struct ServerStatus {
    phase: Phase,
    port: Option<u16>,
    message: Option<String>,
}

struct Server {
    child: Mutex<Option<CommandChild>>,
    log: Mutex<VecDeque<String>>,
    status: Mutex<ServerStatus>,
    exited: AtomicBool,
    tray_hint_shown: AtomicBool,
}

impl Default for Server {
    fn default() -> Self {
        Self {
            child: Mutex::new(None),
            log: Mutex::new(VecDeque::new()),
            status: Mutex::new(ServerStatus { phase: Phase::Starting, port: None, message: None }),
            exited: AtomicBool::new(false),
            tray_hint_shown: AtomicBool::new(false),
        }
    }
}

impl Server {
    fn push_log(&self, line: String) {
        let mut log = self.log.lock().unwrap();
        if log.len() >= LOG_LINES {
            log.pop_front();
        }
        log.push_back(line);
    }

    fn log_text(&self) -> String {
        self.log.lock().unwrap().iter().cloned().collect::<Vec<_>>().join("\n")
    }

    fn stop(&self) {
        if let Some(child) = self.child.lock().unwrap().take() {
            let _ = child.kill();
        }
    }

    fn port(&self) -> Option<u16> {
        self.status.lock().unwrap().port
    }
}

fn set_status(app: &AppHandle, phase: Phase, message: Option<String>) {
    let server = app.state::<Server>();
    let status = {
        let mut status = server.status.lock().unwrap();
        status.phase = phase;
        status.message = message;
        status.clone()
    };
    let _ = app.emit("server-status", status);
}

/// Current server status, polled by the UI on load (events cover later changes).
#[tauri::command]
fn server_status(app: AppHandle) -> ServerStatus {
    app.state::<Server>().status.lock().unwrap().clone()
}

/// Last lines of server output, shown when startup fails.
#[tauri::command]
fn server_log(app: AppHandle) -> String {
    app.state::<Server>().log_text()
}

/// Open one of the original web pages (e.g. "static/cloud.html") in the classic window.
#[tauri::command]
fn open_classic(app: AppHandle, path: String) -> Result<(), String> {
    let port = app.state::<Server>().port().ok_or("JetStream is still starting")?;
    let path = path.trim_start_matches('/');
    let url = Url::parse(&format!("http://127.0.0.1:{port}/{path}")).map_err(|e| e.to_string())?;
    if let Some(window) = app.get_webview_window("classic") {
        window.navigate(url).map_err(|e| e.to_string())?;
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
        return Ok(());
    }
    WebviewWindowBuilder::new(&app, "classic", WebviewUrl::External(url))
        .title("JetStream — Classic pages")
        .inner_size(1300.0, 900.0)
        .min_inner_size(900.0, 600.0)
        .build()
        .map_err(|e| e.to_string())?;
    Ok(())
}

// ----- Rust core: Cloud Storage browsing and local folder analysis ---------------------------

/// Cloud Storage client and running folder scans, shared by the core commands.
#[derive(Default)]
struct Core {
    client: Mutex<Option<GcsClient>>,
    scans: Mutex<HashMap<String, Arc<ScanProgress>>>,
    audits: Mutex<HashMap<String, Arc<AuditProgress>>>,
    /// Planned (not yet run) file operations, by operation id.
    plans: Mutex<HashMap<String, (Plan, bool)>>,
    /// Running file operations, for cancelling.
    running: Mutex<HashMap<String, Arc<OpProgress>>>,
    /// How to undo finished operations (with the bucket's soft-delete setting).
    undos: Mutex<HashMap<String, (Undo, bool)>>,
    /// Speed limit for downloads and file uploads run in the app.
    throttle: Arc<Throttle>,
}

impl Core {
    /// Reuse one client (it caches the access token); recreated after errors so a new
    /// sign-in is picked up.
    fn client(&self) -> Result<GcsClient, String> {
        let mut slot = self.client.lock().unwrap();
        if let Some(client) = slot.as_ref() {
            return Ok(client.clone());
        }
        let http = GcsClient::http_client();
        let auth = TokenProvider::from_environment(http.clone()).map_err(|e| format!("{e:#}"))?;
        let client = GcsClient::new(http, Arc::new(auth));
        *slot = Some(client.clone());
        Ok(client)
    }

    fn reset_client(&self) {
        *self.client.lock().unwrap() = None;
    }
}

#[tauri::command]
async fn list_folder(
    core: tauri::State<'_, Core>,
    bucket: String,
    prefix: String,
    page_token: Option<String>,
    include_folders: Option<bool>,
) -> Result<Listing, String> {
    let client = core.client()?;
    let include_folders = include_folders.unwrap_or(false);
    browse::list_folder(&client, bucket.trim(), &prefix, page_token.as_deref(), 500, include_folders).await.map_err(|e| {
        core.reset_client();
        format!("{e:#}")
    })
}

#[tauri::command]
async fn folder_totals(core: tauri::State<'_, Core>, bucket: String, prefix: String) -> Result<FolderTotals, String> {
    let client = core.client()?;
    browse::folder_totals(&client, bucket.trim(), &prefix, 2_000_000).await.map_err(|e| {
        core.reset_client();
        format!("{e:#}")
    })
}

#[derive(Clone, Serialize)]
struct ScanTick {
    scan_id: String,
    files: u64,
    bytes: u64,
}

/// Analyze a local folder; emits "scan-progress" while running.
#[tauri::command]
async fn scan_folder(app: AppHandle, core: tauri::State<'_, Core>, path: String, scan_id: String) -> Result<ScanReport, String> {
    let progress = Arc::new(ScanProgress::default());
    core.scans.lock().unwrap().insert(scan_id.clone(), progress.clone());

    let done = Arc::new(AtomicBool::new(false));
    let (tick_app, tick_progress, tick_done, tick_id) = (app.clone(), progress.clone(), done.clone(), scan_id.clone());
    std::thread::spawn(move || {
        while !tick_done.load(Ordering::Relaxed) {
            let _ = tick_app.emit(
                "scan-progress",
                ScanTick {
                    scan_id: tick_id.clone(),
                    files: tick_progress.files.load(Ordering::Relaxed),
                    bytes: tick_progress.bytes.load(Ordering::Relaxed),
                },
            );
            std::thread::sleep(Duration::from_millis(200));
        }
    });

    let scan_progress = progress.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        scan::scan_folder(std::path::Path::new(&path), &ScanOptions::default(), &scan_progress)
    })
    .await;
    done.store(true, Ordering::Relaxed);
    core.scans.lock().unwrap().remove(&scan_id);
    result.map_err(|e| e.to_string())?.map_err(|e| format!("{e:#}"))
}

#[tauri::command]
fn cancel_scan(core: tauri::State<'_, Core>, scan_id: String) {
    if let Some(progress) = core.scans.lock().unwrap().get(&scan_id) {
        progress.cancel.store(true, Ordering::Relaxed);
    }
}

#[derive(Clone, Serialize)]
struct AuditTick {
    audit_id: String,
    scanned: u64,
    found: u64,
}

#[tauri::command]
fn default_junk_patterns() -> Vec<String> {
    audit::DEFAULT_PATTERNS.iter().map(|p| p.to_string()).collect()
}

/// Find junk objects under gs://bucket/prefix (read-only); emits "audit-progress".
#[tauri::command]
async fn audit_bucket(
    app: AppHandle,
    core: tauri::State<'_, Core>,
    bucket: String,
    prefix: String,
    patterns: Vec<String>,
    audit_id: String,
) -> Result<AuditReport, String> {
    let client = core.client()?;
    let progress = Arc::new(AuditProgress::default());
    core.audits.lock().unwrap().insert(audit_id.clone(), progress.clone());

    let done = Arc::new(AtomicBool::new(false));
    let (tick_app, tick_progress, tick_done, tick_id) = (app.clone(), progress.clone(), done.clone(), audit_id.clone());
    std::thread::spawn(move || {
        while !tick_done.load(Ordering::Relaxed) {
            let _ = tick_app.emit(
                "audit-progress",
                AuditTick {
                    audit_id: tick_id.clone(),
                    scanned: tick_progress.scanned.load(Ordering::Relaxed),
                    found: tick_progress.found.load(Ordering::Relaxed),
                },
            );
            std::thread::sleep(Duration::from_millis(250));
        }
    });

    let result = audit::audit_bucket(&client, bucket.trim(), &prefix, &patterns, 0, 20_000, &progress).await;
    done.store(true, Ordering::Relaxed);
    core.audits.lock().unwrap().remove(&audit_id);
    result.map_err(|e| {
        core.reset_client();
        format!("{e:#}")
    })
}

#[tauri::command]
fn cancel_audit(core: tauri::State<'_, Core>, audit_id: String) {
    if let Some(progress) = core.audits.lock().unwrap().get(&audit_id) {
        progress.cancel.store(true, Ordering::Relaxed);
    }
}

// ----- Cloud file manager ------------------------------------------------------------------

/// Bucket settings (soft delete, hierarchical namespace, ...) and the user's permissions.
#[tauri::command]
async fn bucket_access(core: tauri::State<'_, Core>, bucket: String) -> Result<BucketAccess, String> {
    Ok(fileops::bucket_access(&core.client()?, bucket.trim()).await)
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
enum OpRequest {
    Copy { bucket: String, paths: Vec<String>, dst_bucket: String, dst_prefix: String },
    Move { bucket: String, paths: Vec<String>, dst_bucket: String, dst_prefix: String },
    Rename { bucket: String, path: String, new_name: String },
    Delete { bucket: String, paths: Vec<String> },
}

#[derive(Serialize)]
struct PlannedOp {
    #[serde(flatten)]
    summary: PlanSummary,
    soft_delete_seconds: u64,
}

fn items(paths: &[String]) -> Vec<Item> {
    paths.iter().map(|p| Item { path: p.clone() }).collect()
}

/// Work out what an operation will do (objects, clashes, fees) without changing anything.
/// Run it with `run_cloud_op` using the same id.
#[tauri::command]
async fn plan_cloud_op(core: tauri::State<'_, Core>, op_id: String, request: OpRequest) -> Result<PlannedOp, String> {
    let client = core.client()?;
    let bucket_name = match &request {
        OpRequest::Copy { bucket, .. } | OpRequest::Move { bucket, .. } | OpRequest::Rename { bucket, .. } | OpRequest::Delete { bucket, .. } => {
            bucket.trim().to_string()
        }
    };
    let source = fileops::bucket_access(&client, &bucket_name).await.bucket;
    let plan = match &request {
        OpRequest::Copy { paths, dst_bucket, dst_prefix, .. } => {
            fileops::plan_transfer(&client, &source, &items(paths), dst_bucket.trim(), dst_prefix, None, false).await
        }
        OpRequest::Move { paths, dst_bucket, dst_prefix, .. } => {
            fileops::plan_transfer(&client, &source, &items(paths), dst_bucket.trim(), dst_prefix, None, true).await
        }
        OpRequest::Rename { path, new_name, .. } => {
            let parent = path.trim_end_matches('/').rsplit_once('/').map(|(p, _)| format!("{p}/")).unwrap_or_default();
            fileops::plan_transfer(&client, &source, &[Item { path: path.clone() }], &bucket_name, &parent, Some(new_name), true).await
        }
        OpRequest::Delete { paths, .. } => fileops::plan_delete(&client, &source, &items(paths)).await,
    }
    .map_err(|e| format!("{e:#}"))?;
    let planned = PlannedOp { summary: plan.summary.clone(), soft_delete_seconds: source.soft_delete_seconds };
    core.plans.lock().unwrap().insert(op_id, (plan, source.soft_delete_seconds > 0));
    Ok(planned)
}

#[derive(Clone, Serialize)]
struct OpTick {
    op_id: String,
    total: u64,
    done: u64,
    skipped: u64,
    failed: u64,
    bytes: u64,
}

/// Run an operation with progress ticks ("cloud-op-progress") and cancellation by `op_id`.
async fn track<F, Fut>(app: &AppHandle, core: &Core, op_id: &str, work: F) -> Result<OpResult, String>
where
    F: FnOnce(Arc<OpProgress>) -> Fut,
    Fut: std::future::Future<Output = Result<OpResult, String>>,
{
    let progress = Arc::new(OpProgress::default());
    core.running.lock().unwrap().insert(op_id.to_string(), progress.clone());
    let finished = Arc::new(AtomicBool::new(false));
    let (tick_app, tick_progress, tick_done, tick_id) = (app.clone(), progress.clone(), finished.clone(), op_id.to_string());
    std::thread::spawn(move || {
        while !tick_done.load(Ordering::Relaxed) {
            let _ = tick_app.emit(
                "cloud-op-progress",
                OpTick {
                    op_id: tick_id.clone(),
                    total: tick_progress.total.load(Ordering::Relaxed),
                    done: tick_progress.done.load(Ordering::Relaxed),
                    skipped: tick_progress.skipped.load(Ordering::Relaxed),
                    failed: tick_progress.failed.load(Ordering::Relaxed),
                    bytes: tick_progress.bytes.load(Ordering::Relaxed),
                },
            );
            std::thread::sleep(Duration::from_millis(250));
        }
    });
    let result = work(progress).await;
    finished.store(true, Ordering::Relaxed);
    core.running.lock().unwrap().remove(op_id);
    result
}

async fn run_with_progress(app: &AppHandle, core: &Core, op_id: &str, plan: &Plan, soft_delete: bool) -> Result<OpResult, String> {
    let client = core.client()?;
    let result = track(app, core, op_id, |progress| async move { Ok(fileops::run(&client, plan, soft_delete, &progress).await) }).await?;
    if let Some(undo) = &result.undo {
        core.undos.lock().unwrap().insert(op_id.to_string(), (undo.clone(), soft_delete));
    }
    Ok(result)
}

/// Download files and folders into a folder on this computer (checked; never replaces files).
#[tauri::command]
async fn download_cloud(
    app: AppHandle,
    core: tauri::State<'_, Core>,
    op_id: String,
    bucket: String,
    paths: Vec<String>,
    dest_dir: String,
) -> Result<OpResult, String> {
    let client = core.client()?;
    let throttle = core.throttle.clone();
    track(&app, &core, &op_id, |progress| async move {
        localops::download(&client, bucket.trim(), &items(&paths), std::path::Path::new(&dest_dir), &throttle, &progress)
            .await
            .map_err(|e| format!("{e:#}"))
    })
    .await
}

/// Upload individual files into gs://bucket/prefix/ (existing objects are left alone).
#[tauri::command]
async fn upload_local_files(
    app: AppHandle,
    core: tauri::State<'_, Core>,
    op_id: String,
    bucket: String,
    prefix: String,
    files: Vec<String>,
) -> Result<OpResult, String> {
    let client = core.client()?;
    let throttle = core.throttle.clone();
    let files: Vec<std::path::PathBuf> = files.into_iter().map(std::path::PathBuf::from).collect();
    track(&app, &core, &op_id, |progress| async move {
        localops::upload_files(&client, &files, bucket.trim(), &prefix, &throttle, &progress).await.map_err(|e| format!("{e:#}"))
    })
    .await
}

/// Speed limit for transfers run by the app itself (downloads, file uploads), in Mbit/s.
#[tauri::command]
fn set_transfer_limit(core: tauri::State<'_, Core>, max_mbps: f64) {
    core.throttle.set_rate(jetstream_transfer::throttle::mbps_to_bytes(max_mbps));
}

/// Which dropped paths are folders (uploaded as sync jobs) and which are files.
#[tauri::command]
fn classify_paths(paths: Vec<String>) -> Vec<(String, bool)> {
    paths.into_iter().map(|p| {
        let is_dir = std::path::Path::new(&p).is_dir();
        (p, is_dir)
    }).collect()
}

/// Run a planned operation; emits "cloud-op-progress".
#[tauri::command]
async fn run_cloud_op(app: AppHandle, core: tauri::State<'_, Core>, op_id: String) -> Result<OpResult, String> {
    let (plan, soft_delete) = core.plans.lock().unwrap().remove(&op_id).ok_or("This operation was already run or has expired; plan it again.")?;
    run_with_progress(&app, &core, &op_id, &plan, soft_delete).await
}

/// Reverse a finished operation (as a new operation, `new_op_id`).
#[tauri::command]
async fn undo_cloud_op(app: AppHandle, core: tauri::State<'_, Core>, op_id: String, new_op_id: String) -> Result<OpResult, String> {
    let (undo, soft_delete) = core.undos.lock().unwrap().remove(&op_id).ok_or("Nothing to undo")?;
    run_with_progress(&app, &core, &new_op_id, &fileops::plan_undo(&undo), soft_delete).await
}

#[tauri::command]
fn cancel_cloud_op(core: tauri::State<'_, Core>, op_id: String) {
    core.plans.lock().unwrap().remove(&op_id);
    if let Some(progress) = core.running.lock().unwrap().get(&op_id) {
        progress.cancel.store(true, Ordering::Relaxed);
    }
}

#[derive(Deserialize)]
struct DeletedObject {
    name: String,
    generation: String,
}

/// Bring back soft-deleted objects; a live object with the same name is never replaced.
#[tauri::command]
async fn restore_objects(
    app: AppHandle,
    core: tauri::State<'_, Core>,
    op_id: String,
    bucket: String,
    objects: Vec<DeletedObject>,
) -> Result<OpResult, String> {
    let pairs: Vec<(String, String)> = objects.into_iter().map(|o| (o.name, o.generation)).collect();
    run_with_progress(&app, &core, &op_id, &fileops::plan_restore(bucket.trim(), &pairs), true).await
}

/// Soft-deleted objects under a folder (recursively).
#[tauri::command]
async fn list_deleted(core: tauri::State<'_, Core>, bucket: String, prefix: String, page_token: Option<String>) -> Result<DeletedPage, String> {
    fileops::list_deleted(&core.client()?, bucket.trim(), &prefix, page_token.as_deref()).await.map_err(|e| format!("{e:#}"))
}

#[tauri::command]
async fn new_folder(core: tauri::State<'_, Core>, bucket: String, parent: String, name: String) -> Result<String, String> {
    let client = core.client()?;
    let source = fileops::bucket_access(&client, bucket.trim()).await.bucket;
    fileops::new_folder(&client, &source, &parent, &name).await.map_err(|e| format!("{e:#}"))
}

/// Compare a local folder with its cloud copy (read-only). Uses the upload manifest, when
/// there is one, to confirm matching files by checksum.
#[tauri::command]
async fn compare_folder(
    core: tauri::State<'_, Core>,
    path: String,
    destination: String,
    exclude_patterns: Vec<String>,
    exclude_folders: Vec<String>,
    recursive: bool,
) -> Result<CompareReport, String> {
    let client = core.client()?;
    let excludes = compare::exclude_regexes(&exclude_patterns, &exclude_folders).map_err(|e| format!("{e:#}"))?;
    let data_dir = portable_data_dir().unwrap_or_else(default_data_dir);
    let manifest_path = data_dir.join("transfer-manifest.db");
    let manifest = if manifest_path.exists() { Manifest::open(&manifest_path).ok() } else { None };
    compare::compare(&client, std::path::Path::new(&path), &destination, &excludes, recursive, manifest.as_ref())
        .await
        .map_err(|e| format!("{e:#}"))
}

/// Portable mode: a "JetStreamData" folder next to the program keeps the job history,
/// logs and upload manifest with the program (e.g. on a USB drive) instead of the user profile.
fn portable_data_dir() -> Option<std::path::PathBuf> {
    let dir = std::env::current_exe().ok()?.parent()?.join("JetStreamData");
    dir.is_dir().then_some(dir)
}

fn free_port() -> std::io::Result<u16> {
    Ok(TcpListener::bind("127.0.0.1:0")?.local_addr()?.port())
}

/// True once the server answers GET /api/version with 200.
fn server_ready(port: u16) -> bool {
    let addr = format!("127.0.0.1:{port}");
    let Ok(mut stream) = TcpStream::connect_timeout(&addr.parse().unwrap(), Duration::from_millis(500)) else {
        return false;
    };
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    let request = format!("GET /api/version HTTP/1.0\r\nHost: {addr}\r\n\r\n");
    if stream.write_all(request.as_bytes()).is_err() {
        return false;
    }
    let mut buf = [0u8; 16];
    let n = stream.read(&mut buf).unwrap_or(0);
    let head = String::from_utf8_lossy(&buf[..n]);
    head.starts_with("HTTP/1.1 200") || head.starts_with("HTTP/1.0 200")
}

fn show_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

fn start_server(app: &AppHandle) -> Result<(), Box<dyn std::error::Error>> {
    let port = free_port()?;
    let cors = format!("{APP_ORIGINS},http://127.0.0.1:{port}");
    let mut command = app
        .shell()
        .sidecar("jetstream-server")?
        .args(["--host", "127.0.0.1", "--port", &port.to_string(), "--no-browser"])
        .env("AUTO_OPEN_BROWSER", "false")
        .env("CORS_ALLOW_ORIGINS", cors)
        .env("JETSTREAM_DESKTOP_PARENT_PID", std::process::id().to_string())
        .env("PYTHONUNBUFFERED", "1")
        // Output comes through a pipe: make it UTF-8 rather than the Windows code page.
        .env("PYTHONIOENCODING", "utf-8:replace")
        .env("PYTHONUTF8", "1");
    if let Some(dir) = portable_data_dir() {
        app.state::<Server>().push_log(format!("Portable mode: data in {}", dir.display()));
        command = command.env("JETSTREAM_DATA_DIR", dir);
    }
    let (mut events, child) = command.spawn()?;
    {
        let server = app.state::<Server>();
        *server.child.lock().unwrap() = Some(child);
        server.status.lock().unwrap().port = Some(port);
    }

    // Keep the tail of the server output for the startup error screen.
    let log_app = app.clone();
    tauri::async_runtime::spawn(async move {
        while let Some(event) = events.recv().await {
            let server = log_app.state::<Server>();
            match event {
                CommandEvent::Stdout(line) | CommandEvent::Stderr(line) => {
                    server.push_log(String::from_utf8_lossy(&line).trim_end().to_string());
                }
                CommandEvent::Error(err) => server.push_log(format!("error: {err}")),
                CommandEvent::Terminated(payload) => {
                    server.push_log(format!("server exited (code {:?})", payload.code));
                    server.exited.store(true, Ordering::SeqCst);
                    if matches!(server.status.lock().unwrap().phase, Phase::Ready) {
                        set_status(&log_app, Phase::Failed, Some("The JetStream server stopped unexpectedly.".into()));
                    }
                }
                _ => {}
            }
        }
    });

    let wait_app = app.clone();
    std::thread::spawn(move || {
        let started = Instant::now();
        loop {
            if server_ready(port) {
                set_status(&wait_app, Phase::Ready, None);
                return;
            }
            if wait_app.state::<Server>().exited.load(Ordering::SeqCst) {
                set_status(&wait_app, Phase::Failed, Some("The JetStream server stopped while starting.".into()));
                return;
            }
            if started.elapsed() > SERVER_START_TIMEOUT {
                set_status(&wait_app, Phase::Failed, Some("The JetStream server did not respond within two minutes.".into()));
                return;
            }
            std::thread::sleep(Duration::from_millis(300));
        }
    });
    Ok(())
}

fn build_tray(app: &AppHandle) -> tauri::Result<()> {
    let open = MenuItem::with_id(app, "open", "Open JetStream", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit JetStream (stops uploads)", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&open, &quit])?;

    let mut tray = TrayIconBuilder::with_id("main")
        .tooltip("NOAA JetStream")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "open" => show_main_window(app),
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = event {
                show_main_window(tray.app_handle());
            }
        });
    if let Some(icon) = app.default_window_icon() {
        tray = tray.icon(icon.clone());
    }
    tray.build(app)?;
    Ok(())
}

fn main() {
    let app = tauri::Builder::default()
        // Must be registered first: a second launch focuses the running app.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| show_main_window(app)))
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_opener::init())
        .manage(Server::default())
        .manage(Core::default())
        .invoke_handler(tauri::generate_handler![
            server_status,
            server_log,
            open_classic,
            list_folder,
            folder_totals,
            scan_folder,
            cancel_scan,
            default_junk_patterns,
            audit_bucket,
            cancel_audit,
            bucket_access,
            plan_cloud_op,
            run_cloud_op,
            undo_cloud_op,
            cancel_cloud_op,
            restore_objects,
            list_deleted,
            new_folder,
            compare_folder,
            download_cloud,
            upload_local_files,
            set_transfer_limit,
            classify_paths
        ])
        .setup(|app| {
            build_tray(app.handle())?;
            if let Err(err) = start_server(app.handle()) {
                app.state::<Server>().push_log(format!("Could not launch server: {err}"));
                set_status(app.handle(), Phase::Failed, Some("The bundled JetStream server could not be launched.".into()));
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            // Closing the main window keeps JetStream running in the tray so uploads continue.
            if let WindowEvent::CloseRequested { api, .. } = event {
                if window.label() == "main" {
                    api.prevent_close();
                    let _ = window.hide();
                    let app = window.app_handle();
                    if !app.state::<Server>().tray_hint_shown.swap(true, Ordering::SeqCst) {
                        let _ = app
                            .notification()
                            .builder()
                            .title("JetStream is still running")
                            .body("Uploads continue in the background. Use the tray icon to reopen or quit.")
                            .show();
                    }
                }
            }
        })
        .build(tauri::generate_context!())
        .expect("error while building JetStream");

    app.run(|app, event| {
        if let RunEvent::Exit = event {
            app.state::<Server>().stop();
        }
    });
}
