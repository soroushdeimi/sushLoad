//! What the confirmation dialog needs from the engine: folder/paused/overwrite options, the
//! probe, per-category folders and the completion event.
mod common;
use common::*;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use sushload_core::{AddRequest, Category, Engine, Event, ProbeInfo, Status};

const T20: Duration = Duration::from_secs(20);

fn req(url: String) -> AddRequest {
    AddRequest {
        url,
        ..AddRequest::default()
    }
}

/// `Engine::probe` blocks, so tests call it off the async workers.
async fn probe(engine: &Engine, request: AddRequest) -> Result<ProbeInfo, String> {
    let engine = engine.clone();
    tokio::task::spawn_blocking(move || engine.probe(&request))
        .await
        .expect("join")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dir_override_saves_into_that_folder() {
    let server = start_server().await;
    let dirs = Dirs::new();
    let engine = dirs.engine(dirs.settings());
    let target = tempfile::tempdir().expect("target");

    let id = engine
        .add_request(AddRequest {
            dir: Some(target.path().to_path_buf()),
            file_name: Some("chosen.bin".into()),
            ..req(server.url("x.bin"))
        })
        .expect("add");
    let v = wait_completed(&engine, id, T20).await;
    assert_eq!(v.path, target.path().join("chosen.bin"));
    assert_file_matches(&v, &deterministic_body(BODY_LEN), BODY_LEN);
    assert!(!dirs.downloads.path().join("chosen.bin").exists());

    let relative = engine.add_request(AddRequest {
        dir: Some("relative/dir".into()),
        ..req(server.url("y.bin"))
    });
    assert!(relative.is_err());
    engine.shutdown();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn start_paused_waits_until_resumed() {
    let server = start_server().await;
    let dirs = Dirs::new();
    let engine = dirs.engine(dirs.settings());

    let id = engine
        .add_request(AddRequest {
            start_paused: true,
            ..req(server.url("later.bin"))
        })
        .expect("add");
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(view(&engine, id).status, Status::Paused);
    assert_eq!(server.control.requests.load(Ordering::SeqCst), 0);

    engine.resume(id);
    let v = wait_completed(&engine, id, T20).await;
    assert_file_matches(&v, &deterministic_body(BODY_LEN), BODY_LEN);
    engine.shutdown();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn overwrite_replaces_and_default_renames() {
    let server = start_server().await;
    let dirs = Dirs::new();
    let engine = dirs.engine(dirs.settings());
    let existing = dirs.downloads.path().join("same.bin");
    std::fs::write(&existing, b"old").expect("seed");

    let renamed = engine
        .add_request(req(server.url("same.bin")))
        .expect("add");
    let v = wait_completed(&engine, renamed, T20).await;
    assert_eq!(v.path, dirs.downloads.path().join("same (1).bin"));
    assert_eq!(std::fs::read(&existing).expect("read"), b"old");

    let over = engine
        .add_request(AddRequest {
            overwrite: true,
            ..req(server.url("same.bin"))
        })
        .expect("add");
    let v = wait_completed(&engine, over, T20).await;
    assert_eq!(v.path, existing);
    assert_file_matches(&v, &deterministic_body(BODY_LEN), BODY_LEN);
    engine.shutdown();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn probe_reports_size_resumable_and_existing_file() {
    let server = start_server().await;
    let dirs = Dirs::new();
    let engine = dirs.engine(dirs.settings());

    let info = probe(&engine, req(server.url("movie.mkv")))
        .await
        .expect("probe");
    assert_eq!(info.file_name, "movie.mkv");
    assert_eq!(info.total, Some(BODY_LEN as u64));
    assert!(info.resumable);
    assert_eq!(info.category, Category::Video);
    assert_eq!(info.dir, dirs.downloads.path());
    assert!(!info.exists);
    assert_eq!(server.control.requests.load(Ordering::SeqCst), 1);
    assert_eq!(
        server.control.seen()[0].range.as_deref(),
        Some("bytes=0-0"),
        "one tiny ranged request"
    );

    std::fs::write(dirs.downloads.path().join("movie.mkv"), b"x").expect("seed");
    let again = probe(&engine, req(server.url("movie.mkv")))
        .await
        .expect("probe");
    assert!(again.exists);

    // No Range support: size from Content-Length, not resumable.
    server.control.ignore_range.store(true, Ordering::SeqCst);
    let plain = probe(&engine, req(server.url("a.zip")))
        .await
        .expect("probe");
    assert!(!plain.resumable);
    assert_eq!(plain.total, Some(BODY_LEN as u64));

    let missing = probe(&engine, req(server.url("missing.bin"))).await;
    assert_eq!(missing.unwrap_err(), "HTTP 404");
    assert!(probe(&engine, req("ftp://h/x".into())).await.is_err());
    engine.shutdown();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn probe_sends_cookie_only_to_its_own_origin() {
    let origin_b = start_server().await;
    let origin_a = start_server_with(Arc::new(Vec::new())).await;
    *origin_a.control.redirect_to.lock().unwrap() = Some(origin_b.url("moved.bin"));
    let dirs = Dirs::new();
    let engine = dirs.engine(dirs.settings());

    let info = probe(
        &engine,
        AddRequest {
            cookies: Some("sid=probe-secret".into()),
            referer: Some("https://example.test/".into()),
            ..req(origin_a.url("start.bin"))
        },
    )
    .await
    .expect("probe");
    assert_eq!(info.total, Some(BODY_LEN as u64));
    let at_a = origin_a.control.seen();
    assert_eq!(at_a.len(), 1);
    assert_eq!(at_a[0].header("cookie"), Some("sid=probe-secret"));
    let at_b = origin_b.control.seen();
    assert_eq!(at_b.len(), 1, "redirect was followed");
    assert_eq!(
        at_b[0].header("cookie"),
        None,
        "cookie leaked: {:?}",
        at_b[0]
    );
    assert_eq!(at_b[0].header("referer"), Some("https://example.test/"));
    engine.shutdown();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn category_dirs_decide_the_folder() {
    let server = start_server().await;
    let dirs = Dirs::new();
    let videos = tempfile::tempdir().expect("videos");
    let mut settings = dirs.settings();
    settings.sort_into_categories = true;
    settings
        .category_dirs
        .insert(Category::Video, videos.path().to_path_buf());
    let engine = dirs.engine(settings);

    let info = probe(&engine, req(server.url("clip.mp4")))
        .await
        .expect("probe");
    assert_eq!(info.dir, videos.path());
    let other = probe(&engine, req(server.url("a.zip")))
        .await
        .expect("probe");
    assert_eq!(other.dir, dirs.downloads.path().join("Archives"));

    let id = engine.add(&server.url("clip.mp4")).expect("add");
    let v = wait_completed(&engine, id, T20).await;
    assert_eq!(v.path, videos.path().join("clip.mp4"));
    assert_eq!(v.category, Category::Video);

    // A category chosen in the dialog moves the file to that category's folder.
    let id = engine
        .add_request(AddRequest {
            category: Some(Category::Video),
            ..req(server.url("odd.bin"))
        })
        .expect("add");
    let v = wait_completed(&engine, id, T20).await;
    assert_eq!(v.path, videos.path().join("odd.bin"));
    assert_eq!(v.category, Category::Video);
    engine.shutdown();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn completion_event_is_emitted_exactly_once() {
    let server = start_server().await;
    let dirs = Dirs::new();
    let engine = dirs.engine(dirs.settings());
    let events: Arc<Mutex<Vec<Event>>> = Arc::default();
    let sink = Arc::clone(&events);
    engine.set_event_handler(move |e| sink.lock().unwrap().push(e.clone()));

    let id = engine.add(&server.url("done.bin")).expect("add");
    wait_completed(&engine, id, T20).await;
    let bad = engine.add(&server.url("missing.bin")).expect("add");
    wait_failed(&engine, bad, T20).await;
    // Nothing else may arrive later (no timers, no repeats).
    tokio::time::sleep(Duration::from_millis(500)).await;

    let got = events.lock().unwrap().clone();
    let completed: Vec<_> = got
        .iter()
        .filter(|e| matches!(e, Event::Completed { .. }))
        .collect();
    assert_eq!(completed.len(), 1, "{got:?}");
    assert!(matches!(
        completed[0],
        Event::Completed { id: i, file_name, total, .. }
            if *i == id && file_name == "done.bin" && *total == BODY_LEN as u64
    ));
    let failed: Vec<_> = got
        .iter()
        .filter(|e| matches!(e, Event::Failed { .. }))
        .collect();
    assert_eq!(failed.len(), 1, "{got:?}");
    assert!(
        matches!(failed[0], Event::Failed { id: i, error, .. } if *i == bad && error == "HTTP 404")
    );
    assert_eq!(got.len(), 2);
    engine.shutdown();
}
