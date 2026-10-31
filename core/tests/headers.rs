//! Browser context (Referer, cookies, User-Agent, file name) handed over with `add_request`.
mod common;
use common::*;
use std::os::unix::fs::PermissionsExt;
use std::time::Duration;
use sushload_core::{AddRequest, Engine, Status};

const T20: Duration = Duration::from_secs(20);
const COOKIE: &str = "session=s3cr3t-cookie; theme=dark";
const REFERER: &str = "https://example.test/page";
const UA: &str = "BrowserUA/1.0 (test)";

fn request(url: String) -> AddRequest {
    AddRequest {
        url,
        file_name: None,
        referer: Some(REFERER.to_string()),
        cookies: Some(COOKIE.to_string()),
        user_agent: Some(UA.to_string()),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn headers_reach_probe_and_every_range_request() {
    let server = start_server().await;
    let dirs = Dirs::new();
    let engine = dirs.engine(dirs.settings());

    let id = engine
        .add_request(request(server.url("ctx.bin")))
        .expect("add");
    let v = wait_completed(&engine, id, T20).await;
    assert!(v.resumable && !v.segments.is_empty(), "expected segments");
    assert_file_matches(&v, &deterministic_body(BODY_LEN), BODY_LEN);

    let seen = server.control.seen();
    let ranged = seen.iter().filter(|r| r.range.is_some()).count();
    assert!(ranged >= 2, "probe plus several segments, saw {ranged}");
    for r in &seen {
        assert_eq!(r.header("cookie"), Some(COOKIE), "{r:?}");
        assert_eq!(r.header("referer"), Some(REFERER), "{r:?}");
        assert_eq!(r.header("user-agent"), Some(UA), "{r:?}");
    }
    engine.shutdown();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn single_stream_download_also_sends_headers() {
    let server = start_server().await;
    server
        .control
        .ignore_range
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let dirs = Dirs::new();
    let engine = dirs.engine(dirs.settings());

    let id = engine
        .add_request(request(server.url("single.bin")))
        .expect("add");
    let v = wait_completed(&engine, id, T20).await;
    assert!(v.segments.is_empty());
    let seen = server.control.seen();
    assert!(seen.len() >= 2, "probe and plain GET");
    for r in &seen {
        assert_eq!(r.header("cookie"), Some(COOKIE));
        assert_eq!(r.header("referer"), Some(REFERER));
        assert_eq!(r.header("user-agent"), Some(UA));
    }
    engine.shutdown();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn plain_add_sends_no_cookie_or_referer_and_default_ua() {
    let server = start_server().await;
    let dirs = Dirs::new();
    let engine = dirs.engine(dirs.settings());

    let id = engine.add(&server.url("plain.bin")).expect("add");
    wait_completed(&engine, id, T20).await;
    for r in server.control.seen() {
        assert_eq!(r.header("cookie"), None);
        assert_eq!(r.header("referer"), None);
        assert!(r.header("user-agent").unwrap_or("").contains("sushLoad"));
    }
    engine.shutdown();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cross_origin_redirect_drops_cookie_but_keeps_referer_and_ua() {
    let origin_b = start_server().await;
    let origin_a = start_server_with(std::sync::Arc::new(Vec::new())).await;
    *origin_a.control.redirect_to.lock().unwrap() = Some(origin_b.url("moved.bin"));
    let dirs = Dirs::new();
    let engine = dirs.engine(dirs.settings());

    let id = engine
        .add_request(request(origin_a.url("start.bin")))
        .expect("add");
    let v = wait_completed(&engine, id, T20).await;
    assert_file_matches(&v, &deterministic_body(BODY_LEN), BODY_LEN);

    let at_a = origin_a.control.seen();
    assert!(!at_a.is_empty());
    for r in &at_a {
        assert_eq!(r.header("cookie"), Some(COOKIE), "A is the origin: {r:?}");
    }
    let at_b = origin_b.control.seen();
    assert!(!at_b.is_empty(), "B was never reached");
    for r in &at_b {
        assert_eq!(r.header("cookie"), None, "cookie leaked to B: {r:?}");
        assert_eq!(r.header("referer"), Some(REFERER), "{r:?}");
        assert_eq!(r.header("user-agent"), Some(UA), "{r:?}");
    }
    // No error text or view field shows the cookie.
    assert!(!format!("{v:?}").contains("s3cr3t"));
    engine.shutdown();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn file_name_override_names_the_file() {
    let server = start_server().await;
    let dirs = Dirs::new();
    let engine = dirs.engine(dirs.settings());

    let mut req = request(server.url("ugly-url-name.bin?len=4096"));
    req.file_name = Some("../nice/Report 2024.pdf".to_string());
    let id = engine.add_request(req).expect("add");
    assert_eq!(view(&engine, id).file_name, ".._nice_Report 2024.pdf");
    let v = wait_completed(&engine, id, T20).await;
    assert_eq!(v.file_name, ".._nice_Report 2024.pdf");
    assert_eq!(v.path.parent().unwrap(), dirs.downloads.path());
    assert_eq!(v.path.file_name().unwrap(), ".._nice_Report 2024.pdf");
    assert_file_matches(&v, &deterministic_body(BODY_LEN), 4096);

    let mut req = request(server.url("other.bin?len=1000"));
    req.file_name = Some("clip.mp4".to_string());
    let id = engine.add_request(req).expect("add");
    let v = wait_completed(&engine, id, T20).await;
    assert_eq!(v.path.file_name().unwrap(), "clip.mp4");
    engine.shutdown();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn invalid_requests_are_rejected_without_echoing_cookies() {
    let dirs = Dirs::new();
    let engine = dirs.engine(dirs.settings());
    let mut req = request("ftp://example.test/a".to_string());
    assert!(engine.add_request(req.clone()).is_err());
    req.url = "http://example.test/a".to_string();
    req.cookies = Some("a=b\r\nX-Evil: 1".to_string());
    let error = engine.add_request(req).unwrap_err();
    assert!(!error.contains("X-Evil"), "{error}");
    assert!(engine.snapshot().is_empty());
    engine.shutdown();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn headers_survive_restart_and_state_file_is_private() {
    let server = start_server().await;
    server
        .control
        .throttle_ms
        .store(15, std::sync::atomic::Ordering::SeqCst);
    let dirs = Dirs::new();
    let engine = dirs.engine(dirs.settings());

    let mut req = request(server.url("resume-ctx.bin"));
    req.file_name = Some("kept-name.bin".to_string());
    let id = engine.add_request(req).expect("add");
    wait_for(&engine, id, T20, "some progress", |v| {
        v.received >= 512 * 1024
    })
    .await;
    engine.pause(id);
    wait_for(&engine, id, T20, "pause", |v| v.status == Status::Paused).await;
    engine.shutdown();
    drop(engine);

    let state = dirs.data.path().join("state.json");
    let mode = std::fs::metadata(&state).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600, "state.json mode is {mode:o}");
    assert!(std::fs::read_to_string(&state).unwrap().contains(COOKIE));

    server
        .control
        .throttle_ms
        .store(0, std::sync::atomic::Ordering::SeqCst);
    let before = server.control.seen().len();
    let engine = Engine::start(dirs.data.path().to_path_buf()).expect("restart");
    engine.set_settings(dirs.settings());
    let restored = view(&engine, id);
    assert_eq!(restored.status, Status::Paused);
    assert!(!format!("{restored:?}").contains("s3cr3t"));
    engine.resume(id);
    let v = wait_completed(&engine, id, T20).await;
    assert_eq!(v.path.file_name().unwrap(), "kept-name.bin");
    assert_file_matches(&v, &deterministic_body(BODY_LEN), BODY_LEN);

    let after = server.control.seen();
    assert!(after.len() > before);
    for r in &after[before..] {
        assert_eq!(r.header("cookie"), Some(COOKIE), "{r:?}");
        assert_eq!(r.header("referer"), Some(REFERER), "{r:?}");
        assert_eq!(r.header("user-agent"), Some(UA), "{r:?}");
    }
    engine.shutdown();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn old_state_files_without_context_still_load() {
    let dirs = Dirs::new();
    let old = r#"{"next_id":2,"downloads":[{"id":1,"url":"http://example.test/a.bin","file_name":"a.bin","path":"/tmp/a.bin","category":"Other","status":"Paused","total":null,"received":0,"segments":[],"resumable":false,"etag":null,"last_modified":null,"error":null,"added_at":1}]}"#;
    std::fs::write(dirs.data.path().join("state.json"), old).unwrap();
    let engine = Engine::start(dirs.data.path().to_path_buf()).expect("start");
    assert_eq!(engine.snapshot().len(), 1);
    engine.shutdown();
}
