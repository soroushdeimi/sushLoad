//! Optional on-device grouping of unknown sites through the user's `tgsort-ai` helper
//! (Apple's Foundation Models; see AI_PROTOCOL.md). The helper is spawned only when a new,
//! unknown domain needs an answer and exits after each call, so an idle app costs nothing.
//! Nothing leaves the device.

use crate::sites::SiteGroup;
use serde_json::{Value, json};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

/// Longest a `classify` call may take.
pub const CLASSIFY_TIMEOUT: Duration = Duration::from_secs(15);
const CHECK_TIMEOUT: Duration = Duration::from_secs(5);

/// What we know about the page a download came from; written to a tiny text file the helper
/// reads (it looks at the first ~1500 characters).
#[derive(Debug, Clone, Default)]
pub struct SiteQuery {
    pub domain: String,
    pub page_title: Option<String>,
    /// URL path without query or fragment.
    pub url_path: String,
    pub file_name: String,
}

impl SiteQuery {
    pub fn describe(&self) -> String {
        let mut text = format!("Website: {}\n", self.domain);
        if let Some(title) = self.page_title.as_deref().filter(|t| !t.trim().is_empty()) {
            text.push_str(&format!("Page title: {}\n", title.trim()));
        }
        if !self.url_path.is_empty() {
            text.push_str(&format!("Download path: {}\n", self.url_path));
        }
        if !self.file_name.is_empty() {
            text.push_str(&format!("File name: {}\n", self.file_name));
        }
        text.push_str("What kind of website is this?\n");
        text
    }
}

/// The helper binary: `$SUSHLOAD_AI_HELPER`, `~/.local/bin/tgsort-ai`, next to this app's own
/// binary, or on `PATH`.
pub fn locate_helper() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("SUSHLOAD_AI_HELPER").map(PathBuf::from)
        && path.is_file()
    {
        return Some(path);
    }
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(home) = std::env::var_os("HOME") {
        candidates.push(PathBuf::from(home).join(".local/bin/tgsort-ai"));
    }
    if let Some(dir) = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf))
    {
        candidates.push(dir.join("tgsort-ai"));
    }
    if let Some(paths) = std::env::var_os("PATH") {
        candidates.extend(std::env::split_paths(&paths).map(|p| p.join("tgsort-ai")));
    }
    candidates.into_iter().find(|p| p.is_file())
}

/// Runs the helper and returns its stdout. Kills it when it takes longer than `timeout`.
fn run(
    helper: &Path,
    args: &[&str],
    stdin: Option<&[u8]>,
    timeout: Duration,
) -> Result<Vec<u8>, String> {
    let mut child = Command::new(helper)
        .args(args)
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("cannot start {}: {e}", helper.display()))?;
    if let Some(data) = stdin
        && let Some(mut pipe) = child.stdin.take()
    {
        // Requests are tiny; a write error just means the helper already exited.
        let _ = pipe.write_all(data);
    }
    let mut stdout = child.stdout.take().ok_or("no stdout")?;
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stdout.read_to_end(&mut buf);
        let _ = tx.send(buf);
    });
    match rx.recv_timeout(timeout) {
        Ok(out) => {
            let status = child.wait().map_err(|e| e.to_string())?;
            if status.success() {
                Ok(out)
            } else {
                Err(format!("helper exited with {status}"))
            }
        }
        Err(_) => {
            let _ = child.kill();
            let _ = child.wait();
            Err("the helper did not answer in time".to_string())
        }
    }
}

/// `tgsort-ai --check`: can the helper classify right now?
pub fn is_available(helper: &Path) -> bool {
    run(helper, &["--check"], None, CHECK_TIMEOUT)
        .ok()
        .and_then(|out| serde_json::from_slice::<Value>(&out).ok())
        .and_then(|v| v.get("available").and_then(Value::as_bool))
        .unwrap_or(false)
}

/// Asks the helper which group the site belongs to. `Ok(None)` = the model was unsure.
/// `Err` = the helper could not be run (not cached by the caller, so it may be retried later).
pub fn classify(helper: &Path, query: &SiteQuery) -> Result<Option<SiteGroup>, String> {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let file = std::env::temp_dir().join(format!("sushload-site-{}-{n}.txt", std::process::id()));
    std::fs::write(&file, query.describe()).map_err(|e| format!("cannot write temp file: {e}"))?;
    let result = classify_file(helper, &file);
    let _ = std::fs::remove_file(&file);
    result
}

