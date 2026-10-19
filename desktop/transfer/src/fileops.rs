//! File-manager operations on Cloud Storage: copy, move and rename (server-side copy, verify,
//! then delete), delete, restore from soft delete, and new folders.
//!
//! Every operation is planned first (list what it touches, find name clashes, estimate
//! early-deletion fees), shown to the user, then run. Each object is handled on its own with
//! generation preconditions, so an object changed after planning is left alone, and an
//! interrupted move can be planned and run again: objects already copied are recognised by
//! checksum and only their originals are removed.
//!
//! Hierarchical-namespace buckets have real folders: renaming or moving a folder within such
//! a bucket is a single Google operation, and folders are created and removed explicitly.

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, bail, Result};
use futures::stream::{self, StreamExt};
use serde::{Deserialize, Serialize};

use crate::browse::folder_prefix;
use crate::gcs::{BucketInfo, GcsClient, GcsError, ObjectInfo, KEPT_METADATA};

const CONCURRENCY: usize = 16;
const MAX_ATTEMPTS: u32 = 5;
/// Failures listed in a result (the count covers all of them).
const MAX_REPORTED_FAILURES: usize = 200;
/// Larger operations aren't offered an undo (it would be another long job).
const MAX_UNDO_OBJECTS: usize = 1000;

/// Permissions the file manager asks about, to grey out what the user can't do.
pub const PERMISSIONS: &[&str] = &[
    "storage.objects.list",
    "storage.objects.get",
    "storage.objects.create",
    "storage.objects.delete",
    "storage.objects.restore",
];

/// A selected file (`path` is the object name) or folder (`path` ends with "/").
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Item {
    pub path: String,
}

