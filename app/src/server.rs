//! Local HTTP API + embedded static UI. One background accept thread (blocking `recv`), one
//! short-lived thread per request. Nothing runs while idle.

use serde_json::{Value, json};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use sushload_core::{
    DownloadId, DownloadView, Engine, ProxyMode, Settings, Status, validate_proxy_url,
};

const INDEX_HTML: &str = include_str!("../web/index.html");
const STYLE_CSS: &str = include_str!("../web/style.css");
const APP_JS: &str = include_str!("../web/app.js");

const MAX_BODY: usize = 1024 * 1024;

/// What the HTTP layer needs from the engine and the OS. `Engine` implements it; tests use a mock.
pub trait Backend: Send + Sync + 'static {
    fn add(&self, url: &str) -> Result<u64, String>;
    fn pause(&self, id: u64);
    fn resume(&self, id: u64);
    fn remove(&self, id: u64, delete_file: bool);
    fn pause_all(&self);
    fn resume_all(&self);
    fn snapshot(&self) -> Vec<DownloadView>;
    fn has_active(&self) -> bool;
    fn settings(&self) -> Settings;
    fn set_settings(&self, s: Settings);
    fn open_path(&self, path: &Path, reveal: bool);
    fn choose_dir(&self) -> Option<String>;
}

impl Backend for Engine {
    fn add(&self, url: &str) -> Result<u64, String> {
        Engine::add(self, url).map(|id| id.0)
    }
    fn pause(&self, id: u64) {
        Engine::pause(self, DownloadId(id))
    }
    fn resume(&self, id: u64) {
        Engine::resume(self, DownloadId(id))
    }
    fn remove(&self, id: u64, delete_file: bool) {
        Engine::remove(self, DownloadId(id), delete_file)
    }
    fn pause_all(&self) {
        Engine::pause_all(self)
    }
    fn resume_all(&self) {
        Engine::resume_all(self)
    }
    fn snapshot(&self) -> Vec<DownloadView> {
        Engine::snapshot(self)
    }
    fn has_active(&self) -> bool {
        Engine::has_active(self)
    }
    fn settings(&self) -> Settings {
        Engine::settings(self)
    }
    fn set_settings(&self, s: Settings) {
        Engine::set_settings(self, s)
    }
    fn open_path(&self, path: &Path, reveal: bool) {
        let mut cmd = Command::new("open");
        if reveal {
            cmd.arg("-R");
        }
        let _ = cmd.arg(path).spawn().map(|mut c| c.wait());
    }
    fn choose_dir(&self) -> Option<String> {
        let out = Command::new("osascript")
            .args(["-e", "POSIX path of (choose folder)"])
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if s.is_empty() {
            return None;
        }
        // Drop a trailing slash (keep a lone "/").
        Some(if s.len() > 1 {
            s.trim_end_matches('/').to_string()
        } else {
            s
        })
    }
}

pub struct Ctx<B: Backend> {
    pub backend: B,
    pub token: String,
    pub port: u16,
}

pub struct Response {
    pub status: u16,
    pub content_type: &'static str,
    pub body: Vec<u8>,
}

fn json_resp(status: u16, v: Value) -> Response {
    Response {
        status,
        content_type: "application/json; charset=utf-8",
        body: v.to_string().into_bytes(),
    }
}

fn err(status: u16, msg: &str) -> Response {
    json_resp(status, json!({ "error": msg }))
}

fn static_resp(ct: &'static str, body: &'static str) -> Response {
    Response {
        status: 200,
        content_type: ct,
        body: body.as_bytes().to_vec(),
    }
}

fn header<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.as_str())
}

/// Constant-time-ish comparison; the token is local-only but there is no reason to leak.
fn tokens_equal(a: &str, b: &str) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.bytes()
        .zip(b.bytes())
        .fold(0u8, |acc, (x, y)| acc | (x ^ y))
        == 0
}

