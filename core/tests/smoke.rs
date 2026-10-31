mod common;
use common::*;
use std::time::Duration;
use sushload_core::Status;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn segmented_download_completes_with_exact_bytes() {
    let server = start_server().await;
    let dirs = Dirs::new();
    let mut settings = dirs.settings();
    settings.connections = 8;
    let engine = dirs.engine(settings);

    let id = engine.add(&server.url("big.bin")).expect("add download");
    let v = wait_completed(&engine, id, Duration::from_secs(20)).await;

    assert_eq!(v.total, Some(BODY_LEN as u64));
    assert_eq!(v.received, BODY_LEN as u64);
    assert_eq!(v.segments.len(), 8, "expected eight segment connections");
    assert_eq!(v.status, Status::Completed);
    assert_file_matches(&v, &deterministic_body(BODY_LEN), BODY_LEN);
    assert!(!engine.has_active());
    engine.shutdown();
}