impl Item {
    fn is_folder(&self) -> bool {
        self.path.ends_with('/')
    }
    /// "a/b/c.jpg" -> "c.jpg", "a/b/" -> "b"
    fn base_name(&self) -> &str {
        self.path.trim_end_matches('/').rsplit('/').next().unwrap_or("")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OpKind {
    Copy,
    Move,
    Delete,
    Restore,
}

#[derive(Debug, Clone)]
struct Task {
    bucket: String,
    name: String,
    generation: Option<String>,
    size: u64,
    crc32c: Option<String>,
    kms_key: Option<String>,
    /// Destination bucket and name (copy/move).
    dst: Option<(String, String)>,
    /// Metadata to carry over to the copy (`Null`: read it from the object when it runs).
    metadata: serde_json::Value,
}

/// Objects in a cold storage class that would be removed before its minimum storage duration
/// (Google bills the remaining days).
#[derive(Debug, Clone, Default, Serialize)]
pub struct EarlyDeletion {
    pub storage_class: String,
    pub min_days: u32,
    pub objects: u64,
    pub bytes: u64,
}

/// What an operation will do, shown before it runs.
#[derive(Debug, Clone, Default, Serialize)]
pub struct PlanSummary {
    pub kind: Option<OpKind>,
    pub objects: u64,
    pub bytes: u64,
    /// Destinations that already exist (with different content); those are left alone.
    pub conflicts: u64,
    pub conflict_examples: Vec<String>,
    /// Copies left by an interrupted move; only the originals still need removing.
    pub already_copied: u64,
    /// Objects under a hold, which can't be deleted or moved.
    pub held: u64,
    pub early_deletion: Vec<EarlyDeletion>,
    /// Folders renamed in one step (hierarchical-namespace buckets).
    pub folder_renames: u64,
    /// Example destination, e.g. the new name of the first object.
    pub example: Option<String>,
}

/// How a destination that already exists is handled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Existing {
    /// Different object: leave both alone.
    Conflict,
    /// Same content (an earlier, interrupted move): just remove the original.
    SameContent,
}

pub struct Plan {
    pub kind: OpKind,
    pub summary: PlanSummary,
    tasks: Vec<Task>,
    existing: HashMap<String, Existing>,
    /// Hierarchical namespace: (bucket, from folder, to folder) renamed by Google in one step.
    folder_renames: Vec<(String, String, String)>,
    /// Hierarchical namespace: folders to remove once their objects are gone (bucket, prefix).
    folder_cleanup: Vec<(String, String)>,
}

#[derive(Default)]
pub struct OpProgress {
    pub done: AtomicU64,
    pub skipped: AtomicU64,
    pub failed: AtomicU64,
    pub bytes: AtomicU64,
    pub cancel: AtomicBool,
}

#[derive(Debug, Clone, Serialize)]
pub struct Failure {
    pub path: String,
    pub error: String,
}

/// What it takes to reverse a finished operation.
#[derive(Debug, Clone)]
pub enum Undo {
    /// Move objects back: (bucket, current name, generation, original bucket, original name).
    MoveBack(Vec<(String, String, Option<String>, String, String)>),
    /// Rename folders back: (bucket, current folder, original folder).
    RenameFoldersBack(Vec<(String, String, String)>),
    /// Restore soft-deleted objects: (bucket, name, generation).
    Restore(Vec<(String, String, String)>),
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct OpResult {
    pub kind: Option<OpKind>,
    pub total: u64,
    pub done: u64,
    /// Left alone because the destination already existed.
    pub skipped: u64,
    pub failed: u64,
    pub failures: Vec<Failure>,
    pub cancelled: bool,
    pub undoable: bool,
    pub elapsed_seconds: f64,
    #[serde(skip)]
    pub undo: Option<Undo>,
}

// ----- planning ---------------------------------------------------------------------------

const PLAN_FIELDS: &str = "name,size,crc32c,generation,storageClass,timeCreated,timeStorageClassUpdated,temporaryHold,\
eventBasedHold,kmsKeyName,contentType,contentEncoding,contentDisposition,contentLanguage,cacheControl,metadata,customTime";

fn kept_metadata(raw: &serde_json::Value) -> serde_json::Value {
    let mut out = serde_json::Map::new();
    for key in KEPT_METADATA {
        if let Some(value) = raw.get(*key).filter(|v| !v.is_null()) {
            out.insert((*key).to_string(), value.clone());
        }
    }
    serde_json::Value::Object(out)
}

fn task_from(bucket: &str, raw: serde_json::Value) -> Result<(Task, ObjectInfo)> {
    let metadata = kept_metadata(&raw);
    let info: ObjectInfo = serde_json::from_value(raw)?;
    let task = Task {
        bucket: bucket.to_string(),
        name: info.name.clone(),
        generation: info.generation.clone(),
        size: info.size,
        crc32c: info.crc32c.clone(),
        kms_key: info.kms_key_name.clone(),
        dst: None,
        metadata,
    };
    Ok((task, info))
}

async fn list_all_raw(client: &GcsClient, bucket: &str, prefix: &str, fields: &str) -> Result<Vec<serde_json::Value>> {
    let mut out = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let (page, next) = client.list_raw(bucket, prefix, token.as_deref(), fields, false).await?;
        out.extend(page);
        match next {
            Some(next) => token = Some(next),
            None => return Ok(out),
        }
    }
}

/// Every object an item covers: a folder recursively (including its own placeholder object,
/// which the folder listing hides), a file itself.
async fn expand(client: &GcsClient, bucket: &str, item: &Item) -> Result<Vec<(Task, ObjectInfo)>> {
    if item.is_folder() {
        let prefix = folder_prefix(&item.path);
        if prefix.is_empty() {
            bail!("Pick a folder inside the bucket, not the whole bucket");
        }
        list_all_raw(client, bucket, &prefix, PLAN_FIELDS).await?.into_iter().map(|raw| task_from(bucket, raw)).collect()
    } else {
        let raw = client.get_object_raw(bucket, &item.path).await.map_err(|e| anyhow!("{e}"))?;
        let raw = raw.ok_or_else(|| anyhow!("{} no longer exists", item.path))?;
        Ok(vec![task_from(bucket, raw)?])
    }
}

/// Minimum storage duration in days for a storage class (Google's early deletion rules).
fn min_storage_days(class: &str) -> u32 {
    match class {
        "NEARLINE" => 30,
        "COLDLINE" => 90,
        "ARCHIVE" => 365,
        _ => 0,
    }
}

/// Seconds since the Unix epoch for an RFC 3339 UTC timestamp like "2026-01-02T03:04:05.678Z".
pub fn parse_rfc3339(value: &str) -> Option<i64> {
    let (date, time) = value.split_once('T')?;
    let mut d = date.split('-').map(|p| p.parse::<i64>());
    let (y, m, day) = (d.next()?.ok()?, d.next()?.ok()?, d.next()?.ok()?);
    let time = time.trim_end_matches('Z').split('+').next()?;
    let mut t = time.split(':');
    let (hh, mm) = (t.next()?.parse::<i64>().ok()?, t.next()?.parse::<i64>().ok()?);
    let ss = t.next()?.split('.').next()?.parse::<i64>().ok()?;
    // Days from civil (Howard Hinnant's algorithm).
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(days * 86_400 + hh * 3600 + mm * 60 + ss)
}

fn now_secs() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

fn early_deletion<'a>(objects: impl Iterator<Item = &'a ObjectInfo>) -> Vec<EarlyDeletion> {
    let now = now_secs();
    let mut by_class: BTreeMap<String, EarlyDeletion> = BTreeMap::new();
    for info in objects {
        let class = info.storage_class.clone().unwrap_or_default();
        let min_days = min_storage_days(&class);
        if min_days == 0 {
            continue;
        }
        // The clock starts when the object entered its current class.
        let since = info.time_storage_class_updated.as_deref().or(info.time_created.as_deref()).and_then(parse_rfc3339);
        // Unknown age: assume young; a warning is cheaper than a surprise bill.
        if since.is_none_or(|s| now - s < i64::from(min_days) * 86_400) {
            let entry = by_class.entry(class.clone()).or_insert_with(|| EarlyDeletion { storage_class: class, min_days, ..Default::default() });
            entry.objects += 1;
            entry.bytes += info.size;
        }
    }
    by_class.into_values().collect()
}