fn classify_file(helper: &Path, file: &Path) -> Result<Option<SiteGroup>, String> {
    let choices: Vec<Value> = SiteGroup::ALL
        .iter()
        .filter(|g| **g != SiteGroup::Other)
        .map(|g| json!({"name": g.label(), "hint": g.hint()}))
        .collect();
    let request = json!({
        "files": [{"id": "0", "path": file.to_string_lossy(), "name": "site.txt"}],
        "choices": choices,
    });
    let out = run(
        helper,
        &["classify"],
        Some(request.to_string().as_bytes()),
        CLASSIFY_TIMEOUT,
    )?;
    let v: Value = serde_json::from_slice(&out).map_err(|e| format!("bad helper answer: {e}"))?;
    let choice = v["results"]
        .as_array()
        .and_then(|r| r.iter().find(|e| e["id"] == "0"))
        .and_then(|e| e["choice"].as_str());
    Ok(choice.and_then(SiteGroup::from_label))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    pub fn fake_helper(dir: &Path, body: &str) -> PathBuf {
        let path = dir.join("tgsort-ai");
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    #[test]
    fn describes_the_site() {
        let q = SiteQuery {
            domain: "faradars.org".into(),
            page_title: Some(" Python course ".into()),
            url_path: "/dl/a.zip".into(),
            file_name: "a.zip".into(),
        };
        let d = q.describe();
        assert!(d.contains("Website: faradars.org"));
        assert!(d.contains("Page title: Python course"));
        assert!(d.contains("File name: a.zip"));
    }

    #[test]
    fn check_and_classify_with_a_fake_helper() {
        let dir = tempfile::tempdir().unwrap();
        let ok = fake_helper(
            dir.path(),
            r#"if [ "$1" = "--check" ]; then echo '{"available":true}'; else cat >/dev/null; echo '{"results":[{"id":"0","choice":"University"}]}'; fi"#,
        );
        assert!(is_available(&ok));
        let q = SiteQuery {
            domain: "x.org".into(),
            ..SiteQuery::default()
        };
        assert_eq!(classify(&ok, &q), Ok(Some(SiteGroup::University)));
    }

    #[test]
    fn unsure_unknown_and_broken_helpers() {
        let dir = tempfile::tempdir().unwrap();
        let q = SiteQuery::default();
        let unsure = fake_helper(
            dir.path(),
            r#"cat >/dev/null; echo '{"results":[{"id":"0","choice":null}]}'"#,
        );
        assert_eq!(classify(&unsure, &q), Ok(None));
        let weird = fake_helper(
            dir.path(),
            r#"cat >/dev/null; echo '{"results":[{"id":"0","choice":"Gardening"}]}'"#,
        );
        assert_eq!(classify(&weird, &q), Ok(None));
        let unavailable = fake_helper(dir.path(), r#"echo '{"available":false}'"#);
        assert!(!is_available(&unavailable));
        let broken = fake_helper(dir.path(), "exit 3");
        assert!(classify(&broken, &q).is_err());
        assert!(!is_available(&broken));
        assert!(classify(&dir.path().join("missing"), &q).is_err());
    }

    #[test]
    fn a_slow_helper_is_killed_at_the_timeout() {
        let dir = tempfile::tempdir().unwrap();
        let slow = fake_helper(dir.path(), "sleep 20");
        let started = std::time::Instant::now();
        let r = run(&slow, &["--check"], None, Duration::from_millis(300));
        assert!(r.is_err());
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    /// Manual check against the real helper:
    /// `cargo test -p sushload-core real_helper -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn real_helper_smoke() {
        let Some(helper) = locate_helper() else {
            println!("no tgsort-ai helper found");
            return;
        };
        let t = std::time::Instant::now();
        let available = is_available(&helper);
        println!(
            "check: available={available} in {} ms",
            t.elapsed().as_millis()
        );
        if !available {
            return;
        }
        for (domain, title, path, name) in [
            (
                "ostadbank.com",
                "Ostad Bank - sample exam questions",
                "/files/ostad/exam-1402.pdf",
                "exam-1402.pdf",
            ),
            (
                "faradars.org",
                "Faradars - online courses and training videos",
                "/dl/python-course-part1.zip",
                "python-course-part1.zip",
            ),
            (
                "coursera.org",
                "Machine Learning | Coursera",
                "/learn/machine-learning/lecture-notes.pdf",
                "lecture-notes.pdf",
            ),
        ] {
            let q = SiteQuery {
                domain: domain.into(),
                page_title: Some(title.into()),
                url_path: path.into(),
                file_name: name.into(),
            };
            let t = std::time::Instant::now();
            let r = classify(&helper, &q);
            println!("{domain}: {r:?} in {} ms", t.elapsed().as_millis());
        }
    }
}
