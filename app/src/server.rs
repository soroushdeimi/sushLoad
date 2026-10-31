//! Local HTTP API + embedded static UI. One background accept thread (blocking `recv`), one
//! short-lived thread per request. Nothing runs while idle.

use serde_json::{Value, json};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use sushload_core::{
    AddRequest, Category, DownloadId, DownloadView, Engine, ProbeInfo, ProxyMode, Settings, Status,
    categories, sanitize_file_name, validate_proxy_url, validate_request,
};

const INDEX_HTML: &str = include_str!("../web/index.html");
const STYLE_CSS: &str = include_str!("../web/style.css");
const APP_JS: &str = include_str!("../web/app.js");

const MAX_BODY: usize = 1024 * 1024;
const MAX_ITEMS: usize = 500;
/// Hand-overs waiting for an answer in the confirmation dialog.
const MAX_PENDING: usize = 100;

/// What the HTTP layer needs from the engine and the OS. `Engine` implements it; tests use a mock.
pub trait Backend: Send + Sync + 'static {
    /// A download handed over by the browser extension (file name, Referer, cookies, UA).
    fn add_request(&self, req: AddRequest) -> Result<u64, String>;
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
    /// Shows the bundled browser extension folder in Finder.
    fn reveal_extension(&self) -> Result<(), String>;
    /// Quick `Range: bytes=0-0` look at a URL for the confirmation dialog.
    fn probe(&self, req: &AddRequest) -> Result<ProbeInfo, String>;
    /// Shows the log folder in Finder.
    fn open_logs(&self) -> Result<(), String>;
}

impl Backend for Engine {
    fn add_request(&self, req: AddRequest) -> Result<u64, String> {
        Engine::add_request(self, req).map(|id| id.0)
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
    fn probe(&self, req: &AddRequest) -> Result<ProbeInfo, String> {
        Engine::probe(self, req)
    }
    fn open_logs(&self) -> Result<(), String> {
        let dir = crate::logger::log_dir();
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        Command::new("open")
            .arg(&dir)
            .spawn()
            .map(|mut c| {
                let _ = c.wait();
            })
            .map_err(|e| e.to_string())
    }
    fn reveal_extension(&self) -> Result<(), String> {
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let dir = crate::nativehost::extension_dir(&exe)
            .ok_or("the extension folder was not found next to the app")?;
        Command::new("open")
            .arg("-R")
            .arg(&dir)
            .spawn()
            .map(|mut c| {
                let _ = c.wait();
            })
            .map_err(|e| e.to_string())
    }
}

/// Called when a hand-over from the browser lands in the confirmation list; the argument is
/// true when the list was empty before (nobody is looking at a dialog yet).
pub type Notify = Box<dyn Fn(bool) + Send + Sync>;

/// A hand-over waiting for the user's answer. Held in memory only (cookies included), so a
/// quit forgets it. More than one item means a "Download all links" batch.
struct Pending {
    id: u64,
    items: Vec<AddRequest>,
}

pub struct Ctx<B: Backend> {
    pub backend: B,
    pub token: String,
    pub port: u16,
    pending: Mutex<Vec<Pending>>,
    next_pending: Mutex<u64>,
    notify: Notify,
}

impl<B: Backend> Ctx<B> {
    pub fn new(backend: B, token: String, port: u16, notify: Notify) -> Self {
        Ctx {
            backend,
            token,
            port,
            pending: Mutex::new(Vec::new()),
            next_pending: Mutex::new(1),
            notify,
        }
    }

    fn pending(&self) -> std::sync::MutexGuard<'_, Vec<Pending>> {
        self.pending.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// What to do when something is waiting for the user.
pub enum Attention {
    /// Bring the native window to the front and let its page open the dialog.
    Window,
    /// Open the web UI in the browser (unless `--no-open`).
    Browser { open: bool },
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
        "confirm_new_downloads": s.confirm_new_downloads,
        "notify_on_complete": s.notify_on_complete,
        "category_dirs": s.category_dirs.iter()
            .map(|(c, d)| (c.label().to_lowercase(), json!(d.to_string_lossy())))
            .collect::<serde_json::Map<String, Value>>(),
    })
}

