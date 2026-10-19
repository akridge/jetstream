//! File-manager calls fake-gcs-server doesn't support (soft delete and restore, permission
//! checks, hierarchical-namespace folders), against a small in-process server that answers
//! as Cloud Storage documents. The tests check the requests sent and how replies are handled.

use std::sync::{Arc, Mutex};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use jetstream_transfer::auth::TokenProvider;
use jetstream_transfer::fileops::{self, Item, OpProgress, Undo};
use jetstream_transfer::gcs::GcsClient;

type Handler = dyn Fn(&str, &str) -> (u16, String) + Send + Sync;

/// Serves `handler(method, path_and_query)`; records every request line.
async fn mock(handler: Arc<Handler>) -> (GcsClient, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let log = Arc::new(Mutex::new(Vec::new()));
    let server_log = log.clone();
    tokio::spawn(async move {
        loop {
            let (mut socket, _) = listener.accept().await.unwrap();
            let (handler, log) = (handler.clone(), server_log.clone());
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let mut chunk = [0u8; 8192];
                // Read headers, then any body (only its length matters here).
                let end = loop {
                    let n = socket.read(&mut chunk).await.unwrap_or(0);
                    if n == 0 {
                        return;
                    }
                    buf.extend_from_slice(&chunk[..n]);
                    if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                        break pos + 4;
                    }
                };
                let head = String::from_utf8_lossy(&buf[..end]).to_string();
                let length: usize = head
                    .lines()
                    .find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse().unwrap_or(0)))
                    .unwrap_or(0);
                while buf.len() < end + length {
                    let n = socket.read(&mut chunk).await.unwrap_or(0);
                    if n == 0 {
                        break;
                    }
                    buf.extend_from_slice(&chunk[..n]);
                }
                let line = head.lines().next().unwrap_or_default();
                let mut parts = line.split(' ');
                let (method, target) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));
                log.lock().unwrap().push(format!("{method} {target}"));
                let (status, body) = handler(method, target);
                let reason = match status {
                    200 => "OK",
                    204 => "No Content",
                    404 => "Not Found",
                    412 => "Precondition Failed",
                    _ => "Other",
                };
                let response = format!(
                    "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = socket.write_all(response.as_bytes()).await;
                let _ = socket.shutdown().await;
            });
        }
    });
    let http = GcsClient::http_client();
    let client = GcsClient::with_base(http.clone(), &base, Arc::new(TokenProvider::none(http)));
    (client, log)
}