fn is_held(info: &ObjectInfo) -> bool {
    info.temporary_hold == Some(true) || info.event_based_hold == Some(true)
}

/// Plan a copy or move of `items` from `bucket` into `dst_bucket`/`dst_prefix`. With
/// `rename_to` (single item only), the item gets that name instead of its own: a rename is a
/// move into the same folder under a new name.
#[allow(clippy::too_many_arguments)]
pub async fn plan_transfer(
    client: &GcsClient,
    source: &BucketInfo,
    items: &[Item],
    dst_bucket: &str,
    dst_prefix: &str,
    rename_to: Option<&str>,
    delete_source: bool,
) -> Result<Plan> {
    let bucket = source.name.as_str();
    if items.is_empty() {
        bail!("Nothing selected");
    }
    if rename_to.is_some() && items.len() != 1 {
        bail!("Rename one item at a time");
    }
    if let Some(name) = rename_to {
        validate_name(name)?;
    }
    let dst_prefix = folder_prefix(dst_prefix);
    let kind = if delete_source { OpKind::Move } else { OpKind::Copy };
    let mut plan = Plan {
        kind,
        summary: PlanSummary { kind: Some(kind), ..Default::default() },
        tasks: Vec::new(),
        existing: HashMap::new(),
        folder_renames: Vec::new(),
        folder_cleanup: Vec::new(),
    };
    let mut removed: Vec<ObjectInfo> = Vec::new();

    for item in items {
        let name = rename_to.unwrap_or_else(|| item.base_name());
        let target = if item.is_folder() { format!("{dst_prefix}{name}/") } else { format!("{dst_prefix}{name}") };
        let same_bucket = dst_bucket == bucket;
        if same_bucket && target == item.path {
            bail!("\"{}\" is already there", item.base_name());
        }
        if same_bucket && item.is_folder() && target.starts_with(&item.path) {
            bail!("Can't put \"{}\" inside itself", item.base_name());
        }

        // Hierarchical namespace: Google renames the folder in one step, no copies.
        if item.is_folder() && delete_source && same_bucket && source.hierarchical_namespace {
            if !client.list_raw(bucket, &target, None, "name", false).await?.0.is_empty()
                || client.list_folders(bucket, &target).await.map(|f| !f.is_empty()).unwrap_or(false)
            {
                bail!("A folder named \"{name}\" already exists there");
            }
            plan.folder_renames.push((bucket.to_string(), item.path.clone(), target.clone()));
            plan.summary.folder_renames += 1;
            plan.summary.example.get_or_insert(format!("gs://{dst_bucket}/{target}"));
            continue;
        }

        // What's already at the destination, by name, with its checksum.
        let mut there: HashMap<String, (Option<String>, u64)> = HashMap::new();
        let found = if item.is_folder() {
            list_all_raw(client, dst_bucket, &target, "name,crc32c,size").await?
        } else {
            client.get_object_raw(dst_bucket, &target).await.map_err(|e| anyhow!("{e}"))?.into_iter().collect()
        };
        for raw in found {
            if let Ok(info) = serde_json::from_value::<ObjectInfo>(raw) {
                there.insert(info.name, (info.crc32c, info.size));
            }
        }
        // A rename never merges into (or replaces) something that already has the new name.
        if rename_to.is_some() && !there.is_empty() {
            bail!("Something named \"{name}\" already exists here");
        }

        for (mut task, info) in expand(client, bucket, item).await? {
            let dst = if item.is_folder() { format!("{target}{}", &task.name[item.path.len()..]) } else { target.clone() };
            if let Some((crc, size)) = there.get(&dst) {
                let same = crc.is_some() && *crc == task.crc32c && *size == task.size;
                if same && delete_source {
                    plan.existing.insert(dst.clone(), Existing::SameContent);
                    plan.summary.already_copied += 1;
                } else {
                    plan.existing.insert(dst.clone(), Existing::Conflict);
                    plan.summary.conflicts += 1;
                    if plan.summary.conflict_examples.len() < 5 {
                        plan.summary.conflict_examples.push(dst.clone());
                    }
                }
            }
            plan.summary.example.get_or_insert(format!("gs://{dst_bucket}/{dst}"));
            plan.summary.objects += 1;
            plan.summary.bytes += task.size;
            if delete_source && is_held(&info) {
                plan.summary.held += 1;
            }
            task.dst = Some((dst_bucket.to_string(), dst));
            plan.tasks.push(task);
            if delete_source {
                removed.push(info);
            }
        }
        if item.is_folder() && delete_source && source.hierarchical_namespace {
            plan.folder_cleanup.push((bucket.to_string(), item.path.clone()));
        }
    }
    if !source.autoclass {
        plan.summary.early_deletion = early_deletion(removed.iter());
    }
    Ok(plan)
}

