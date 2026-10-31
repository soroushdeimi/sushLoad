//! Shared helpers for the integration tests: a tiny HTTP/1.1 origin server with switchable
//! behaviour, engine/settings helpers and polling utilities.
#![allow(dead_code)]

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};
use sushload_core::{DownloadId, DownloadView, Engine, Settings, Status};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::task::JoinHandle;

pub const MIB: usize = 1024 * 1024;
pub const BODY_LEN: usize = 8 * MIB;
const CHUNK: usize = 16 * 1024;

/// Deterministic pseudo-random bytes.
pub fn deterministic_body(len: usize) -> Vec<u8> {
    let mut state: u64 = 0x9E37_79B9_7F4A_7C15;
    let mut body = Vec::with_capacity(len);
    for _ in 0..len {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        body.push((state & 0xff) as u8);
    }
    body
}

/// Switches that tests flip while the server runs.
#[derive(Default)]
pub struct Control {
    /// Ignore `Range` and always answer 200 with the whole body.
    pub ignore_range: AtomicBool,
    /// Sleep this many milliseconds after every 16 KiB written.
    pub throttle_ms: AtomicU64,
    /// When non-zero, close each response after this many body bytes.
    pub drop_after: AtomicU64,
    /// Number of requests received.
    pub requests: AtomicUsize,
    /// Requests that carried a `Range` header.
    pub range_requests: AtomicUsize,
}

pub struct TestServer {
    pub addr: SocketAddr,
    pub control: Arc<Control>,
    task: JoinHandle<()>,
}