#[tokio::test]
async fn bucket_access_reads_settings_and_permissions_and_degrades() {
    let (client, log) = mock(Arc::new(|_, target: &str| {
        if target.starts_with("/storage/v1/b/bk/iam/testPermissions") {
            (200, r#"{"permissions":["storage.objects.list","storage.objects.get"]}"#.into())
        } else if target.starts_with("/storage/v1/b/bk?") {
            (200, r#"{"name":"bk","softDeletePolicy":{"retentionDurationSeconds":"604800"},"hierarchicalNamespace":{"enabled":true},"autoclass":{"enabled":true}}"#.into())
        } else {
            (403, r#"{"error":{"message":"no storage.buckets.get"}}"#.into())
        }
    }))
    .await;
    let access = fileops::bucket_access(&client, "bk").await;
    assert!(access.known);
    assert_eq!(access.bucket.soft_delete_seconds, 604_800);
    assert!(access.bucket.hierarchical_namespace && access.bucket.autoclass);
    assert_eq!(access.permissions.unwrap(), ["storage.objects.list", "storage.objects.get"]);
    let asked = log.lock().unwrap().iter().find(|l| l.contains("testPermissions")).cloned().unwrap();
    assert!(asked.contains("permissions=storage.objects.delete") && asked.contains("permissions=storage.objects.restore"));

    // Object-only roles can't read the bucket: everything becomes "unknown", not an error.
    let other = fileops::bucket_access(&client, "other").await;
    assert!(!other.known && other.permissions.is_none());
    assert_eq!(other.bucket.name, "other");
}

#[tokio::test]
async fn delete_with_soft_delete_is_undone_by_restoring_each_generation() {
    let (client, log) = mock(Arc::new(|method: &str, target: &str| match method {
        "GET" if target.starts_with("/storage/v1/b/bk/o/notes%2Ff.txt") => {
            (200, r#"{"name":"notes/f.txt","generation":"7","size":"3","crc32c":"AAAAAA=="}"#.into())
        }
        "DELETE" => (204, String::new()),
        "POST" if target.contains("/restore?") => (200, r#"{"name":"notes/f.txt","generation":"9"}"#.into()),
        _ => (404, String::new()),
    }))
    .await;
    let bucket = jetstream_transfer::gcs::BucketInfo { name: "bk".into(), soft_delete_seconds: 604_800, ..Default::default() };
    let plan = fileops::plan_delete(&client, &bucket, &[Item { path: "notes/f.txt".into() }]).await.unwrap();
    let result = fileops::run(&client, &plan, true, &OpProgress::default()).await;
    assert_eq!((result.done, result.undoable), (1, true));
    assert!(matches!(&result.undo, Some(Undo::Restore(list)) if list[0] == ("bk".into(), "notes/f.txt".into(), "7".into())));

    let undone = fileops::run(&client, &fileops::plan_undo(result.undo.as_ref().unwrap()), true, &OpProgress::default()).await;
    assert_eq!(undone.done, 1);
    let log = log.lock().unwrap();
    assert!(log.iter().any(|l| l == "DELETE /storage/v1/b/bk/o/notes%2Ff.txt?ifGenerationMatch=7"), "{log:?}");
    assert!(
        log.iter().any(|l| l == "POST /storage/v1/b/bk/o/notes%2Ff.txt/restore?generation=7&ifGenerationMatch=0"),
        "{log:?}"
    );
}

#[tokio::test]
async fn restore_never_replaces_a_live_object_and_lists_deleted_newest_first() {
    let (client, log) = mock(Arc::new(|method: &str, target: &str| match method {
        "GET" if target.contains("softDeleted=true") => (
            200,
            r#"{"items":[
                {"name":"a/old.txt","generation":"1","size":"5","softDeleteTime":"2026-10-01T00:00:00Z","hardDeleteTime":"2026-10-08T00:00:00Z"},
                {"name":"a/new.txt","generation":"2","size":"6","softDeleteTime":"2026-10-18T00:00:00Z","hardDeleteTime":"2026-10-25T00:00:00Z"}
            ]}"#
            .into(),
        ),
        "POST" if target.contains("old.txt/restore") => (412, r#"{"error":{"message":"precondition"}}"#.into()),
        "POST" if target.contains("/restore") => (200, r#"{"name":"a/new.txt","generation":"3"}"#.into()),
        _ => (404, String::new()),
    }))
    .await;
    let page = fileops::list_deleted(&client, "bk", "a", None).await.unwrap();
    assert_eq!(page.objects.iter().map(|o| o.name.as_str()).collect::<Vec<_>>(), ["a/new.txt", "a/old.txt"]);
    assert_eq!(page.objects[0].hard_delete_time.as_deref(), Some("2026-10-25T00:00:00Z"));
    assert!(log.lock().unwrap()[0].contains("prefix=a%2F"));

    let plan = fileops::plan_restore("bk", &[("a/old.txt".into(), "1".into()), ("a/new.txt".into(), "2".into())]);
    let result = fileops::run(&client, &plan, true, &OpProgress::default()).await;
    assert_eq!((result.done, result.skipped, result.failed), (1, 1, 0), "a live a/old.txt exists, so it's skipped");
}

#[tokio::test]
async fn hierarchical_namespace_renames_folders_in_one_operation() {
    let (client, log) = mock(Arc::new(|method: &str, target: &str| match method {
        "POST" if target.contains("/renameTo/folders/") => {
            (200, r#"{"name":"projects/_/buckets/bk/operations/op-1","done":false}"#.into())
        }
        "GET" if target.starts_with("/storage/v1/b/bk/operations/op-1") => (200, r#"{"name":"op-1","done":true}"#.into()),
        "GET" if target.starts_with("/storage/v1/b/bk/folders") => (200, r#"{"items":[]}"#.into()),
        "GET" if target.starts_with("/storage/v1/b/bk/o?") => (200, r#"{"items":[]}"#.into()),
        "POST" if target.starts_with("/storage/v1/b/bk/folders?recursive=true") => (200, r#"{"name":"data/new/"}"#.into()),
        _ => (404, String::new()),
    }))
    .await;
    let bucket = jetstream_transfer::gcs::BucketInfo { name: "bk".into(), hierarchical_namespace: true, ..Default::default() };
    let plan = fileops::plan_transfer(&client, &bucket, &[Item { path: "data/photos/".into() }], "bk", "data/", Some("pics"), true)
        .await
        .unwrap();
    assert_eq!((plan.summary.folder_renames, plan.summary.objects), (1, 0), "no per-object copies");
    let result = fileops::run(&client, &plan, false, &OpProgress::default()).await;
    assert_eq!((result.done, result.failed, result.undoable), (1, 0, true), "{:?}", result.failures);
    assert!(matches!(&result.undo, Some(Undo::RenameFoldersBack(r)) if r[0] == ("bk".into(), "data/pics/".into(), "data/photos/".into())));

    assert_eq!(fileops::new_folder(&client, &bucket, "data/", "new").await.unwrap(), "data/new/");
    let log = log.lock().unwrap();
    assert!(log.iter().any(|l| l == "POST /storage/v1/b/bk/folders/data%2Fphotos%2F/renameTo/folders/data%2Fpics%2F"), "{log:?}");
    assert!(log.iter().any(|l| l.starts_with("GET /storage/v1/b/bk/operations/op-1")));
    assert!(!log.iter().any(|l| l.contains("rewriteTo")));
}