/// Plan deleting `items` (folders recursively).
pub async fn plan_delete(client: &GcsClient, source: &BucketInfo, items: &[Item]) -> Result<Plan> {
    if items.is_empty() {
        bail!("Nothing selected");
    }
    let bucket = source.name.as_str();
    let mut tasks = Vec::new();
    let mut infos = Vec::new();
    let mut folder_cleanup = Vec::new();
    for item in items {
        for (task, info) in expand(client, bucket, item).await? {
            tasks.push(task);
            infos.push(info);
        }
        if item.is_folder() && source.hierarchical_namespace {
            folder_cleanup.push((bucket.to_string(), item.path.clone()));
        }
    }
    let summary = PlanSummary {
        kind: Some(OpKind::Delete),
        objects: tasks.len() as u64,
        bytes: tasks.iter().map(|t| t.size).sum(),
        held: infos.iter().filter(|i| is_held(i)).count() as u64,
        early_deletion: if source.autoclass { Vec::new() } else { early_deletion(infos.iter()) },
        ..Default::default()
    };
    Ok(Plan { kind: OpKind::Delete, summary, tasks, existing: HashMap::new(), folder_renames: Vec::new(), folder_cleanup })
}

/// Plan restoring soft-deleted objects (name + generation, as listed by `list_deleted`).
pub fn plan_restore(bucket: &str, objects: &[(String, String)]) -> Plan {
    let tasks: Vec<Task> = objects
        .iter()
        .map(|(name, generation)| Task {
            bucket: bucket.to_string(),
            name: name.clone(),
            generation: Some(generation.clone()),
            size: 0,
            crc32c: None,
            kms_key: None,
            dst: None,
            metadata: serde_json::Value::Null,
        })
        .collect();
    let summary = PlanSummary { kind: Some(OpKind::Restore), objects: tasks.len() as u64, ..Default::default() };
    Plan { kind: OpKind::Restore, summary, tasks, existing: HashMap::new(), folder_renames: Vec::new(), folder_cleanup: Vec::new() }
}