pub fn view_json(v: &DownloadView) -> Value {
    json!({
        "id": v.id.0,
        "url": v.url,
        "file_name": v.file_name,
        "path": v.path.to_string_lossy(),
        "category": v.category.label().to_lowercase(),
        "status": v.status.label().to_lowercase(),
        "total": v.total,
        "received": v.received,
        "speed": v.speed,
        "eta_secs": v.eta_secs,
        "connections": v.connections,
        "segments": v.segments.iter().map(|s| json!({"start": s.start, "end": s.end, "received": s.received})).collect::<Vec<_>>(),
        "resumable": v.resumable,
        "error": v.error,
        "added_at": v.added_at,
    })
}

fn settings_json(s: &Settings) -> Value {
    json!({
        "download_dir": s.download_dir.to_string_lossy(),
        "sort_into_categories": s.sort_into_categories,
        "max_concurrent": s.max_concurrent,
        "connections": s.connections,
        "speed_limit": s.speed_limit,
        "proxy_mode": match s.proxy_mode {
            ProxyMode::System => "system",
            ProxyMode::Off => "off",
            ProxyMode::Custom => "custom",
        },
        "proxy_url": s.proxy_url,
    })
}

/// Parses and validates a settings object from the UI.
pub fn parse_settings(v: &Value) -> Result<Settings, String> {
    let dir = v
        .get("download_dir")
        .and_then(Value::as_str)
        .ok_or("download_dir missing")?;
    if dir.trim().is_empty() {
        return Err("download_dir is empty".into());
    }
    let dir = PathBuf::from(dir);
    if !dir.is_absolute() {
        return Err("download_dir must be an absolute path".into());
    }
    let sort = v
        .get("sort_into_categories")
        .and_then(Value::as_bool)
        .ok_or("sort_into_categories missing")?;
    let mc = v
        .get("max_concurrent")
        .and_then(Value::as_u64)
        .ok_or("max_concurrent missing")?;
    if !(1..=10).contains(&mc) {
        return Err("max_concurrent must be 1..=10".into());
    }
    let conns = v
        .get("connections")
        .and_then(Value::as_u64)
        .ok_or("connections missing")?;
    if !(1..=16).contains(&conns) {
        return Err("connections must be 1..=16".into());
    }
    let limit = v
        .get("speed_limit")
        .and_then(Value::as_u64)
        .ok_or("speed_limit missing")?;
    if limit > 1 << 40 {
        return Err("speed_limit too large".into());
    }
    let proxy_mode = match v.get("proxy_mode").and_then(Value::as_str) {
        None | Some("system") => ProxyMode::System,
        Some("off") => ProxyMode::Off,
        Some("custom") => ProxyMode::Custom,
        Some(other) => return Err(format!("unknown proxy_mode \"{other}\"")),
    };
    let proxy_url = v
        .get("proxy_url")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    if proxy_mode == ProxyMode::Custom {
        validate_proxy_url(&proxy_url)?;
    }
    Ok(Settings {
        download_dir: dir,
        sort_into_categories: sort,
        max_concurrent: mc as u32,
        connections: conns as u32,
        speed_limit: limit,
        proxy_mode,
        proxy_url,
    })
}

fn id_of(body: &Value) -> Option<u64> {
    body.get("id").and_then(Value::as_u64)
}

