//! Chrome native-messaging host: `sushload --native-host chrome-extension://<id>/`.
//!
//! Protocol: each message is a 4-byte native-endian length followed by that many bytes of JSON,
//! on stdin (extension -> host) and stdout (host -> extension). The extension uses
//! `sendNativeMessage`, so there is exactly one request and one reply per process.
//! Cookies travel through here; they are never logged and never written to disk.

use serde_json::{Value, json};
use std::io::{self, Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub const HOST_NAME: &str = "com.soroushdeimi.sushload";
pub const EXTENSION_ID: &str = "ibiipbpgmolcjidbajmjlkjcnngphfok";
/// Chrome refuses messages from the host above 1 MB and the extension sends at most 1 MB.
pub const MAX_MESSAGE: usize = 1024 * 1024;
const LAUNCH_WAIT: Duration = Duration::from_secs(8);

/// Browser data dirs (relative to `~/Library/Application Support`) that read native-messaging
/// manifests from `<dir>/NativeMessagingHosts/`.
pub const BROWSER_DIRS: [&str; 5] = [
    "Google/Chrome",
    "Google/Chrome Beta",
    "Chromium",
    "BraveSoftware/Brave-Browser",
    "Microsoft Edge",
];

// ---------- framing ----------

/// Reads one framed message. `Ok(None)` on a clean EOF before any byte.
pub fn read_message<R: Read>(r: &mut R) -> Result<Option<Value>, String> {
    let mut len = [0u8; 4];
    match r.read(&mut len[..1]) {
        Ok(0) => return Ok(None),
        Ok(_) => {}
        Err(e) => return Err(format!("read: {e}")),
    }
    r.read_exact(&mut len[1..])
        .map_err(|e| format!("truncated length: {e}"))?;
    let n = u32::from_ne_bytes(len) as usize;
    if n > MAX_MESSAGE {
        return Err(format!("message too large ({n} bytes)"));
    }
    let mut buf = vec![0u8; n];
    r.read_exact(&mut buf)
        .map_err(|e| format!("truncated message: {e}"))?;
    serde_json::from_slice(&buf)
        .map(Some)
        .map_err(|_| "message is not valid JSON".to_string())
}

pub fn write_message<W: Write>(w: &mut W, v: &Value) -> Result<(), String> {
    let body = v.to_string().into_bytes();
    if body.len() > MAX_MESSAGE {
        return Err("reply too large".into());
    }
    w.write_all(&(body.len() as u32).to_ne_bytes())
        .and_then(|_| w.write_all(&body))
        .and_then(|_| w.flush())
        .map_err(|e| format!("write: {e}"))
}

// ---------- talking to the running app ----------

fn http_post(port: u16, token: &str, path: &str, body: &str) -> Result<Value, String> {
    http_call("POST", port, token, path, body)
}

fn http_call(
    method: &str,
    port: u16,
    token: &str,
    path: &str,
    body: &str,
) -> Result<Value, String> {
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    let mut s = TcpStream::connect_timeout(&addr, Duration::from_secs(2))
        .map_err(|e| format!("cannot reach sushLoad: {e}"))?;
    let t = Some(Duration::from_secs(20));
    let _ = s.set_read_timeout(t);
    let _ = s.set_write_timeout(t);
    let req = format!(
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nX-Sushload-Token: {token}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    s.write_all(req.as_bytes())
        .and_then(|_| s.write_all(body.as_bytes()))
        .map_err(|e| format!("cannot send to sushLoad: {e}"))?;
    let mut raw = Vec::new();
    s.read_to_end(&mut raw)
        .map_err(|e| format!("no answer from sushLoad: {e}"))?;
    parse_http_response(&raw)
}

fn parse_http_response(raw: &[u8]) -> Result<Value, String> {
    let split = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .ok_or("bad answer from sushLoad")?;
    let head = String::from_utf8_lossy(&raw[..split]);
    let status: u16 = head
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .ok_or("bad answer from sushLoad")?;
    let v: Value = serde_json::from_slice(&raw[split + 4..]).unwrap_or(Value::Null);
    if status == 200 {
        Ok(v)
    } else {
        Err(v
            .get("error")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| format!("sushLoad answered HTTP {status}")))
    }
}

fn launch_app() {
    let opened = |args: &[&str]| {
        Command::new("open")
            .args(args)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    };
    if opened(&["-b", crate::BUNDLE_ID]) || opened(&["-a", "sushLoad"]) {
        return;
    }
    if let Ok(exe) = std::env::current_exe() {
        use std::os::unix::process::CommandExt;
        let _ = Command::new(exe)
            .args(["--web", "--no-open"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .process_group(0)
            .spawn();
    }
}

/// Port + token of a live instance, launching the app first when none runs.
fn ensure_running(dir: &Path) -> Result<(u16, String), String> {
    if let Some(r) = crate::running_instance(dir) {
        return Ok(r);
    }
    launch_app();
    let start = Instant::now();
    while start.elapsed() < LAUNCH_WAIT {
        std::thread::sleep(Duration::from_millis(250));
        if let Some(r) = crate::running_instance(dir) {
            return Ok(r);
        }
    }
    Err("sushLoad did not start in time".into())
}

// ---------- message handling ----------

pub fn handle_message(dir: &Path, msg: &Value) -> Value {
    let version = env!("CARGO_PKG_VERSION");
    match msg.get("type").and_then(Value::as_str) {
        Some("ping") => {
            log::debug!(target: "host", "message type=ping");
            let instance = crate::running_instance(dir);
            // Does the app ask before starting a download? The popup words itself accordingly.
            let confirm = instance
                .as_ref()
                .and_then(|(port, token)| http_call("GET", *port, token, "/api/state", "").ok())
                .and_then(|state| state["settings"]["confirm_new_downloads"].as_bool())
                .unwrap_or(false);
            json!({ "ok": true, "running": instance.is_some(), "version": version, "confirm": confirm })
        }
        Some("open") => {
            log::info!(target: "host", "message type=open");
            let _ = Command::new("open").args(["-b", crate::BUNDLE_ID]).status();
            json!({ "ok": true })
        }
        Some("add") => {
            let Some(items) = msg.get("items").filter(|i| i.is_array()) else {
                return json!({ "ok": false, "error": "items missing" });
            };
            log::info!(
                target: "host",
                "message type=add items={}",
                items.as_array().map_or(0, Vec::len)
            );
            let (port, token) = match ensure_running(dir) {
                Ok(r) => r,
                Err(e) => {
                    log::warn!(target: "host", "add failed: {e}");
                    return json!({ "ok": false, "error": e });
                }
            };
            match http_post(
                port,
                &token,
                "/api/add_request",
                &json!({ "items": items }).to_string(),
            ) {
                Ok(v) => {
                    let added = v.get("added").and_then(Value::as_u64).unwrap_or(0);
                    // Waiting for the user's answer in the app's "Download File Info" dialog.
                    let queued = v.get("queued").and_then(Value::as_u64).unwrap_or(0);
                    let confirm = v.get("confirm").and_then(Value::as_bool).unwrap_or(false);
                    let errors = v.get("errors").cloned().unwrap_or(json!([]));
                    let failed = errors.as_array().is_some_and(|a| !a.is_empty());
                    log::info!(
                        target: "host",
                        "add result added={added} queued={queued} errors={}",
                        errors.as_array().map_or(0, Vec::len)
                    );
                    if added + queued == 0 && failed {
                        let first = errors[0].as_str().unwrap_or("could not add");
                        json!({ "ok": false, "added": 0, "error": first, "errors": errors })
                    } else {
                        json!({
                            "ok": true, "added": added + queued, "queued": queued,
                            "confirm": confirm, "errors": errors,
                        })
                    }
                }
                Err(e) => {
                    log::warn!(target: "host", "add failed: {e}");
                    json!({ "ok": false, "error": e })
                }
            }
        }
        _ => json!({ "ok": false, "error": "unknown message type" }),
    }
}

/// Entry point for `--native-host`. Returns the process exit code.
pub fn run(origin: Option<&str>) -> i32 {
    if let Some(o) = origin
        && o.starts_with("chrome-extension://")
        && !o.contains(EXTENSION_ID)
    {
        eprintln!("sushload native host: unexpected origin");
        return 1;
    }
    let stdin = io::stdin();
    let reply = match read_message(&mut stdin.lock()) {
        Ok(Some(msg)) => handle_message(&crate::data_dir(), &msg),
        Ok(None) => return 0,
        Err(e) => json!({ "ok": false, "error": e }),
    };
    match write_message(&mut io::stdout().lock(), &reply) {
        Ok(()) => 0,
        Err(_) => 1,
    }
}

// ---------- host manifests ----------

pub fn manifest_json(exe: &Path) -> String {
    let v = json!({
        "name": HOST_NAME,
        "description": "sushLoad download manager",
        "path": exe.to_string_lossy(),
        "type": "stdio",
        "allowed_origins": [format!("chrome-extension://{EXTENSION_ID}/")],
    });
    serde_json::to_string_pretty(&v).unwrap_or_default() + "\n"
}

/// Writes `com.soroushdeimi.sushload.json` for every installed Chromium browser when it is
/// missing or points elsewhere. `support` is `~/Library/Application Support`. Returns how many
/// files were written.
pub fn install_manifests(support: &Path, exe: &Path) -> usize {
    let want = manifest_json(exe);
    let mut written = 0;
    for b in BROWSER_DIRS {
        let browser = support.join(b);
        if !browser.is_dir() {
            continue;
        }
        let dir = browser.join("NativeMessagingHosts");
        let file = dir.join(format!("{HOST_NAME}.json"));
        if std::fs::read_to_string(&file).is_ok_and(|s| s == want) {
            continue;
        }
        if std::fs::create_dir_all(&dir).is_ok() && std::fs::write(&file, &want).is_ok() {
            written += 1;
        }
    }
    written
}

pub fn support_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Library/Application Support"))
}

/// Called once at app start; cheap (a few stat calls).
pub fn ensure_manifests() {
    if let (Some(support), Ok(exe)) = (support_dir(), std::env::current_exe()) {
        install_manifests(&support, &exe);
    }
}

// ---------- extension folder ----------

/// Where the unpacked extension lives for the binary at `exe`: inside the app bundle
/// (`Contents/Resources/extension`), else the repo's `extension/` (dev builds in `target/`).
pub fn extension_dir(exe: &Path) -> Option<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(macos) = exe.parent() {
        candidates.push(macos.join("../Resources/extension"));
    }
    for a in exe.ancestors().skip(1).take(4) {
        candidates.push(a.join("extension"));
    }
    candidates.push(PathBuf::from(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../extension"
    )));
    candidates
        .into_iter()
        .find(|c| c.join("manifest.json").is_file())
        .and_then(|c| c.canonicalize().ok())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn frame(v: &Value) -> Vec<u8> {
        let mut out = Vec::new();
        write_message(&mut out, v).unwrap();
        out
    }

    #[test]
    fn framing_round_trip() {
        let v =
            json!({"type": "add", "items": [{"url": "https://a.test/x.zip", "cookies": "a=b"}]});
        let bytes = frame(&v);
        assert_eq!(
            u32::from_ne_bytes(bytes[..4].try_into().unwrap()) as usize,
            bytes.len() - 4
        );
        let got = read_message(&mut Cursor::new(bytes)).unwrap().unwrap();
        assert_eq!(got, v);
    }

    #[test]
    fn eof_is_none_and_truncation_errors() {
        assert!(
            read_message(&mut Cursor::new(Vec::new()))
                .unwrap()
                .is_none()
        );
        assert!(read_message(&mut Cursor::new(vec![1, 0])).is_err());
        let mut b = 10u32.to_ne_bytes().to_vec();
        b.extend_from_slice(b"{}");
        assert!(read_message(&mut Cursor::new(b)).is_err());
    }

    #[test]
    fn oversize_rejected_without_reading_body() {
        let b = ((MAX_MESSAGE + 1) as u32).to_ne_bytes().to_vec();
        let e = read_message(&mut Cursor::new(b)).unwrap_err();
        assert!(e.contains("too large"), "{e}");
        // Exactly at the limit is accepted as far as the length goes (body is then missing).
        let b = (MAX_MESSAGE as u32).to_ne_bytes().to_vec();
        let e = read_message(&mut Cursor::new(b)).unwrap_err();
        assert!(e.contains("truncated"), "{e}");
    }

    #[test]
    fn invalid_json_rejected() {
        let mut b = 3u32.to_ne_bytes().to_vec();
        b.extend_from_slice(b"nop");
        assert!(read_message(&mut Cursor::new(b)).is_err());
    }

    #[test]
    fn http_response_parsing() {
        let ok = b"HTTP/1.1 200 OK\r\nContent-Length: 20\r\n\r\n{\"added\":1,\"errors\":[]}";
        assert_eq!(parse_http_response(ok).unwrap()["added"], 1);
        let bad = b"HTTP/1.1 401 Unauthorized\r\n\r\n{\"error\":\"missing or wrong token\"}";
        assert_eq!(
            parse_http_response(bad).unwrap_err(),
            "missing or wrong token"
        );
        assert!(parse_http_response(b"garbage").is_err());
    }

    #[test]
    fn unknown_and_malformed_messages() {
        let dir = std::env::temp_dir().join("sushload-nh-test-none");
        let r = handle_message(&dir, &json!({"type": "nope"}));
        assert_eq!(r["ok"], false);
        let r = handle_message(&dir, &json!({"type": "add"}));
        assert_eq!(r["error"], "items missing");
        let r = handle_message(&dir, &json!({"type": "ping"}));
        assert_eq!(
            (r["ok"].clone(), r["running"].clone()),
            (json!(true), json!(false))
        );
    }

    #[test]
    fn manifests_written_only_for_installed_browsers() {
        let tmp = std::env::temp_dir().join(format!("sushload-nh-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(tmp.join("Google/Chrome")).unwrap();
        std::fs::create_dir_all(tmp.join("BraveSoftware/Brave-Browser")).unwrap();
        let exe = Path::new("/Applications/sushLoad.app/Contents/MacOS/sushload");
        assert_eq!(install_manifests(&tmp, exe), 2);
        assert!(!tmp.join("Microsoft Edge").exists());
        let f = tmp.join("Google/Chrome/NativeMessagingHosts/com.soroushdeimi.sushload.json");
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&f).unwrap()).unwrap();
        assert_eq!(v["name"], HOST_NAME);
        assert_eq!(v["path"], exe.to_str().unwrap());
        assert_eq!(v["type"], "stdio");
        assert_eq!(
            v["allowed_origins"][0],
            format!("chrome-extension://{EXTENSION_ID}/")
        );
        // Unchanged -> nothing rewritten; moved binary -> rewritten.
        assert_eq!(install_manifests(&tmp, exe), 0);
        assert_eq!(install_manifests(&tmp, Path::new("/opt/x/sushload")), 2);
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn extension_dir_found_in_dev_tree() {
        let exe = Path::new(env!("CARGO_MANIFEST_DIR")).join("../target/debug/sushload");
        let d = extension_dir(&exe).expect("extension/ exists in the repo");
        assert!(d.join("manifest.json").is_file());
    }
}