impl TestServer {
    /// URL for `name`; optional `?len=N` serves only the first N bytes of the body.
    pub fn url(&self, name: &str) -> String {
        format!("http://{}/{name}", self.addr)
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn parse_range(header: Option<&str>, len: usize) -> (usize, usize, bool) {
    let full = (0, len.saturating_sub(1), false);
    let Some(spec) = header.and_then(|h| h.trim().strip_prefix("bytes=")) else {
        return full;
    };
    let Some((start, end)) = spec.split_once('-') else {
        return full;
    };
    let Ok(start) = start.trim().parse::<usize>() else {
        return full;
    };
    let end = if end.trim().is_empty() {
        len.saturating_sub(1)
    } else {
        end.trim()
            .parse::<usize>()
            .unwrap_or(len.saturating_sub(1))
            .min(len.saturating_sub(1))
    };
    if start >= len || start > end {
        return full;
    }
    (start, end, true)
}

pub async fn start_server() -> TestServer {
    start_server_with(Arc::new(deterministic_body(BODY_LEN))).await
}

pub async fn start_server_with(body: Arc<Vec<u8>>) -> TestServer {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    let control = Arc::new(Control::default());
    let ctl = Arc::clone(&control);
    let task = tokio::spawn(async move {
        loop {
            let Ok((socket, _)) = listener.accept().await else {
                break;
            };
            tokio::spawn(serve(socket, Arc::clone(&body), Arc::clone(&ctl)));
        }
    });
    TestServer {
        addr,
        control,
        task,
    }
}

async fn serve(mut socket: tokio::net::TcpStream, body: Arc<Vec<u8>>, ctl: Arc<Control>) {
    let mut request = Vec::new();
    let mut buffer = [0u8; 1024];
    loop {
        match socket.read(&mut buffer).await {
            Ok(0) | Err(_) => return,
            Ok(read) => request.extend_from_slice(&buffer[..read]),
        }
        if request.windows(4).any(|w| w == b"\r\n\r\n") {
            break;
        }
        if request.len() > 64 * 1024 {
            return;
        }
    }
    ctl.requests.fetch_add(1, Ordering::SeqCst);

    let text = String::from_utf8_lossy(&request).into_owned();
    let mut lines = text.split("\r\n");
    let mut parts = lines.next().unwrap_or_default().split_whitespace();
    let _method = parts.next();
    let target = parts.next().unwrap_or("/").to_string();
    let mut range = None;
    for line in lines {
        if let Some((key, value)) = line.split_once(':')
            && key.eq_ignore_ascii_case("range")
        {
            range = Some(value.trim().to_string());
        }
    }
    if range.is_some() {
        ctl.range_requests.fetch_add(1, Ordering::SeqCst);
    }

    if target.starts_with("/missing") {
        let _ = socket
            .write_all(
                b"HTTP/1.1 404 Not Found\r\nContent-Length: 9\r\nConnection: close\r\n\r\nnot found",
            )
            .await;
        return;
    }

    let len = target
        .split_once("len=")
        .and_then(|(_, n)| n.split('&').next()?.parse::<usize>().ok())
        .map_or(body.len(), |n| n.min(body.len()));
    let body = &body[..len];

    if ctl.ignore_range.load(Ordering::SeqCst) {
        range = None;
    }
    let (start, end, partial) = parse_range(range.as_deref(), body.len());
    let slice = &body[start..=end];
    let status = if partial {
        "206 Partial Content"
    } else {
        "200 OK"
    };
    let mut header = format!(
        "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n",
        slice.len()
    );
    if !ctl.ignore_range.load(Ordering::SeqCst) {
        header.push_str("Accept-Ranges: bytes\r\n");
    }
    if partial {
        header.push_str(&format!(
            "Content-Range: bytes {start}-{end}/{}\r\n",
            body.len()
        ));
    }
    header.push_str("\r\n");
    if socket.write_all(header.as_bytes()).await.is_err() {
        return;
    }

    let limit = match ctl.drop_after.load(Ordering::SeqCst) {
        0 => usize::MAX,
        n => n as usize,
    };
    let mut sent = 0usize;
    for piece in slice.chunks(CHUNK) {
        let piece = &piece[..piece.len().min(limit.saturating_sub(sent))];
        if piece.is_empty() {
            // Abrupt close mid-body.
            return;
        }
        if socket.write_all(piece).await.is_err() {
            return;
        }
        sent += piece.len();
        let pause = ctl.throttle_ms.load(Ordering::SeqCst);
        if pause > 0 {
            tokio::time::sleep(Duration::from_millis(pause)).await;
        }
    }
    let _ = socket.flush().await;
    let _ = socket.shutdown().await;
}

/// Temp dirs plus settings pointing at them.
pub struct Dirs {
    pub data: tempfile::TempDir,
    pub downloads: tempfile::TempDir,
}

impl Dirs {
    pub fn new() -> Dirs {
        Dirs {
            data: tempfile::tempdir().expect("data dir"),
            downloads: tempfile::tempdir().expect("download dir"),
        }
    }

    pub fn settings(&self) -> Settings {
        Settings {
            download_dir: self.downloads.path().to_path_buf(),
            sort_into_categories: false,
            max_concurrent: 3,
            connections: 4,
            speed_limit: 0,
            ..Settings::default()
        }
    }

    pub fn engine(&self, settings: Settings) -> Engine {
        let engine = Engine::start(self.data.path().to_path_buf()).expect("engine start");
        engine.set_settings(settings);
        engine
    }
}

pub fn view(engine: &Engine, id: DownloadId) -> DownloadView {
    engine
        .snapshot()
        .into_iter()
        .find(|v| v.id == id)
        .expect("download is in the snapshot")
}

/// Polls until `done` is true for the download's view, or panics after `timeout`.
pub async fn wait_for(
    engine: &Engine,
    id: DownloadId,
    timeout: Duration,
    what: &str,
    done: impl Fn(&DownloadView) -> bool,
) -> DownloadView {
    let deadline = Instant::now() + timeout;
    loop {
        let v = view(engine, id);
        if done(&v) {
            return v;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {what}: {v:?}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

pub async fn wait_completed(engine: &Engine, id: DownloadId, timeout: Duration) -> DownloadView {
    let v = wait_for(engine, id, timeout, "completion", |v| {
        matches!(v.status, Status::Completed | Status::Failed)
    })
    .await;
    assert_eq!(
        v.status,
        Status::Completed,
        "download failed: {:?}",
        v.error
    );
    v
}

pub async fn wait_failed(engine: &Engine, id: DownloadId, timeout: Duration) -> DownloadView {
    let v = wait_for(engine, id, timeout, "failure", |v| {
        matches!(v.status, Status::Completed | Status::Failed)
    })
    .await;
    assert_eq!(v.status, Status::Failed, "expected failure: {v:?}");
    v
}

pub fn part_of(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(".part");
    PathBuf::from(name)
}

/// Asserts the finished file equals `body[..len]` and that no `.part` is left.
pub fn assert_file_matches(v: &DownloadView, body: &[u8], len: usize) {
    assert!(v.path.exists(), "completed file is missing: {:?}", v.path);
    assert!(!part_of(&v.path).exists(), ".part was not renamed");
    let data = std::fs::read(&v.path).expect("read downloaded file");
    assert_eq!(data.len(), len, "length differs");
    assert!(data == body[..len], "content differs");
}