/// Pure request router: no sockets, easy to test.
pub fn handle<B: Backend>(
    ctx: &Ctx<B>,
    method: &str,
    path: &str,
    headers: &[(String, String)],
    body: &[u8],
) -> Response {
    // DNS-rebinding guard: only our own origin names are accepted.
    let host_ok = header(headers, "host").is_some_and(|h| {
        h == format!("127.0.0.1:{}", ctx.port) || h == format!("localhost:{}", ctx.port)
    });
    if !host_ok {
        return err(403, "bad host");
    }

    let path_only = path.split(['?', '#']).next().unwrap_or("/");

    if !path_only.starts_with("/api/") {
        if method != "GET" && method != "HEAD" {
            return err(405, "method not allowed");
        }
        return match path_only {
            "/" | "/index.html" => static_resp("text/html; charset=utf-8", INDEX_HTML),
            "/style.css" => static_resp("text/css; charset=utf-8", STYLE_CSS),
            "/app.js" => static_resp("text/javascript; charset=utf-8", APP_JS),
            _ => err(404, "not found"),
        };
    }

    if !header(headers, "x-sushload-token").is_some_and(|t| tokens_equal(t, &ctx.token)) {
        return err(401, "missing or wrong token");
    }

    let b = &ctx.backend;
    let parsed: Value = if body.is_empty() {
        Value::Null
    } else {
        match serde_json::from_slice(body) {
            Ok(v) => v,
            Err(_) => return err(400, "invalid JSON"),
        }
    };

    match (method, path_only) {
        ("GET", "/api/state") => json_resp(
            200,
            json!({
                "active": b.has_active(),
                "downloads": b.snapshot().iter().map(view_json).collect::<Vec<_>>(),
                "settings": settings_json(&b.settings()),
            }),
        ),
        ("POST", "/api/add") => {
            let Some(text) = parsed.get("urls").and_then(Value::as_str) else {
                return err(400, "urls missing");
            };
            let mut added = 0u32;
            let mut errors = Vec::new();
            for line in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
                match b.add(line) {
                    Ok(_) => added += 1,
                    Err(e) => errors.push(format!("{line}: {e}")),
                }
            }
            json_resp(200, json!({ "added": added, "errors": errors }))
        }
        ("POST", "/api/pause") | ("POST", "/api/resume") | ("POST", "/api/remove") => {
            let Some(id) = id_of(&parsed) else {
                return err(400, "id missing");
            };
            match path_only {
                "/api/pause" => b.pause(id),
                "/api/resume" => b.resume(id),
                _ => b.remove(
                    id,
                    parsed
                        .get("delete_file")
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                ),
            }
            json_resp(200, json!({ "ok": true }))
        }
        ("POST", "/api/pause_all") => {
            b.pause_all();
            json_resp(200, json!({ "ok": true }))
        }
        ("POST", "/api/resume_all") => {
            b.resume_all();
            json_resp(200, json!({ "ok": true }))
        }
        ("POST", "/api/settings") => match parse_settings(&parsed) {
            Ok(s) => {
                b.set_settings(s);
                json_resp(
                    200,
                    json!({ "ok": true, "settings": settings_json(&b.settings()) }),
                )
            }
            Err(e) => err(400, &e),
        },
        ("POST", "/api/open") | ("POST", "/api/reveal") => {
            let Some(id) = id_of(&parsed) else {
                return err(400, "id missing");
            };
            // The path always comes from the engine's snapshot, never from the request.
            let Some(v) = b.snapshot().into_iter().find(|v| v.id.0 == id) else {
                return err(404, "no such download");
            };
            let reveal = path_only == "/api/reveal";
            if !reveal && v.status != Status::Completed {
                return err(409, "download is not finished");
            }
            let target = if v.path.exists() {
                v.path.clone()
            } else if let Some(parent) = v.path.parent().filter(|p| p.exists()) {
                if !reveal {
                    return err(404, "file is gone");
                }
                parent.to_path_buf()
            } else {
                return err(404, "file is gone");
            };
            b.open_path(&target, reveal);
            json_resp(200, json!({ "ok": true }))
        }
        ("POST", "/api/choose_dir") => json_resp(200, json!({ "path": b.choose_dir() })),
        _ => err(404, "not found"),
    }
}

/// Random 128-bit token as 32 hex chars, from `/dev/urandom`.
pub fn random_token() -> std::io::Result<String> {
    let mut buf = [0u8; 16];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut buf)?;
    Ok(buf.iter().map(|b| format!("{b:02x}")).collect())
}

pub struct RunningServer {
    pub port: u16,
    pub token: String,
}

/// Binds 127.0.0.1 (preferred port, else OS-chosen) and serves on a background thread.
pub fn start(engine: Engine, preferred_port: u16) -> Result<RunningServer, String> {
    let token = random_token().map_err(|e| format!("cannot read /dev/urandom: {e}"))?;
    let server = tiny_http::Server::http(("127.0.0.1", preferred_port))
        .or_else(|_| tiny_http::Server::http(("127.0.0.1", 0)))
        .map_err(|e| format!("cannot bind a local port: {e}"))?;
    let port = server
        .server_addr()
        .to_ip()
        .map(|a| a.port())
        .ok_or("server has no IP address")?;
    let ctx = Arc::new(Ctx {
        backend: engine,
        token: token.clone(),
        port,
    });
    std::thread::Builder::new()
        .name("sushload-http".into())
        .spawn(move || {
            for req in server.incoming_requests() {
                let ctx = ctx.clone();
                let _ = std::thread::Builder::new()
                    .name("sushload-req".into())
                    .spawn(move || serve_one(&ctx, req));
            }
        })
        .map_err(|e| e.to_string())?;
    Ok(RunningServer { port, token })
}