/// Plan reversing a finished operation.
pub fn plan_undo(undo: &Undo) -> Plan {
    let empty = |kind: OpKind| Plan {
        kind,
        summary: PlanSummary { kind: Some(kind), ..Default::default() },
        tasks: Vec::new(),
        existing: HashMap::new(),
        folder_renames: Vec::new(),
        folder_cleanup: Vec::new(),
    };
    match undo {
        Undo::MoveBack(moves) => {
            let mut plan = empty(OpKind::Move);
            plan.tasks = moves
                .iter()
                .map(|(bucket, name, generation, orig_bucket, orig_name)| Task {
                    bucket: bucket.clone(),
                    name: name.clone(),
                    generation: generation.clone(),
                    size: 0,
                    crc32c: None,
                    kms_key: None,
                    dst: Some((orig_bucket.clone(), orig_name.clone())),
                    metadata: serde_json::Value::Null,
                })
                .collect();
            plan.summary.objects = plan.tasks.len() as u64;
            plan
        }
        Undo::RenameFoldersBack(renames) => {
            let mut plan = empty(OpKind::Move);
            plan.folder_renames = renames.clone();
            plan.summary.folder_renames = renames.len() as u64;
            plan
        }
        Undo::Restore(objects) => {
            let bucket = objects.first().map(|o| o.0.clone()).unwrap_or_default();
            let pairs: Vec<(String, String)> = objects.iter().map(|(_, n, g)| (n.clone(), g.clone())).collect();
            plan_restore(&bucket, &pairs)
        }
    }
}

/// Characters that make an object name awkward or invalid.
pub fn validate_name(name: &str) -> Result<()> {
    if name.trim().is_empty() {
        bail!("Name can't be empty");
    }
    if name.trim() != name {
        bail!("Name can't start or end with spaces");
    }
    if name.contains('/') {
        bail!("Name can't contain /");
    }
    if name == "." || name == ".." {
        bail!("Name can't be . or ..");
    }
    if name.chars().any(|c| c.is_control()) {
        bail!("Name can't contain control characters");
    }
    if name.len() > 512 {
        bail!("Name is too long");
    }
    Ok(())
}

// ----- running ----------------------------------------------------------------------------

/// Retry rate limits (429), server errors and network failures with backoff. Writing the
/// same object name again within a second (an immediate undo) also hits 429 and is retried.
async fn retry<T, F, Fut>(mut f: F) -> Result<T, GcsError>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T, GcsError>>,
{
    let mut attempt = 0;
    loop {
        match f().await {
            Err(GcsError::Retryable(_)) if attempt + 1 < MAX_ATTEMPTS => {
                attempt += 1;
                let jitter = rand::random::<u64>() % 250;
                tokio::time::sleep(Duration::from_millis(500 * 2u64.pow(attempt - 1) + jitter)).await;
            }
            other => return other,
        }
    }
}

enum Outcome {
    Done(Option<UndoStep>),
    Skipped,
}

enum UndoStep {
    MovedTo(String, String, Option<String>, String, String),
    Deleted(String, String, String),
}

async fn delete_original(client: &GcsClient, task: &Task) -> Result<(), String> {
    retry(|| client.delete_object(&task.bucket, &task.name, task.generation.as_deref())).await.map_err(|e| e.to_string())
}

