//! A connection that stops taking the request body must not hang an upload forever: the
//! request is abandoned (and retried by the caller) once nothing moves for the stall timeout.

use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::io::AsyncReadExt;
use tokio::net::TcpListener;

use jetstream_transfer::auth::TokenProvider;
use jetstream_transfer::gcs::{GcsClient, GcsError};

#[tokio::test]
async fn a_stalled_upload_is_abandoned_and_reported_as_retryable() {
    std::env::set_var("JETSTREAM_TRANSFER_STALL_SECS", "2");
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        // Read a little, then stop reading (like a dead NAT mapping) but keep the socket open.
        let mut buf = vec![0u8; 64 * 1024];
        let _ = socket.read(&mut buf).await;
        tokio::time::sleep(Duration::from_secs(60)).await;
        drop(socket);
    });
    let http = GcsClient::http_client();
    let client = GcsClient::with_base(http.clone(), &base, Arc::new(TokenProvider::none(http)));
    let data = vec![7u8; 64 * 1024 * 1024]; // far more than socket buffers hold
    let started = Instant::now();
    let result = client.upload_multipart("bucket", "big.bin", data, 1, false).await;
    let elapsed = started.elapsed();
    match result {
        Err(GcsError::Retryable(message)) => assert!(message.contains("stalled"), "{message}"),
        other => panic!("expected a retryable stall, got {:?}", other.map(|i| i.name)),
    }
    assert!(elapsed < Duration::from_secs(15), "gave up after {elapsed:?}");
}