fn serve_one<B: Backend>(ctx: &Ctx<B>, mut req: tiny_http::Request) {
    let method = req.method().as_str().to_string();
    let url = req.url().to_string();
    let headers: Vec<(String, String)> = req
        .headers()
        .iter()
        .map(|h| {
            (
                h.field.as_str().as_str().to_string(),
                h.value.as_str().to_string(),
            )
        })
        .collect();
    let mut body = Vec::new();
    if req.body_length().is_some_and(|n| n > MAX_BODY) {
        let _ = req.respond(tiny_http::Response::from_string("too large").with_status_code(413));
        return;
    }
    let _ = req.as_reader().take(MAX_BODY as u64).read_to_end(&mut body);
    let r = handle(ctx, &method, &url, &headers, &body);
    let mut resp = tiny_http::Response::from_data(r.body).with_status_code(r.status);
    for (k, v) in [
        ("Content-Type", r.content_type),
        ("Cache-Control", "no-store"),
        ("X-Content-Type-Options", "nosniff"),
        ("Referrer-Policy", "no-referrer"),
    ] {
        if let Ok(h) = tiny_http::Header::from_bytes(k, v) {
            resp.add_header(h);
        }
    }
    let _ = req.respond(resp);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use sushload_core::{Category, Segment};

    #[derive(Default)]
    struct Mock {
        added: Mutex<Vec<String>>,
        settings: Mutex<Option<Settings>>,
        removed: Mutex<Vec<(u64, bool)>>,
    }

    impl Backend for Mock {
        fn add(&self, url: &str) -> Result<u64, String> {
            if !url.starts_with("http") {
                return Err("invalid URL".into());
            }
            let mut a = self.added.lock().unwrap();
            a.push(url.to_string());
            Ok(a.len() as u64)
        }
        fn pause(&self, _: u64) {}
        fn resume(&self, _: u64) {}
        fn remove(&self, id: u64, d: bool) {
            self.removed.lock().unwrap().push((id, d));
        }
        fn pause_all(&self) {}
        fn resume_all(&self) {}
        fn snapshot(&self) -> Vec<DownloadView> {
            vec![DownloadView {
                id: DownloadId(7),
                url: "http://x/y.zip".into(),
                file_name: "y.zip".into(),
                path: PathBuf::from("/nonexistent/y.zip"),
                category: Category::Archives,
                status: Status::Downloading,
                total: Some(100),
                received: 10,
                speed: 5,
                eta_secs: Some(18),
                connections: 2,
                segments: vec![Segment {
                    start: 0,
                    end: 49,
                    received: 10,
                }],
                resumable: true,
                error: None,
                added_at: 1,
            }]
        }
        fn has_active(&self) -> bool {
            true
        }
        fn settings(&self) -> Settings {
            self.settings.lock().unwrap().clone().unwrap_or_default()
        }
        fn set_settings(&self, s: Settings) {
            *self.settings.lock().unwrap() = Some(s);
        }
        fn open_path(&self, _: &Path, _: bool) {}
        fn choose_dir(&self) -> Option<String> {
            None
        }
    }

    fn ctx() -> Ctx<Mock> {
        Ctx {
            backend: Mock::default(),
            token: "tok123".into(),
            port: 47815,
        }
    }

    fn hdrs(token: Option<&str>, host: &str) -> Vec<(String, String)> {
        let mut h = vec![("Host".to_string(), host.to_string())];
        if let Some(t) = token {
            h.push(("X-Sushload-Token".to_string(), t.to_string()));
        }
        h
    }

    fn call(c: &Ctx<Mock>, m: &str, p: &str, token: Option<&str>, body: &str) -> Response {
        handle(c, m, p, &hdrs(token, "127.0.0.1:47815"), body.as_bytes())
    }

    fn settings_body(mc: u64, conns: u64) -> String {
        json!({"download_dir": "/tmp/dl", "sort_into_categories": true,
               "max_concurrent": mc, "connections": conns, "speed_limit": 0})
        .to_string()
    }

    #[test]
    fn token_required() {
        let c = ctx();
        assert_eq!(call(&c, "GET", "/api/state", None, "").status, 401);
        assert_eq!(
            call(&c, "GET", "/api/state", Some("wrong!"), "").status,
            401
        );
        assert_eq!(call(&c, "POST", "/api/pause_all", None, "").status, 401);
        let r = call(&c, "GET", "/api/state", Some("tok123"), "");
        assert_eq!(r.status, 200);
        let v: Value = serde_json::from_slice(&r.body).unwrap();
        assert_eq!(v["active"], true);
        assert_eq!(v["downloads"][0]["status"], "downloading");
        assert_eq!(v["downloads"][0]["category"], "archives");
        assert_eq!(v["settings"]["connections"], 8);
    }

    #[test]
    fn foreign_host_rejected() {
        let c = ctx();
        let r = handle(
            &c,
            "GET",
            "/api/state",
            &hdrs(Some("tok123"), "evil.example.com"),
            b"",
        );
        assert_eq!(r.status, 403);
        let r = handle(&c, "GET", "/", &hdrs(None, "evil.example.com:47815"), b"");
        assert_eq!(r.status, 403);
        let r = handle(
            &c,
            "GET",
            "/api/state",
            &hdrs(Some("tok123"), "127.0.0.1:9999"),
            b"",
        );
        assert_eq!(r.status, 403);
        let r = handle(
            &c,
            "GET",
            "/api/state",
            &hdrs(Some("tok123"), "localhost:47815"),
            b"",
        );
        assert_eq!(r.status, 200);
        assert_eq!(handle(&c, "GET", "/", &[], b"").status, 403);
    }

    #[test]
    fn add_two_urls_calls_engine_twice() {
        let c = ctx();
        let body =
            json!({"urls": "http://a.test/1.zip\n\n  http://b.test/2.zip \nnot-a-url"}).to_string();
        let r = call(&c, "POST", "/api/add", Some("tok123"), &body);
        assert_eq!(r.status, 200);
        let v: Value = serde_json::from_slice(&r.body).unwrap();
        assert_eq!(v["added"], 2);
        assert_eq!(v["errors"].as_array().unwrap().len(), 1);
        assert_eq!(
            *c.backend.added.lock().unwrap(),
            vec!["http://a.test/1.zip", "http://b.test/2.zip"]
        );
    }

    #[test]
    fn remove_passes_delete_flag() {
        let c = ctx();
        call(
            &c,
            "POST",
            "/api/remove",
            Some("tok123"),
            r#"{"id":3,"delete_file":true}"#,
        );
        call(&c, "POST", "/api/remove", Some("tok123"), r#"{"id":4}"#);
        assert_eq!(
            *c.backend.removed.lock().unwrap(),
            vec![(3, true), (4, false)]
        );
        assert_eq!(
            call(&c, "POST", "/api/remove", Some("tok123"), "{}").status,
            400
        );
        assert_eq!(
            call(&c, "POST", "/api/remove", Some("tok123"), "nope").status,
            400
        );
    }

    #[test]
    fn settings_validation() {
        let c = ctx();
        for (mc, cn) in [(0, 8), (11, 8), (3, 0), (3, 17)] {
            let r = call(
                &c,
                "POST",
                "/api/settings",
                Some("tok123"),
                &settings_body(mc, cn),
            );
            assert_eq!(r.status, 400, "mc={mc} conns={cn}");
        }
        assert!(c.backend.settings.lock().unwrap().is_none());
        let r = call(
            &c,
            "POST",
            "/api/settings",
            Some("tok123"),
            &settings_body(10, 16),
        );
        assert_eq!(r.status, 200);
        let s = c.backend.settings.lock().unwrap().clone().unwrap();
        assert_eq!((s.max_concurrent, s.connections), (10, 16));
        let rel = json!({"download_dir": "rel/dir", "sort_into_categories": true,
                         "max_concurrent": 3, "connections": 8, "speed_limit": 0});
        assert_eq!(
            call(
                &c,
                "POST",
                "/api/settings",
                Some("tok123"),
                &rel.to_string()
            )
            .status,
            400
        );
    }

    fn proxy_body(mode: Option<&str>, url: &str) -> String {
        let mut v = json!({"download_dir": "/tmp/dl", "sort_into_categories": true,
               "max_concurrent": 3, "connections": 8, "speed_limit": 0, "proxy_url": url});
        if let Some(m) = mode {
            v["proxy_mode"] = json!(m);
        }
        v.to_string()
    }

    #[test]
    fn proxy_settings() {
        let c = ctx();
        let r = call(
            &c,
            "POST",
            "/api/settings",
            Some("tok123"),
            &proxy_body(Some("custom"), "socks5h://u:p%40@h.example:1080"),
        );
        assert_eq!(r.status, 200);
        let v: Value = serde_json::from_slice(&r.body).unwrap();
        assert_eq!(v["settings"]["proxy_mode"], "custom");
        assert_eq!(
            v["settings"]["proxy_url"],
            "socks5h://u:p%40@h.example:1080"
        );

        let r = call(
            &c,
            "POST",
            "/api/settings",
            Some("tok123"),
            &proxy_body(Some("custom"), "ftp://h:21"),
        );
        assert_eq!(r.status, 400);
        let v: Value = serde_json::from_slice(&r.body).unwrap();
        assert!(
            v["error"]
                .as_str()
                .unwrap()
                .contains("Unsupported proxy type")
        );
        assert_eq!(
            call(
                &c,
                "POST",
                "/api/settings",
                Some("tok123"),
                &proxy_body(Some("custom"), "")
            )
            .status,
            400
        );

        for m in ["off", "system"] {
            let r = call(
                &c,
                "POST",
                "/api/settings",
                Some("tok123"),
                &proxy_body(Some(m), ""),
            );
            assert_eq!(r.status, 200, "{m}");
            let v: Value = serde_json::from_slice(&r.body).unwrap();
            assert_eq!(v["settings"]["proxy_mode"], m);
            assert_eq!(v["settings"]["proxy_url"], "");
        }
        // Missing mode keeps System.
        let r = call(
            &c,
            "POST",
            "/api/settings",
            Some("tok123"),
            &proxy_body(None, ""),
        );
        let v: Value = serde_json::from_slice(&r.body).unwrap();
        assert_eq!(v["settings"]["proxy_mode"], "system");
        assert_eq!(
            call(
                &c,
                "POST",
                "/api/settings",
                Some("tok123"),
                &proxy_body(Some("bogus"), "")
            )
            .status,
            400
        );
    }

    #[test]
    fn open_requires_finished_download() {
        let c = ctx();
        assert_eq!(
            call(&c, "POST", "/api/open", Some("tok123"), r#"{"id":7}"#).status,
            409
        );
        assert_eq!(
            call(&c, "POST", "/api/open", Some("tok123"), r#"{"id":99}"#).status,
            404
        );
    }

    #[test]
    fn static_files_content_types() {
        let c = ctx();
        let r = call(&c, "GET", "/?t=abc&app=1", None, "");
        assert_eq!(
            (r.status, r.content_type),
            (200, "text/html; charset=utf-8")
        );
        assert!(String::from_utf8_lossy(&r.body).contains("sushLoad"));
        let r = call(&c, "GET", "/style.css", None, "");
        assert_eq!((r.status, r.content_type), (200, "text/css; charset=utf-8"));
        let r = call(&c, "GET", "/app.js", None, "");
        assert_eq!(
            (r.status, r.content_type),
            (200, "text/javascript; charset=utf-8")
        );
        assert_eq!(call(&c, "GET", "/nope", None, "").status, 404);
        assert_eq!(call(&c, "GET", "/api/nope", Some("tok123"), "").status, 404);
    }

    #[test]
    fn token_is_32_hex() {
        let t = random_token().unwrap();
        assert_eq!(t.len(), 32);
        assert!(t.bytes().all(|b| b.is_ascii_hexdigit()));
    }
}