async fn run_task(client: &GcsClient, kind: OpKind, task: &Task, soft_delete: bool, existing: Option<Existing>) -> Result<Outcome, String> {
    match kind {
        OpKind::Copy | OpKind::Move => {
            let (dst_bucket, dst) = task.dst.clone().ok_or("no destination")?;
            match existing {
                Some(Existing::Conflict) => return Ok(Outcome::Skipped),
                Some(Existing::SameContent) => {
                    delete_original(client, task).await?;
                    return Ok(Outcome::Done(None));
                }
                None => {}
            }
            let (metadata, kms_key, expected_crc, expected_size) = if task.metadata.is_null() {
                // Undo: read the object as it is now.
                let raw = client.get_object_raw(&task.bucket, &task.name).await.map_err(|e| e.to_string())?.ok_or("no longer exists")?;
                let info: ObjectInfo = serde_json::from_value(raw.clone()).map_err(|e| e.to_string())?;
                (kept_metadata(&raw), info.kms_key_name, info.crc32c, info.size)
            } else {
                (task.metadata.clone(), task.kms_key.clone(), task.crc32c.clone(), task.size)
            };
            let copy = retry(|| {
                client.rewrite(&task.bucket, &task.name, task.generation.as_deref(), &dst_bucket, &dst, &metadata, true, kms_key.as_deref())
            })
            .await;
            let copy = match copy {
                Ok(copy) => copy,
                Err(GcsError::AlreadyExists) => {
                    // A 412: either the destination appeared, or the original changed since listing.
                    return match client.get_object(&dst_bucket, &dst).await {
                        Ok(Some(_)) => Ok(Outcome::Skipped),
                        _ => Err("changed since it was listed; left in place".into()),
                    };
                }
                Err(err) => return Err(err.to_string()),
            };
            // Verify before anything is deleted.
            let crc_ok = match (&expected_crc, &copy.crc32c) {
                (Some(expected), Some(actual)) => expected == actual,
                _ => true,
            };
            if !crc_ok || copy.size != expected_size {
                return Err(format!("the copy at gs://{dst_bucket}/{dst} didn't match the original; original kept"));
            }
            if kind == OpKind::Copy {
                return Ok(Outcome::Done(None));
            }
            delete_original(client, task)
                .await
                .map_err(|e| format!("copied to gs://{dst_bucket}/{dst}, but the original couldn't be removed: {e}"))?;
            Ok(Outcome::Done(Some(UndoStep::MovedTo(dst_bucket, dst, copy.generation, task.bucket.clone(), task.name.clone()))))
        }
        OpKind::Delete => {
            delete_original(client, task).await?;
            Ok(Outcome::Done(match (&task.generation, soft_delete) {
                (Some(generation), true) => Some(UndoStep::Deleted(task.bucket.clone(), task.name.clone(), generation.clone())),
                _ => None,
            }))
        }
        OpKind::Restore => {
            let generation = task.generation.clone().ok_or("no generation")?;
            match retry(|| client.restore_object(&task.bucket, &task.name, &generation)).await {
                Ok(_) => Ok(Outcome::Done(None)),
                // ifGenerationMatch=0: a live object with this name exists.
                Err(GcsError::AlreadyExists) => Ok(Outcome::Skipped),
                Err(err) => Err(err.to_string()),
            }
        }
    }
}

/// Remove empty hierarchical-namespace folders under `prefix`, deepest first.
async fn cleanup_folders(client: &GcsClient, bucket: &str, prefix: &str) -> Result<(), String> {
    let mut folders = client.list_folders(bucket, prefix).await.map_err(|e| format!("{e:#}"))?;
    if !folders.iter().any(|f| f == prefix) {
        folders.push(prefix.to_string());
    }
    folders.sort_by_key(|f| std::cmp::Reverse(f.matches('/').count()));
    for folder in folders {
        retry(|| client.delete_folder(bucket, &folder)).await.map_err(|e| format!("folder gs://{bucket}/{folder}: {e}"))?;
    }
    Ok(())
}