/// Folder each category would use right now (for the dialog's "Save to").
fn default_dirs_json(s: &Settings) -> Value {
    Category::ALL
        .iter()
        .map(|c| {
            (
                c.label().to_lowercase(),
                json!(categories::dir_for(s, *c).to_string_lossy()),
            )
        })
        .collect::<serde_json::Map<String, Value>>()
        .into()
}

fn category_from(label: &str) -> Option<Category> {
    Category::ALL
        .into_iter()
        .find(|c| c.label().eq_ignore_ascii_case(label))
}

fn pending_json(list: &[Pending]) -> Value {
    list.iter()
        .map(|p| {
            json!({
                "id": p.id,
                "batch": p.items.len() > 1,
                // No cookies here: they stay in memory on this side.
                "items": p.items.iter().enumerate().map(|(i, r)| json!({
                    "index": i,
                    "url": r.url,
                    "file_name": r.file_name,
                    "referer": r.referer,
                })).collect::<Vec<_>>(),
            })
        })
        .collect::<Vec<_>>()
        .into()
}

/// Parses and validates a settings object from the UI. Newer optional fields keep their value
/// from `base` when the object leaves them out.
pub fn parse_settings(v: &Value, base: &Settings) -> Result<Settings, String> {
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
    let flag = |key: &str, fallback: bool| v.get(key).and_then(Value::as_bool).unwrap_or(fallback);
    let category_dirs = match v.get("category_dirs") {
        None | Some(Value::Null) => base.category_dirs.clone(),
        Some(Value::Object(map)) => {
            let mut out = std::collections::HashMap::new();
            for (key, value) in map {
                let category =
                    category_from(key).ok_or_else(|| format!("unknown category \"{key}\""))?;
                let path = value.as_str().unwrap_or("").trim();
                if path.is_empty() {
                    continue;
                }
                if !Path::new(path).is_absolute() {
                    return Err(format!("the folder for {key} must be an absolute path"));
                }
                out.insert(category, PathBuf::from(path));
            }
            out
        }
        Some(_) => return Err("category_dirs must be an object".into()),
    };
    Ok(Settings {
        download_dir: dir,
        sort_into_categories: sort,
        max_concurrent: mc as u32,
        connections: conns as u32,
        speed_limit: limit,
        proxy_mode,
        proxy_url,
        confirm_new_downloads: flag("confirm_new_downloads", base.confirm_new_downloads),
        category_dirs,
        notify_on_complete: flag("notify_on_complete", base.notify_on_complete),
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
            // Browsers ask for it on every page load; answer quietly instead of logging a 404.
            "/favicon.ico" => Response {
                status: 204,
                content_type: "image/x-icon",
                body: Vec::new(),
            },
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
                "default_dirs": default_dirs_json(&b.settings()),
                "pending": pending_json(&ctx.pending()),
            }),
        ),
        ("POST", "/api/add") => {
            let Some(text) = parsed.get("urls").and_then(Value::as_str) else {
                return err(400, "urls missing");
            };
            let items = text
                .lines()
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .map(|url| AddRequest {
                    url: url.to_string(),
                    ..AddRequest::default()
                })
                .collect();
            // The page that typed the URLs opens the dialog itself: no attention needed.
            add_or_queue(ctx, items, false)
        }
        ("POST", "/api/add_request") => {
            let Some(items) = parsed.get("items").and_then(Value::as_array) else {
                return err(400, "items missing");
            };
            if items.len() > MAX_ITEMS {
                return err(400, "too many items");
            }
            let mut requests = Vec::new();
            let mut bad = 0;
            for item in items {
                match serde_json::from_value::<AddRequest>(item.clone()) {
                    Ok(req) => requests.push(req),
                    Err(_) => bad += 1,
                }
            }
            let mut r = add_or_queue(ctx, requests, true);
            if bad > 0
                && let Ok(mut v) = serde_json::from_slice::<Value>(&r.body)
            {
                if let Some(errors) = v["errors"].as_array_mut() {
                    errors.extend((0..bad).map(|_| json!("invalid item")));
                }
                r.body = v.to_string().into_bytes();
            }
            r
        }
        ("POST", "/api/probe") => {
            let (Some(pid), Some(index)) = (
                parsed.get("pending_id").and_then(Value::as_u64),
                parsed.get("index").and_then(Value::as_u64),
            ) else {
                return err(400, "pending_id and index missing");
            };
            let request = {
                let list = ctx.pending();
                list.iter()
                    .find(|p| p.id == pid)
                    .and_then(|p| p.items.get(index as usize))
                    .cloned()
            };
            let Some(request) = request else {
                return err(404, "no such pending download");
            };
            match b.probe(&request) {
                Ok(info) => json_resp(
                    200,
                    json!({
                        "file_name": info.file_name,
                        "total": info.total,
                        "resumable": info.resumable,
                        "category": info.category.label().to_lowercase(),
                        "dir": info.dir.to_string_lossy(),
                        "exists": info.exists,
                    }),
                ),
                Err(e) => err(502, &sushload_core::redact::redact_text(&e)),
            }
        }
        ("POST", "/api/confirm") => confirm(ctx, &parsed),
        ("POST", "/api/discard") => {
            let Some(pid) = parsed.get("pending_id").and_then(Value::as_u64) else {
                return err(400, "pending_id missing");
            };
            let mut list = ctx.pending();
            let before = list.len();
            list.retain(|p| p.id != pid);
            if list.len() == before {
                return err(404, "no such pending download");
            }
            log::info!(target: "server", "discard pending={pid}");
            json_resp(200, json!({ "ok": true }))
        }
        ("POST", "/api/exists") => {
            let (Some(dir), Some(name)) = (
                parsed.get("dir").and_then(Value::as_str),
                parsed.get("file_name").and_then(Value::as_str),
            ) else {
                return err(400, "dir and file_name missing");
            };
            if !Path::new(dir).is_absolute() {
                return err(400, "dir must be an absolute path");
            }
            let taken = categories::name_taken(Path::new(dir), &sanitize_file_name(name));
            json_resp(200, json!({ "exists": taken }))
        }
        ("POST", "/api/open_logs") => match b.open_logs() {
            Ok(()) => json_resp(200, json!({ "ok": true })),
            Err(e) => err(500, &e),
        },
        ("POST", "/api/reveal_extension") => match b.reveal_extension() {
            Ok(()) => json_resp(200, json!({ "ok": true })),
            Err(e) => err(404, &e),
        },
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
        ("POST", "/api/settings") => match parse_settings(&parsed, &b.settings()) {
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

/// Starts the requests right away, or (confirmation on) parks them as one pending entry.
fn add_or_queue<B: Backend>(ctx: &Ctx<B>, items: Vec<AddRequest>, notify: bool) -> Response {
    let b = &ctx.backend;
    let mut errors = Vec::new();
    if b.settings().confirm_new_downloads {
        let mut valid = Vec::new();
        for req in items {
            match validate_request(&req) {
                Ok(()) => valid.push(req),
                // Errors name the URL only; cookies never leave the request.
                Err(e) => errors.push(format!(
                    "{}: {e}",
                    sushload_core::redact::redact_url(&req.url)
                )),
            }
        }
        let queued = valid.len();
        if queued > 0 {
            let first = {
                let mut list = ctx.pending();
                if list.len() >= MAX_PENDING {
                    return err(429, "too many downloads are waiting for confirmation");
                }
                let first = list.is_empty();
                let mut next = ctx.next_pending.lock().unwrap_or_else(|e| e.into_inner());
                let id = *next;
                *next += 1;
                log::info!(target: "server", "queued {queued} item(s) for confirmation (pending={id})");
                list.push(Pending { id, items: valid });
                first
            };
            if notify {
                (ctx.notify)(first);
            }
        }
        return json_resp(
            200,
            json!({ "added": 0, "queued": queued, "confirm": true, "errors": errors }),
        );
    }
    let mut added = 0u32;
    for req in items {
        let url = req.url.clone();
        match b.add_request(req) {
            Ok(_) => added += 1,
            Err(e) => errors.push(format!("{}: {e}", sushload_core::redact::redact_url(&url))),
        }
    }
    json_resp(
        200,
        json!({ "added": added, "queued": 0, "confirm": false, "errors": errors }),
    )
}

/// The dialog's answer for one pending entry: which items to start (with their final name,
/// category, folder and overwrite choice), whether to add them paused, and settings to remember.
fn confirm<B: Backend>(ctx: &Ctx<B>, body: &Value) -> Response {
    let b = &ctx.backend;
    let Some(pid) = body.get("pending_id").and_then(Value::as_u64) else {
        return err(400, "pending_id missing");
    };
    let Some(specs) = body.get("items").and_then(Value::as_array) else {
        return err(400, "items missing");
    };
    let entry = {
        let mut list = ctx.pending();
        match list.iter().position(|p| p.id == pid) {
            Some(i) => list.remove(i),
            None => return err(404, "no such pending download"),
        }
    };
    let paused = body
        .get("start_paused")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let shared_dir = body
        .get("dir")
        .and_then(Value::as_str)
        .filter(|d| !d.is_empty());
    let mut added = 0u32;
    let mut errors = Vec::new();
    let mut remember: Option<(Category, PathBuf)> = None;
    for spec in specs {
        let Some(mut req) = spec
            .get("index")
            .and_then(Value::as_u64)
            .and_then(|i| entry.items.get(i as usize))
            .cloned()
        else {
            errors.push("invalid item".to_string());
            continue;
        };
        if let Some(name) = spec.get("file_name").and_then(Value::as_str)
            && !name.trim().is_empty()
        {
            req.file_name = Some(name.to_string());
        }
        req.category = spec
            .get("category")
            .and_then(Value::as_str)
            .and_then(category_from);
        req.dir = spec
            .get("dir")
            .and_then(Value::as_str)
            .or(shared_dir)
            .filter(|d| !d.is_empty())
            .map(PathBuf::from);
        req.overwrite = spec
            .get("overwrite")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        req.start_paused = paused;
        if entry.items.len() == 1
            && let Some(dir) = &req.dir
        {
            let category = req
                .category
                .unwrap_or_else(|| categories::classify(req.file_name.as_deref().unwrap_or("")));
            remember = Some((category, dir.clone()));
        }
        let url = req.url.clone();
        match b.add_request(req) {
            Ok(_) => added += 1,
            Err(e) => errors.push(format!("{}: {e}", sushload_core::redact::redact_url(&url))),
        }
    }
    let remember_dir = body
        .get("remember_dir")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let dont_ask = body
        .get("dont_ask")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if (remember_dir && remember.is_some()) || dont_ask {
        let mut settings = b.settings();
        if remember_dir && let Some((category, dir)) = remember {
            settings.category_dirs.insert(category, dir);
        }
        if dont_ask {
            settings.confirm_new_downloads = false;
        }
        b.set_settings(settings);
    }
    log::info!(
        target: "server",
        "confirm pending={pid} added={added} paused={paused} errors={}",
        errors.len()
    );
    json_resp(200, json!({ "added": added, "errors": errors }))
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
pub fn start(
    engine: Engine,
    preferred_port: u16,
    attention: Attention,
) -> Result<RunningServer, String> {
    let token = random_token().map_err(|e| format!("cannot read /dev/urandom: {e}"))?;
    let server = tiny_http::Server::http(("127.0.0.1", preferred_port))
        .or_else(|_| tiny_http::Server::http(("127.0.0.1", 0)))
        .map_err(|e| format!("cannot bind a local port: {e}"))?;
    let port = server
        .server_addr()
        .to_ip()
        .map(|a| a.port())
        .ok_or("server has no IP address")?;
    let page = format!("http://127.0.0.1:{port}/?t={token}");
    let notify: Notify = match attention {
        Attention::Window => Box::new(|_| crate::window::request_attention()),
        Attention::Browser { open } => Box::new(move |first| {
            if first && open {
                crate::open_in_browser(&page);
            }
        }),
    };
    let ctx = Arc::new(Ctx::new(engine, token.clone(), port, notify));
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
    if r.status >= 400 {
        let path = url.split(['?', '#']).next().unwrap_or("");
        log::warn!(target: "server", "{method} {path} -> {}", r.status);
    }
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
        requests: Mutex<Vec<AddRequest>>,
        settings: Mutex<Option<Settings>>,
        removed: Mutex<Vec<(u64, bool)>>,
    }

    impl Backend for Mock {
        fn add_request(&self, req: AddRequest) -> Result<u64, String> {
            if !req.url.starts_with("http") {
                return Err("invalid URL".into());
            }
            let mut a = self.requests.lock().unwrap();
            a.push(req);
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
        fn reveal_extension(&self) -> Result<(), String> {
            Ok(())
        }
        fn probe(&self, req: &AddRequest) -> Result<ProbeInfo, String> {
            if req.url.contains("broken") {
                return Err("HTTP 403".into());
            }
            Ok(ProbeInfo {
                file_name: req.file_name.clone().unwrap_or_else(|| "probed.zip".into()),
                total: Some(1234),
                resumable: true,
                category: Category::Archives,
                dir: PathBuf::from("/tmp/dl/Archives"),
                exists: true,
            })
        }
        fn open_logs(&self) -> Result<(), String> {
            Ok(())
        }
    }

    /// Confirmation on (the default): hand-overs wait in the pending list. The second value
    /// records every attention call (true = the list was empty before).
    fn ctx_notes() -> (Ctx<Mock>, Arc<Mutex<Vec<bool>>>) {
        let notes: Arc<Mutex<Vec<bool>>> = Arc::default();
        let sink = Arc::clone(&notes);
        let c = Ctx::new(
            Mock::default(),
            "tok123".into(),
            47815,
            Box::new(move |first| sink.lock().unwrap().push(first)),
        );
        (c, notes)
    }

    fn ctx() -> Ctx<Mock> {
        ctx_notes().0
    }

    /// Confirmation off: hand-overs go straight to the engine.
    fn ctx_direct() -> Ctx<Mock> {
        let c = ctx();
        c.backend.set_settings(Settings {
            confirm_new_downloads: false,
            ..Settings::default()
        });
        c
    }

    fn body_of(r: &Response) -> Value {
        serde_json::from_slice(&r.body).unwrap()
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
        let c = ctx_direct();
        let body =
            json!({"urls": "http://a.test/1.zip\n\n  http://b.test/2.zip \nnot-a-url"}).to_string();
        let r = call(&c, "POST", "/api/add", Some("tok123"), &body);
        assert_eq!(r.status, 200);
        let v: Value = serde_json::from_slice(&r.body).unwrap();
        assert_eq!(v["added"], 2);
        assert_eq!(v["errors"].as_array().unwrap().len(), 1);
        let urls: Vec<String> = c
            .backend
            .requests
            .lock()
            .unwrap()
            .iter()
            .map(|r| r.url.clone())
            .collect();
        assert_eq!(urls, vec!["http://a.test/1.zip", "http://b.test/2.zip"]);
    }

    #[test]
    fn add_request_passes_browser_context() {
        let c = ctx_direct();
        assert_eq!(call(&c, "POST", "/api/add_request", None, "{}").status, 401);
        let body = json!({"items": [
            {"url": "https://a.test/f.zip", "file_name": "f.zip", "referer": "https://a.test/",
             "cookies": "sid=SECRET1; x=y", "user_agent": "UA/1"},
            {"url": "https://b.test/g.iso"},
            {"url": "ftp://c.test/h.bin", "cookies": "sid=SECRET2"},
            {"url": 5}
        ]})
        .to_string();
        let r = call(&c, "POST", "/api/add_request", Some("tok123"), &body);
        assert_eq!(r.status, 200);
        let text = String::from_utf8(r.body.clone()).unwrap();
        assert!(
            !text.contains("SECRET"),
            "cookies leaked in the reply: {text}"
        );
        let v: Value = serde_json::from_slice(&r.body).unwrap();
        assert_eq!(v["added"], 2);
        assert_eq!(v["errors"].as_array().unwrap().len(), 2);
        let got = c.backend.requests.lock().unwrap().clone();
        assert_eq!(got[0].file_name.as_deref(), Some("f.zip"));
        assert_eq!(got[0].referer.as_deref(), Some("https://a.test/"));
        assert_eq!(got[0].cookies.as_deref(), Some("sid=SECRET1; x=y"));
        assert_eq!(got[0].user_agent.as_deref(), Some("UA/1"));
        assert_eq!(got[1].cookies, None);
        assert_eq!(
            call(&c, "POST", "/api/add_request", Some("tok123"), "{}").status,
            400
        );
        let many = json!({"items": vec![json!({"url": "http://x.test/a"}); MAX_ITEMS + 1]});
        assert_eq!(
            call(
                &c,
                "POST",
                "/api/add_request",
                Some("tok123"),
                &many.to_string()
            )
            .status,
            400
        );
    }

    #[test]
    fn reveal_extension_needs_token() {
        let c = ctx();
        assert_eq!(
            call(&c, "POST", "/api/reveal_extension", None, "").status,
            401
        );
        assert_eq!(
            call(&c, "POST", "/api/reveal_extension", Some("tok123"), "").status,
            200
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

    fn post(c: &Ctx<Mock>, path: &str, body: Value) -> Response {
        call(c, "POST", path, Some("tok123"), &body.to_string())
    }

    fn state(c: &Ctx<Mock>) -> Value {
        body_of(&call(c, "GET", "/api/state", Some("tok123"), ""))
    }

    #[test]
    fn confirm_on_parks_hand_overs_instead_of_starting_them() {
        let (c, notes) = ctx_notes();
        let r = post(
            &c,
            "/api/add_request",
            json!({"items": [
                {"url": "https://a.test/one.zip", "cookies": "sid=SECRET9", "referer": "https://a.test/"},
                {"url": "ftp://bad.test/x"},
            ]}),
        );
        assert_eq!(r.status, 200);
        let v = body_of(&r);
        assert_eq!(
            (v["added"].clone(), v["queued"].clone()),
            (json!(0), json!(1))
        );
        assert_eq!(v["confirm"], true);
        assert_eq!(v["errors"].as_array().unwrap().len(), 1);
        assert!(
            c.backend.requests.lock().unwrap().is_empty(),
            "engine must not see it yet"
        );

        let st = state(&c);
        let text = serde_json::to_string(&st).unwrap();
        assert!(
            !text.contains("SECRET9"),
            "cookie leaked to the page: {text}"
        );
        assert_eq!(st["pending"].as_array().unwrap().len(), 1);
        assert_eq!(st["pending"][0]["batch"], false);
        assert_eq!(
            st["pending"][0]["items"][0]["url"],
            "https://a.test/one.zip"
        );
        assert_eq!(st["settings"]["confirm_new_downloads"], true);
        assert_eq!(
            st["default_dirs"]["archives"],
            "/tmp/dl/Archives".replace(
                "/tmp/dl",
                &Settings::default().download_dir.to_string_lossy()
            )
        );

        // A second arrival while the first still waits is not "first".
        post(
            &c,
            "/api/add_request",
            json!({"items": [{"url": "https://b.test/two.zip"}]}),
        );
        assert_eq!(*notes.lock().unwrap(), vec![true, false]);
        assert_eq!(state(&c)["pending"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn add_url_dialog_queues_a_batch_without_asking_for_attention() {
        let (c, notes) = ctx_notes();
        let r = post(
            &c,
            "/api/add",
            json!({"urls": "http://a.test/1.zip\nhttp://b.test/2.zip\nnope"}),
        );
        let v = body_of(&r);
        assert_eq!(v["queued"], 2);
        assert_eq!(v["errors"].as_array().unwrap().len(), 1);
        assert!(notes.lock().unwrap().is_empty());
        let st = state(&c);
        assert_eq!(st["pending"][0]["batch"], true);
        assert_eq!(st["pending"][0]["items"].as_array().unwrap().len(), 2);
        assert!(c.backend.requests.lock().unwrap().is_empty());
    }

    #[test]
    fn confirm_starts_with_the_chosen_name_folder_and_keeps_cookies() {
        let c = ctx();
        post(
            &c,
            "/api/add_request",
            json!({"items": [{"url": "https://a.test/one.zip", "cookies": "sid=KEEPME", "user_agent": "UA/2"}]}),
        );
        let pid = state(&c)["pending"][0]["id"].as_u64().unwrap();
        let r = post(
            &c,
            "/api/confirm",
            json!({"pending_id": pid, "start_paused": true, "remember_dir": true, "dont_ask": true,
                   "items": [{"index": 0, "file_name": "renamed.zip", "category": "archives",
                              "dir": "/Volumes/Big/Zips", "overwrite": true}]}),
        );
        assert_eq!(r.status, 200);
        assert_eq!(body_of(&r)["added"], 1);
        let got = c.backend.requests.lock().unwrap().clone();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].file_name.as_deref(), Some("renamed.zip"));
        assert_eq!(got[0].dir, Some(PathBuf::from("/Volumes/Big/Zips")));
        assert_eq!(got[0].category, Some(Category::Archives));
        assert!(got[0].overwrite && got[0].start_paused);
        assert_eq!(got[0].cookies.as_deref(), Some("sid=KEEPME"));
        assert_eq!(got[0].user_agent.as_deref(), Some("UA/2"));
        let s = c.backend.settings();
        assert!(!s.confirm_new_downloads, "Don't ask again");
        assert_eq!(
            s.category_dirs.get(&Category::Archives),
            Some(&PathBuf::from("/Volumes/Big/Zips"))
        );
        assert!(state(&c)["pending"].as_array().unwrap().is_empty());
        assert_eq!(
            post(&c, "/api/confirm", json!({"pending_id": pid, "items": []})).status,
            404
        );
    }

    #[test]
    fn batch_confirm_starts_only_checked_items_in_one_shared_folder() {
        let c = ctx();
        post(
            &c,
            "/api/add_request",
            json!({"items": [{"url": "https://a.test/1.zip"}, {"url": "https://a.test/2.mp4"}, {"url": "https://a.test/3.pdf"}]}),
        );
        let pid = state(&c)["pending"][0]["id"].as_u64().unwrap();
        post(
            &c,
            "/api/confirm",
            json!({"pending_id": pid, "dir": "/tmp/shared", "remember_dir": true,
                   "items": [{"index": 0, "category": "archives"}, {"index": 2, "category": "documents"}]}),
        );
        let got = c.backend.requests.lock().unwrap().clone();
        let urls: Vec<_> = got.iter().map(|r| r.url.as_str()).collect();
        assert_eq!(urls, ["https://a.test/1.zip", "https://a.test/3.pdf"]);
        assert!(
            got.iter()
                .all(|r| r.dir == Some(PathBuf::from("/tmp/shared")))
        );
        // A shared folder is never remembered for one category.
        assert!(c.backend.settings().category_dirs.is_empty());
        assert!(c.backend.settings().confirm_new_downloads);
    }

    #[test]
    fn discard_drops_the_pending_entry() {
        let c = ctx();
        post(
            &c,
            "/api/add_request",
            json!({"items": [{"url": "https://a.test/x.zip"}]}),
        );
        let pid = state(&c)["pending"][0]["id"].as_u64().unwrap();
        assert_eq!(
            post(&c, "/api/discard", json!({"pending_id": pid})).status,
            200
        );
        assert!(state(&c)["pending"].as_array().unwrap().is_empty());
        assert!(c.backend.requests.lock().unwrap().is_empty());
        assert_eq!(
            post(&c, "/api/discard", json!({"pending_id": pid})).status,
            404
        );
        assert_eq!(post(&c, "/api/discard", json!({})).status, 400);
        assert_eq!(call(&c, "POST", "/api/discard", None, "{}").status, 401);
    }

    #[test]
    fn probe_uses_the_stored_request() {
        let c = ctx();
        post(
            &c,
            "/api/add_request",
            json!({"items": [{"url": "https://a.test/x.zip"}, {"url": "https://a.test/broken"}]}),
        );
        let pid = state(&c)["pending"][0]["id"].as_u64().unwrap();
        let r = post(&c, "/api/probe", json!({"pending_id": pid, "index": 0}));
        let v = body_of(&r);
        assert_eq!(r.status, 200);
        assert_eq!(
            (
                v["total"].clone(),
                v["resumable"].clone(),
                v["exists"].clone()
            ),
            (json!(1234), json!(true), json!(true))
        );
        assert_eq!(v["category"], "archives");
        let r = post(&c, "/api/probe", json!({"pending_id": pid, "index": 1}));
        assert_eq!(r.status, 502);
        assert_eq!(body_of(&r)["error"], "HTTP 403");
        assert_eq!(
            post(&c, "/api/probe", json!({"pending_id": 999, "index": 0})).status,
            404
        );
    }

    #[test]
    fn confirm_off_sends_straight_to_the_engine() {
        let c = ctx_direct();
        let r = post(
            &c,
            "/api/add_request",
            json!({"items": [{"url": "https://a.test/x.zip"}]}),
        );
        let v = body_of(&r);
        assert_eq!(
            (
                v["added"].clone(),
                v["queued"].clone(),
                v["confirm"].clone()
            ),
            (json!(1), json!(0), json!(false))
        );
        assert_eq!(c.backend.requests.lock().unwrap().len(), 1);
        assert!(state(&c)["pending"].as_array().unwrap().is_empty());
    }

    #[test]
    fn exists_and_new_settings_fields() {
        let c = ctx();
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("here.bin"), b"x").unwrap();
        let d = dir.path().to_string_lossy().to_string();
        let r = post(
            &c,
            "/api/exists",
            json!({"dir": d, "file_name": "here.bin"}),
        );
        assert_eq!(body_of(&r)["exists"], true);
        let r = post(
            &c,
            "/api/exists",
            json!({"dir": d, "file_name": "nope.bin"}),
        );
        assert_eq!(body_of(&r)["exists"], false);
        assert_eq!(
            post(&c, "/api/exists", json!({"dir": "rel", "file_name": "a"})).status,
            400
        );

        let mut body: Value = serde_json::from_str(&settings_body(3, 8)).unwrap();
        body["confirm_new_downloads"] = json!(false);
        body["notify_on_complete"] = json!(false);
        body["category_dirs"] = json!({"video": "/Volumes/M", "images": ""});
        let r = post(&c, "/api/settings", body.clone());
        assert_eq!(r.status, 200);
        let v = body_of(&r);
        assert_eq!(v["settings"]["confirm_new_downloads"], false);
        assert_eq!(v["settings"]["notify_on_complete"], false);
        assert_eq!(
            v["settings"]["category_dirs"],
            json!({"video": "/Volumes/M"})
        );
        // Leaving the new keys out keeps what is stored.
        let r = call(
            &c,
            "POST",
            "/api/settings",
            Some("tok123"),
            &settings_body(3, 8),
        );
        let v = body_of(&r);
        assert_eq!(v["settings"]["confirm_new_downloads"], false);
        assert_eq!(
            v["settings"]["category_dirs"],
            json!({"video": "/Volumes/M"})
        );
        body["category_dirs"] = json!({"video": "relative"});
        assert_eq!(post(&c, "/api/settings", body.clone()).status, 400);
        body["category_dirs"] = json!({"bogus": "/x"});
        assert_eq!(post(&c, "/api/settings", body).status, 400);
        assert_eq!(post(&c, "/api/open_logs", json!({})).status, 200);
    }

    #[test]
    fn token_is_32_hex() {
        let t = random_token().unwrap();
        assert_eq!(t.len(), 32);
        assert!(t.bytes().all(|b| b.is_ascii_hexdigit()));
    }
}
