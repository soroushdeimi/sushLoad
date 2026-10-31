mod common;
use common::*;
use sushload_core::Status;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

const T20: Duration = Duration::from_secs(20);

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn server_without_range_uses_single_connection() {
    let server = start_server().await;
    server.control.ignore_range.store(true, Ordering::SeqCst);
    let dirs = Dirs::new();
    let engine = dirs.engine(dirs.settings());

    let id = engine.add(&server.url("norange.bin")).expect("add");
    let v = wait_completed(&engine, id, T20).await;

    assert!(!v.resumable);
    assert!(v.segments.is_empty(), "single connection has no segments");
    assert_eq!(v.received, BODY_LEN as u64);
    assert_file_matches(&v, &deterministic_body(BODY_LEN), BODY_LEN);
    // One probe plus one plain GET; no ranged segment requests succeeded.
    assert!(
        server.control.requests.load(Ordering::SeqCst) <= 3,
        "too many requests: {}",
        server.control.requests.load(Ordering::SeqCst)
    );
    engine.shutdown();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pause_shutdown_restart_resume_gives_exact_bytes() {
    let server = start_server().await;
    server.control.throttle_ms.store(15, Ordering::SeqCst);
    let dirs = Dirs::new();
    let engine = dirs.engine(dirs.settings());

    let id = engine.add(&server.url("resume.bin")).expect("add");
    let v = wait_for(&engine, id, T20, "some progress", |v| {
        v.received >= 512 * 1024
    })
    .await;
    assert_eq!(v.status, Status::Downloading);

    engine.pause(id);
    let paused = wait_for(&engine, id, T20, "pause", |v| v.status == Status::Paused).await;
    assert!(paused.received > 0 && paused.received < BODY_LEN as u64);
    engine.shutdown();
    drop(engine);

    // A new engine on the same data dir sees the paused download with its progress.
    server.control.throttle_ms.store(0, Ordering::SeqCst);
    let engine = Engine2::start(&dirs);
    let restored = view(&engine, id);
    assert_eq!(restored.status, Status::Paused);
    assert!(restored.received > 0, "progress lost: {restored:?}");
    assert!(!restored.segments.is_empty());

    engine.resume(id);
    let done = wait_completed(&engine, id, T20).await;
    assert_file_matches(&done, &deterministic_body(BODY_LEN), BODY_LEN);
    engine.shutdown();
}

/// Starts an engine on existing dirs without re-applying settings (they come from state.json).
struct Engine2;
impl Engine2 {
    fn start(dirs: &Dirs) -> sushload_core::Engine {
        sushload_core::Engine::start(dirs.data.path().to_path_buf()).expect("restart")
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn speed_limit_is_enforced() {
    let server = start_server().await;
    let dirs = Dirs::new();
    let mut settings = dirs.settings();
    settings.speed_limit = MIB as u64;
    let engine = dirs.engine(settings);

    let started = Instant::now();
    let id = engine
        .add(&server.url("slow.bin?len=3145728"))
        .expect("add");
    let v = wait_completed(&engine, id, Duration::from_secs(30)).await;
    let elapsed = started.elapsed();

    assert!(
        elapsed >= Duration::from_secs(2),
        "3 MiB at 1 MiB/s finished in {elapsed:?}"
    );
    assert_file_matches(&v, &deterministic_body(BODY_LEN), 3 * MIB);
    engine.shutdown();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn max_concurrent_one_never_runs_two_at_once() {
    let server = start_server().await;
    server.control.throttle_ms.store(5, Ordering::SeqCst);
    let dirs = Dirs::new();
    let mut settings = dirs.settings();
    settings.max_concurrent = 1;
    settings.connections = 2;
    let engine = dirs.engine(settings);

    let ids: Vec<_> = (0..3)
        .map(|i| {
            engine
                .add(&server.url(&format!("q{i}.bin?len=1048576")))
                .expect("add")
        })
        .collect();

    let deadline = Instant::now() + Duration::from_secs(30);
    let mut max_running = 0;
    loop {
        let views = engine.snapshot();
        let running = views
            .iter()
            .filter(|v| matches!(v.status, Status::Connecting | Status::Downloading))
            .count();
        max_running = max_running.max(running);
        assert!(
            running <= 1,
            "{running} downloads active at once: {views:?}"
        );
        if views.iter().all(|v| v.status == Status::Completed) {
            break;
        }
        assert!(
            views.iter().all(|v| v.status != Status::Failed),
            "a download failed: {views:?}"
        );
        assert!(Instant::now() < deadline, "timed out: {views:?}");
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    assert_eq!(max_running, 1, "never observed a running download");
    for id in ids {
        assert_file_matches(&view(&engine, id), &deterministic_body(BODY_LEN), MIB);
    }
    engine.shutdown();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn not_found_fails_with_404() {
    let server = start_server().await;
    let dirs = Dirs::new();
    let engine = dirs.engine(dirs.settings());

    let id = engine.add(&server.url("missing.bin")).expect("add");
    let v = wait_failed(&engine, id, T20).await;
    assert!(v.error.as_deref().unwrap_or("").contains("404"), "{v:?}");
    engine.shutdown();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn remove_with_delete_removes_completed_file() {
    let server = start_server().await;
    let dirs = Dirs::new();
    let engine = dirs.engine(dirs.settings());

    let id = engine
        .add(&server.url("gone.bin?len=1048576"))
        .expect("add");
    let v = wait_completed(&engine, id, T20).await;
    assert!(v.path.exists());

    engine.remove(id, true);
    assert!(engine.snapshot().iter().all(|v| v.id != id));
    wait_until(|| !v.path.exists(), "final file deletion").await;
    engine.shutdown();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn remove_keeps_completed_file_without_delete_and_drops_part() {
    let server = start_server().await;
    let dirs = Dirs::new();
    let engine = dirs.engine(dirs.settings());

    let kept = engine
        .add(&server.url("kept.bin?len=1048576"))
        .expect("add");
    let kept_view = wait_completed(&engine, kept, T20).await;
    engine.remove(kept, false);

    server.control.throttle_ms.store(20, Ordering::SeqCst);
    let running = engine.add(&server.url("partial.bin")).expect("add");
    let v = wait_for(&engine, running, T20, "progress", |v| v.received > 0).await;
    let part = part_of(&v.path);
    assert!(part.exists());
    engine.remove(running, true);
    wait_until(|| !part.exists(), ".part deletion").await;
    assert!(kept_view.path.exists(), "remove(false) deleted the file");
    engine.shutdown();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn failure_mid_download_keeps_progress_and_resume_finishes() {
    let server = start_server().await;
    // Every response is cut after 64 KiB; workers retry from where they stopped, then give up.
    server.control.drop_after.store(64 * 1024, Ordering::SeqCst);
    let dirs = Dirs::new();
    let engine = dirs.engine(dirs.settings());

    let id = engine.add(&server.url("flaky.bin")).expect("add");
    let failed = wait_failed(&engine, id, Duration::from_secs(60)).await;
    assert!(failed.received > 0, "progress was discarded: {failed:?}");
    assert!(!failed.segments.is_empty(), "segments were discarded");
    assert!(failed.received < BODY_LEN as u64);
    assert!(part_of(&failed.path).exists(), ".part was removed");

    server.control.drop_after.store(0, Ordering::SeqCst);
    engine.resume(id);
    let done = wait_completed(&engine, id, T20).await;
    assert_file_matches(&done, &deterministic_body(BODY_LEN), BODY_LEN);
    engine.shutdown();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn changed_file_on_resume_restarts_from_zero() {
    use std::sync::Arc;
    let server = start_server_with(Arc::new(deterministic_body(4 * MIB))).await;
    server.control.drop_after.store(64 * 1024, Ordering::SeqCst);
    let dirs = Dirs::new();
    let engine = dirs.engine(dirs.settings());
    let id = engine.add(&server.url("same.bin")).expect("add");
    let failed = wait_failed(&engine, id, Duration::from_secs(60)).await;
    assert!(failed.received > 0);
    engine.shutdown();
    drop(engine);

    // A different file now lives at the same URL (different length): progress must be reset.
    let other = start_server_with(Arc::new(deterministic_body(3 * MIB + 5))).await;
    let state_path = dirs.data.path().join("state.json");
    let text = std::fs::read_to_string(&state_path).expect("state");
    let text = text.replace(&server.addr.to_string(), &other.addr.to_string());
    std::fs::write(&state_path, text).expect("rewrite state");

    let engine = Engine2::start(&dirs);
    engine.resume(id);
    let done = wait_completed(&engine, id, T20).await;
    assert_eq!(done.total, Some(3 * MIB as u64 + 5));
    assert_file_matches(&done, &deterministic_body(3 * MIB + 5), 3 * MIB + 5);
    engine.shutdown();
}

async fn wait_until(mut cond: impl FnMut() -> bool, what: &str) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !cond() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}