/// Run a plan. `soft_delete` (the bucket keeps deleted objects) makes deletes undoable.
pub async fn run(client: &GcsClient, plan: &Plan, soft_delete: bool, progress: &OpProgress) -> OpResult {
    let started = Instant::now();
    let failures = Mutex::new(Vec::new());
    let fail = |path: String, error: String| {
        progress.failed.fetch_add(1, Ordering::Relaxed);
        let mut list = failures.lock().unwrap();
        if list.len() < MAX_REPORTED_FAILURES {
            list.push(Failure { path, error });
        }
    };
    let mut renamed = Vec::new();
    for (bucket, from, to) in &plan.folder_renames {
        if progress.cancel.load(Ordering::Relaxed) {
            break;
        }
        match retry(|| client.rename_folder(bucket, from, to)).await {
            Ok(()) => {
                progress.done.fetch_add(1, Ordering::Relaxed);
                renamed.push((bucket.clone(), to.clone(), from.clone()));
            }
            Err(GcsError::AlreadyExists) => fail(format!("gs://{bucket}/{from}"), format!("a folder already exists at {to}")),
            Err(err) => fail(format!("gs://{bucket}/{from}"), err.to_string()),
        }
    }

    let undo_steps = Mutex::new(Vec::new());
    let kind = plan.kind;
    stream::iter(plan.tasks.iter())
        .for_each_concurrent(CONCURRENCY, |task| {
            let (undo_steps, fail) = (&undo_steps, &fail);
            async move {
                if progress.cancel.load(Ordering::Relaxed) {
                    return;
                }
                let existing = task.dst.as_ref().and_then(|(_, name)| plan.existing.get(name).copied());
                match run_task(client, kind, task, soft_delete, existing).await {
                    Ok(Outcome::Done(step)) => {
                        progress.done.fetch_add(1, Ordering::Relaxed);
                        progress.bytes.fetch_add(task.size, Ordering::Relaxed);
                        if let Some(step) = step {
                            undo_steps.lock().unwrap().push(step);
                        }
                    }
                    Ok(Outcome::Skipped) => {
                        progress.skipped.fetch_add(1, Ordering::Relaxed);
                    }
                    Err(error) => fail(format!("gs://{}/{}", task.bucket, task.name), error),
                }
            }
        })
        .await;

    if !progress.cancel.load(Ordering::Relaxed) && progress.failed.load(Ordering::Relaxed) == 0 {
        for (bucket, prefix) in &plan.folder_cleanup {
            if let Err(error) = cleanup_folders(client, bucket, prefix).await {
                fail(format!("gs://{bucket}/{prefix}"), format!("objects removed, but the empty folder remains: {error}"));
            }
        }
    }

    let steps = undo_steps.into_inner().unwrap();
    let undo = if steps.len() > MAX_UNDO_OBJECTS {
        None
    } else if !renamed.is_empty() && steps.is_empty() {
        Some(Undo::RenameFoldersBack(renamed))
    } else if !steps.is_empty() && renamed.is_empty() {
        match kind {
            OpKind::Move => Some(Undo::MoveBack(
                steps
                    .into_iter()
                    .filter_map(|s| match s {
                        UndoStep::MovedTo(b, n, g, ob, on) => Some((b, n, g, ob, on)),
                        UndoStep::Deleted(..) => None,
                    })
                    .collect(),
            )),
            OpKind::Delete => Some(Undo::Restore(
                steps
                    .into_iter()
                    .filter_map(|s| match s {
                        UndoStep::Deleted(b, n, g) => Some((b, n, g)),
                        UndoStep::MovedTo(..) => None,
                    })
                    .collect(),
            )),
            _ => None,
        }
    } else {
        None
    };
    OpResult {
        kind: Some(kind),
        total: plan.tasks.len() as u64 + plan.folder_renames.len() as u64,
        done: progress.done.load(Ordering::Relaxed),
        skipped: progress.skipped.load(Ordering::Relaxed),
        failed: progress.failed.load(Ordering::Relaxed),
        failures: failures.into_inner().unwrap(),
        cancelled: progress.cancel.load(Ordering::Relaxed),
        undoable: undo.is_some(),
        elapsed_seconds: started.elapsed().as_secs_f64(),
        undo,
    }
}

// ----- small operations -------------------------------------------------------------------

