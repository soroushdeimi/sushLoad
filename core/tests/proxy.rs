mod common;
use common::*;
use sushload_core::{Engine, ProxyMode, Settings};
use std::net::SocketAddr;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;

const T20: Duration = Duration::from_secs(20);

struct Proxy {
    addr: SocketAddr,
    /// Request lines (HTTP proxy) or `host:port` targets (SOCKS5) seen.
    seen: Arc<Mutex<Vec<String>>>,
    task: JoinHandle<()>,
}

impl Proxy {
    fn count(&self) -> usize {
        self.seen.lock().unwrap().len()
    }
}

impl Drop for Proxy {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn base64(input: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in input.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |n, (i, b)| n | (*b as u32) << (16 - 8 * i));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(T[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// HTTP forward proxy: absolute-form requests only. With `auth`, requires Basic credentials.
async fn http_proxy(auth: Option<(&str, &str)>) -> Proxy {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = Arc::clone(&seen);
    let expected = auth.map(|(u, p)| format!("Basic {}", base64(format!("{u}:{p}").as_bytes())));
    let task = tokio::spawn(async move {
        loop {
            let Ok((mut client, _)) = listener.accept().await else {
                break;
            };
            let log = Arc::clone(&log);
            let expected = expected.clone();
            tokio::spawn(async move {
                let mut request = Vec::new();
                let mut buf = [0u8; 2048];
                while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                    match client.read(&mut buf).await {
                        Ok(0) | Err(_) => return,
                        Ok(n) => request.extend_from_slice(&buf[..n]),
                    }
                }
                let text = String::from_utf8_lossy(&request).into_owned();
                let mut lines = text.split("\r\n");
                let first = lines.next().unwrap_or_default().to_string();
                let mut parts = first.split_whitespace();
                let (_method, target) = (parts.next(), parts.next().unwrap_or(""));
                let supplied = text.split("\r\n").find_map(|l| {
                    l.split_once(':')
                        .filter(|(k, _)| k.eq_ignore_ascii_case("proxy-authorization"))
                        .map(|(_, v)| v.trim().to_string())
                });
                if let Some(expected) = &expected
                    && supplied.as_deref() != Some(expected.as_str())
                {
                    let _ = client
                        .write_all(b"HTTP/1.1 407 Proxy Authentication Required\r\nProxy-Authenticate: Basic\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                        .await;
                    return;
                }
                let Some(rest) = target.strip_prefix("http://") else {
                    return;
                };
                let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
                log.lock().unwrap().push(first.clone());
                let Ok(mut origin) = TcpStream::connect(authority).await else {
                    return;
                };
                // Rewrite to origin form and drop proxy headers.
                let mut out = format!("GET /{path} HTTP/1.1\r\n");
                for line in text.split("\r\n").skip(1) {
                    if line.is_empty() || line.to_ascii_lowercase().starts_with("proxy-") {
                        continue;
                    }
                    out.push_str(line);
                    out.push_str("\r\n");
                }
                out.push_str("\r\n");
                if origin.write_all(out.as_bytes()).await.is_err() {
                    return;
                }
                let _ = tokio::io::copy(&mut origin, &mut client).await;
                let _ = client.shutdown().await;
            });
        }
    });
    Proxy { addr, seen, task }
}

/// Minimal SOCKS5 server (CONNECT only). With `auth`, requires RFC 1929 user/pass.
async fn socks5_proxy(auth: Option<(&str, &str)>) -> Proxy {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = Arc::clone(&seen);
    let auth = auth.map(|(u, p)| (u.to_string(), p.to_string()));
    let task = tokio::spawn(async move {
        loop {
            let Ok((client, _)) = listener.accept().await else {
                break;
            };
            let log = Arc::clone(&log);
            let auth = auth.clone();
            tokio::spawn(async move {
                let _ = socks5_session(client, auth, log).await;
            });
        }
    });
    Proxy { addr, seen, task }
}

async fn socks5_session(
    mut c: TcpStream,
    auth: Option<(String, String)>,
    log: Arc<Mutex<Vec<String>>>,
) -> std::io::Result<()> {
    let mut head = [0u8; 2];
    c.read_exact(&mut head).await?;
    let mut methods = vec![0u8; head[1] as usize];
    c.read_exact(&mut methods).await?;
    let wanted = if auth.is_some() { 0x02 } else { 0x00 };
    if head[0] != 5 || !methods.contains(&wanted) {
        c.write_all(&[5, 0xFF]).await?;
        return Ok(());
    }
    c.write_all(&[5, wanted]).await?;
    if let Some((user, pass)) = &auth {
        let mut v = [0u8; 2];
        c.read_exact(&mut v).await?;
        let mut u = vec![0u8; v[1] as usize];
        c.read_exact(&mut u).await?;
        let mut pl = [0u8; 1];
        c.read_exact(&mut pl).await?;
        let mut p = vec![0u8; pl[0] as usize];
        c.read_exact(&mut p).await?;
        if v[0] != 1 || u != user.as_bytes() || p != pass.as_bytes() {
            c.write_all(&[1, 1]).await?;
            return Ok(());
        }
        c.write_all(&[1, 0]).await?;
    }
    let mut req = [0u8; 4];
    c.read_exact(&mut req).await?;
    let host = match req[3] {
        1 => {
            let mut a = [0u8; 4];
            c.read_exact(&mut a).await?;
            format!("{}.{}.{}.{}", a[0], a[1], a[2], a[3])
        }
        3 => {
            let mut l = [0u8; 1];
            c.read_exact(&mut l).await?;
            let mut d = vec![0u8; l[0] as usize];
            c.read_exact(&mut d).await?;
            String::from_utf8_lossy(&d).into_owned()
        }
        _ => {
            c.write_all(&[5, 8, 0, 1, 0, 0, 0, 0, 0, 0]).await?;
            return Ok(());
        }
    };
    let mut port = [0u8; 2];
    c.read_exact(&mut port).await?;
    let target = format!("{host}:{}", u16::from_be_bytes(port));
    if req[1] != 1 {
        c.write_all(&[5, 7, 0, 1, 0, 0, 0, 0, 0, 0]).await?;
        return Ok(());
    }
    log.lock().unwrap().push(target.clone());
    let mut origin = match TcpStream::connect(&target).await {
        Ok(o) => o,
        Err(_) => {
            c.write_all(&[5, 5, 0, 1, 0, 0, 0, 0, 0, 0]).await?;
            return Ok(());
        }
    };
    c.write_all(&[5, 0, 0, 1, 0, 0, 0, 0, 0, 0]).await?;
    let _ = tokio::io::copy_bidirectional(&mut c, &mut origin).await;
    Ok(())
}

fn custom(dirs: &Dirs, url: String) -> Settings {
    Settings {
        proxy_mode: ProxyMode::Custom,
        proxy_url: url,
        ..dirs.settings()
    }
}

async fn download_ok(engine: &Engine, url: &str) {
    let id = engine.add(url).expect("add");
    let v = wait_completed(engine, id, T20).await;
    let len = url
        .split_once("len=")
        .and_then(|(_, n)| n.parse::<usize>().ok())
        .unwrap_or(BODY_LEN);
    assert_file_matches(&v, &deterministic_body(BODY_LEN), len);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn http_proxy_carries_every_request() {
    let origin = start_server().await;
    let proxy = http_proxy(None).await;
    let dirs = Dirs::new();
    let engine = dirs.engine(custom(&dirs, format!("http://{}", proxy.addr)));

    download_ok(&engine, &origin.url("via-http.bin")).await;

    let seen = proxy.seen.lock().unwrap().clone();
    assert!(seen.len() >= 2, "proxy saw {seen:?}");
    assert!(
        seen.iter().all(|l| l.starts_with("GET http://127.0.0.1:")),
        "{seen:?}"
    );
    // Probe + one request per segment: the origin saw exactly what the proxy forwarded.
    assert_eq!(seen.len(), origin.control.requests.load(Ordering::SeqCst));
    assert!(origin.control.range_requests.load(Ordering::SeqCst) >= 4);
    engine.shutdown();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn http_proxy_basic_auth_from_url_userinfo() {
    let origin = start_server().await;
    let proxy = http_proxy(Some(("alice", "p@ss:w/rd"))).await;
    let dirs = Dirs::new();
    // The password is percent-encoded in the URL and must be decoded before use.
    let engine = dirs.engine(custom(
        &dirs,
        format!("http://alice:p%40ss%3Aw%2Frd@{}", proxy.addr),
    ));

    download_ok(&engine, &origin.url("authed.bin?len=3145728")).await;
    assert!(proxy.count() >= 1);
    engine.shutdown();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn socks5_proxy_without_auth() {
    let origin = start_server().await;
    let proxy = socks5_proxy(None).await;
    let dirs = Dirs::new();
    let engine = dirs.engine(custom(&dirs, format!("socks5://{}", proxy.addr)));

    download_ok(&engine, &origin.url("via-socks.bin")).await;

    let seen = proxy.seen.lock().unwrap().clone();
    assert!(seen.len() >= 2, "socks server saw {seen:?}");
    assert!(
        seen.iter().all(|t| *t == origin.addr.to_string()),
        "{seen:?}"
    );
    engine.shutdown();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn socks5h_proxy_with_user_pass_resolves_remotely() {
    let origin = start_server().await;
    let proxy = socks5_proxy(Some(("bob", "s3cret!"))).await;
    let dirs = Dirs::new();
    let engine = dirs.engine(custom(
        &dirs,
        format!("socks5h://bob:s3cret!@{}", proxy.addr),
    ));

    // `localhost` is sent to the proxy as a domain name (ATYP 3) and resolved there.
    let url = format!("http://localhost:{}/via-socks5h.bin", origin.addr.port());
    download_ok(&engine, &url).await;

    let seen = proxy.seen.lock().unwrap().clone();
    assert!(!seen.is_empty(), "socks server saw nothing");
    let expected = format!("localhost:{}", origin.addr.port());
    assert!(seen.iter().all(|t| *t == expected), "{seen:?}");
    engine.shutdown();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn socks5_wrong_password_fails_without_leaking_it() {
    let origin = start_server().await;
    let proxy = socks5_proxy(Some(("bob", "right"))).await;
    let dirs = Dirs::new();
    let engine = dirs.engine(custom(
        &dirs,
        format!("socks5://bob:wrongpw@{}", proxy.addr),
    ));

    let id = engine.add(&origin.url("denied.bin")).expect("add");
    let v = wait_failed(&engine, id, Duration::from_secs(40)).await;
    let error = v.error.unwrap_or_default();
    assert!(!error.contains("wrongpw"), "{error}");
    assert_eq!(proxy.count(), 0, "connection must not get past auth");
    engine.shutdown();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn proxy_off_ignores_proxy_environment() {
    let origin = start_server().await;
    let proxy = http_proxy(None).await;
    let dirs = Dirs::new();
    let proxy_url = format!("http://{}", proxy.addr);
    // SAFETY: only this test touches these variables; clients in this file always choose their
    // proxy mode explicitly (Off / Custom), which never reads them.
    unsafe {
        for key in ["HTTP_PROXY", "http_proxy", "ALL_PROXY", "all_proxy"] {
            std::env::set_var(key, &proxy_url);
        }
    }
    let settings = Settings {
        proxy_mode: ProxyMode::Off,
        ..dirs.settings()
    };
    let engine = dirs.engine(settings);

    download_ok(&engine, &origin.url("direct.bin")).await;
    assert_eq!(
        proxy.count(),
        0,
        "Off must not use the proxy from the environment"
    );
    engine.shutdown();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unreachable_custom_proxy_fails_and_names_the_proxy_without_password() {
    let origin = start_server().await;
    let dirs = Dirs::new();
    let engine = dirs.engine(custom(&dirs, "socks5://user:topsecret@127.0.0.1:1".into()));

    let id = engine.add(&origin.url("nope.bin")).expect("add");
    let v = wait_failed(&engine, id, T20).await;
    let error = v.error.unwrap_or_default();
    assert!(error.contains("Proxy"), "{error}");
    assert!(error.contains("127.0.0.1:1"), "{error}");
    assert!(!error.contains("topsecret"), "{error}");
    assert_eq!(origin.control.requests.load(Ordering::SeqCst), 0);
    engine.shutdown();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bad_proxy_setting_fails_new_downloads_and_engine_still_starts() {
    let origin = start_server().await;
    let dirs = Dirs::new();
    let engine = dirs.engine(custom(&dirs, "ftp://user:hunter2@host:21".into()));

    let id = engine.add(&origin.url("bad.bin")).expect("add");
    let v = wait_failed(&engine, id, T20).await;
    let error = v.error.unwrap_or_default();
    assert!(error.starts_with("Proxy:"), "{error}");
    assert!(!error.contains("hunter2"), "{error}");
    engine.shutdown();
    drop(engine);

    // The bad value was saved; starting again must still work, and fixing it recovers.
    let engine = Engine::start(dirs.data.path().to_path_buf()).expect("start with bad proxy");
    assert_eq!(engine.settings().proxy_url, "ftp://user:hunter2@host:21");
    let proxy = http_proxy(None).await;
    engine.set_settings(custom(&dirs, format!("http://{}", proxy.addr)));
    engine.resume(id);
    let done = wait_completed(&engine, id, T20).await;
    assert_file_matches(&done, &deterministic_body(BODY_LEN), BODY_LEN);
    assert!(proxy.count() >= 1);
    engine.shutdown();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn changing_the_proxy_applies_to_the_next_download() {
    let origin = start_server().await;
    let proxy = http_proxy(None).await;
    let dirs = Dirs::new();
    let settings = Settings {
        proxy_mode: ProxyMode::Off,
        ..dirs.settings()
    };
    let engine = dirs.engine(settings);

    download_ok(&engine, &origin.url("first.bin?len=2097152")).await;
    assert_eq!(proxy.count(), 0);

    engine.set_settings(custom(&dirs, format!("http://{}", proxy.addr)));
    download_ok(&engine, &origin.url("second.bin")).await;
    assert!(proxy.count() >= 1);
    engine.shutdown();
}