/// Create an empty folder: a real folder in hierarchical-namespace buckets, otherwise a
/// zero-byte "name/" placeholder object, as the Cloud Console does.
pub async fn new_folder(client: &GcsClient, source: &BucketInfo, parent: &str, name: &str) -> Result<String> {
    validate_name(name)?;
    let bucket = source.name.as_str();
    let path = format!("{}{name}/", folder_prefix(parent));
    if !client.list_raw(bucket, &path, None, "name", false).await?.0.is_empty() {
        bail!("A folder named \"{name}\" already exists");
    }
    let created = if source.hierarchical_namespace {
        client.create_folder(bucket, &path).await
    } else {
        client.upload_multipart(bucket, &path, Vec::new(), crc32c::crc32c(&[]), true).await.map(|_| ())
    };
    match created {
        Ok(()) => Ok(path),
        Err(GcsError::AlreadyExists) => bail!("A folder named \"{name}\" already exists"),
        Err(err) => bail!("{err}"),
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct DeletedPage {
    pub objects: Vec<ObjectInfo>,
    pub next_page_token: Option<String>,
}

/// Soft-deleted objects under a prefix (recursively), most recently deleted first within a page.
/// One row per deleted version, so a name can appear more than once.
pub async fn list_deleted(client: &GcsClient, bucket: &str, prefix: &str, page_token: Option<&str>) -> Result<DeletedPage> {
    let (page, next) = client
        .list_raw(
            bucket,
            &folder_prefix(prefix),
            page_token,
            "name,size,generation,storageClass,updated,timeCreated,softDeleteTime,hardDeleteTime,contentType",
            true,
        )
        .await?;
    let mut objects: Vec<ObjectInfo> = page.into_iter().filter_map(|raw| serde_json::from_value(raw).ok()).collect();
    objects.sort_by(|a, b| b.soft_delete_time.cmp(&a.soft_delete_time));
    Ok(DeletedPage { objects, next_page_token: next })
}

#[derive(Debug, Clone, Serialize)]
pub struct BucketAccess {
    #[serde(flatten)]
    pub bucket: BucketInfo,
    /// The bucket's settings could be read (users with object-only roles often can't).
    pub known: bool,
    /// Granted permissions, or `None` if they couldn't be checked (treat as allowed).
    pub permissions: Option<Vec<String>>,
}

/// Bucket settings and the user's permissions. Never fails: anything unreadable is "unknown".
pub async fn bucket_access(client: &GcsClient, bucket: &str) -> BucketAccess {
    let (info, permissions) = tokio::join!(client.get_bucket(bucket), client.test_permissions(bucket, PERMISSIONS));
    let known = info.is_ok();
    BucketAccess {
        bucket: info.unwrap_or_else(|_| BucketInfo { name: bucket.to_string(), ..Default::default() }),
        known,
        permissions: permissions.ok(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stamp(secs: i64) -> String {
        // Inverse of parse_rfc3339 for tests.
        let mut day = secs.div_euclid(86_400);
        let rem = secs.rem_euclid(86_400);
        let (mut y, mut m) = (1970, 1);
        let month_len = |y: i64, m: i64| match m {
            2 if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 => 29,
            2 => 28,
            4 | 6 | 9 | 11 => 30,
            _ => 31,
        };
        while day >= month_len(y, m) {
            day -= month_len(y, m);
            m += 1;
            if m > 12 {
                m = 1;
                y += 1;
            }
        }
        format!("{y:04}-{m:02}-{:02}T{:02}:{:02}:{:02}.123Z", day + 1, rem / 3600, rem % 3600 / 60, rem % 60)
    }

    #[test]
    fn parses_timestamps() {
        assert_eq!(parse_rfc3339("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_rfc3339("2026-01-02T03:04:05.678Z"), Some(1_767_323_045));
        assert_eq!(parse_rfc3339("2000-02-29T12:00:00Z"), Some(951_825_600));
        assert_eq!(parse_rfc3339(&stamp(1_234_567_890)), Some(1_234_567_890));
        assert_eq!(parse_rfc3339("garbage"), None);
    }

    #[test]
    fn early_deletion_counts_young_cold_objects_from_their_class_change() {
        let now = now_secs();
        let obj = |class: &str, created_days_ago: i64, class_days_ago: Option<i64>, size: u64| ObjectInfo {
            name: "x".into(),
            size,
            storage_class: Some(class.into()),
            time_created: Some(stamp(now - created_days_ago * 86_400)),
            time_storage_class_updated: class_days_ago.map(|d| stamp(now - d * 86_400)),
            ..Default::default()
        };
        let objects = [
            obj("STANDARD", 1, None, 10),
            obj("NEARLINE", 10, None, 20),
            obj("NEARLINE", 40, None, 40),
            obj("ARCHIVE", 400, Some(200), 80), // old object, recently moved to Archive
            obj("COLDLINE", 10, Some(100), 160),
        ];
        let fees = early_deletion(objects.iter());
        assert_eq!(fees.len(), 2);
        assert_eq!((fees[0].storage_class.as_str(), fees[0].objects, fees[0].bytes), ("ARCHIVE", 1, 80));
        assert_eq!((fees[1].storage_class.as_str(), fees[1].objects, fees[1].bytes), ("NEARLINE", 1, 20));
    }

    #[test]
    fn names_are_validated() {
        assert!(validate_name("survey 2026").is_ok());
        assert!(validate_name("").is_err());
        assert!(validate_name("a/b").is_err());
        assert!(validate_name(" padded").is_err());
        assert!(validate_name("..").is_err());
    }

    #[test]
    fn item_names() {
        assert_eq!(Item { path: "a/b/".into() }.base_name(), "b");
        assert_eq!(Item { path: "a/b/c.jpg".into() }.base_name(), "c.jpg");
        assert!(Item { path: "a/".into() }.is_folder());
    }

    #[test]
    fn kept_metadata_passes_through_known_fields_only() {
        let raw = serde_json::json!({
            "name": "x", "contentType": "image/jpeg", "metadata": {"survey": "kona"},
            "storageClass": "COLDLINE", "cacheControl": null, "etag": "abc"
        });
        assert_eq!(
            kept_metadata(&raw),
            serde_json::json!({"contentType": "image/jpeg", "metadata": {"survey": "kona"}, "storageClass": "COLDLINE"})
        );
    }
}
