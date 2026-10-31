//! A tiny file logger behind the `log` facade: `~/Library/Logs/sushLoad/sushload.log`, one line
//! per record (`2026-10-31T08:40:12Z INFO  engine: …`), rotated at 1 MiB into `.1`..`.3`.
//! Level comes from `SUSHLOAD_LOG` (error|warn|info|debug, default info). Each line is one
//! `write` on an `O_APPEND` file, so the app and the short-lived native host (separate
//! processes) can share it; only the app rotates.

use log::{LevelFilter, Log, Metadata, Record};
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

pub const MAX_BYTES: u64 = 1024 * 1024;
pub const KEEP: u32 = 3;

pub fn log_dir() -> PathBuf {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    home.join("Library").join("Logs").join("sushLoad")
}

pub fn parse_level(text: Option<&str>) -> LevelFilter {
    match text.map(|t| t.trim().to_ascii_lowercase()).as_deref() {
        Some("error") => LevelFilter::Error,
        Some("warn" | "warning") => LevelFilter::Warn,
        Some("debug") => LevelFilter::Debug,
        _ => LevelFilter::Info,
    }
}

/// `2026-10-31T08:40:12Z` for Unix seconds (UTC, proleptic Gregorian).
pub fn format_utc(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    // Days since 1970-01-01 -> civil date (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

struct Sink {
    file: Option<File>,
    size: u64,
}

pub struct FileLogger {
    path: PathBuf,
    level: LevelFilter,
    max_bytes: u64,
    keep: u32,
    rotate: bool,
    sink: Mutex<Sink>,
}

impl FileLogger {
    pub fn new(path: PathBuf, level: LevelFilter, max_bytes: u64, keep: u32, rotate: bool) -> Self {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let file = open_append(&path);
        let size = file
            .as_ref()
            .and_then(|f| f.metadata().ok())
            .map_or(0, |m| m.len());
        FileLogger {
            path,
            level,
            max_bytes,
            keep,
            rotate,
            sink: Mutex::new(Sink { file, size }),
        }
    }

    /// Appends one line (a trailing newline is added), rotating first when it would not fit.
    pub fn write_line(&self, line: &str) {
        let mut sink = self.sink.lock().unwrap_or_else(|e| e.into_inner());
        let bytes = line.len() as u64 + 1;
        if self.rotate && sink.size > 0 && sink.size + bytes > self.max_bytes {
            sink.file = None;
            rotate_files(&self.path, self.keep);
            sink.file = open_append(&self.path);
            sink.size = 0;
        }
        if let Some(file) = sink.file.as_mut() {
            let mut buf = String::with_capacity(line.len() + 1);
            buf.push_str(line);
            buf.push('\n');
            if file.write_all(buf.as_bytes()).is_ok() {
                sink.size += bytes;
            }
        }
    }
}

fn open_append(path: &Path) -> Option<File> {
    use std::os::unix::fs::OpenOptionsExt;
    OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(path)
        .ok()
}

fn numbered(path: &Path, n: u32) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(format!(".{n}"));
    PathBuf::from(name)
}

/// `log` -> `log.1` -> `log.2` -> `log.3`; the oldest is dropped.
fn rotate_files(path: &Path, keep: u32) {
    let _ = std::fs::remove_file(numbered(path, keep));
    for n in (1..keep).rev() {
        let _ = std::fs::rename(numbered(path, n), numbered(path, n + 1));
    }
    let _ = std::fs::rename(path, numbered(path, 1));
}

pub fn format_line(secs: u64, level: log::Level, target: &str, message: &str) -> String {
    // One record per line: fold embedded newlines.
    let message = message.replace(['\n', '\r'], " ");
    format!("{} {:<5} {target}: {message}", format_utc(secs), level)
}

impl Log for FileLogger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        // Our own targets follow SUSHLOAD_LOG; libraries (hyper, reqwest, tiny_http…) only
        // get a line when something is wrong.
        let own = matches!(
            metadata.target(),
            "engine" | "server" | "app" | "host" | "notify"
        );
        let limit = if own {
            self.level
        } else {
            self.level.min(LevelFilter::Warn)
        };
        metadata.level() <= limit
    }

    fn log(&self, record: &Record) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        self.write_line(&format_line(
            secs,
            record.level(),
            record.target(),
            &record.args().to_string(),
        ));
    }

    fn flush(&self) {}
}

/// Installs the logger. `rotate` is true only in the app process. Safe to call once.
pub fn init(rotate: bool) {
    let level = parse_level(std::env::var("SUSHLOAD_LOG").ok().as_deref());
    let logger = FileLogger::new(
        log_dir().join("sushload.log"),
        level,
        MAX_BYTES,
        KEEP,
        rotate,
    );
    let logger: &'static FileLogger = Box::leak(Box::new(logger));
    if log::set_logger(logger).is_ok() {
        log::set_max_level(level);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utc_timestamps() {
        assert_eq!(format_utc(0), "1970-01-01T00:00:00Z");
        assert_eq!(format_utc(1_000_000_000), "2001-09-09T01:46:40Z");
        assert_eq!(format_utc(1_782_895_212), "2026-07-01T08:40:12Z");
        assert_eq!(format_utc(1_709_164_800), "2024-02-29T00:00:00Z");
    }

    #[test]
    fn line_format_and_levels() {
        let line = format_line(1_000_000_000, log::Level::Info, "engine", "hello\nworld");
        assert_eq!(line, "2001-09-09T01:46:40Z INFO  engine: hello world");
        assert!(format_line(0, log::Level::Warn, "x", "m").contains(" WARN  x: m"));
        assert!(format_line(0, log::Level::Error, "x", "m").contains(" ERROR x: m"));
        assert_eq!(parse_level(None), LevelFilter::Info);
        assert_eq!(parse_level(Some("DEBUG")), LevelFilter::Debug);
        assert_eq!(parse_level(Some("error")), LevelFilter::Error);
        assert_eq!(parse_level(Some("nonsense")), LevelFilter::Info);
    }

    #[test]
    fn library_chatter_is_limited_to_warnings() {
        let dir = tempfile::tempdir().unwrap();
        let logger = FileLogger::new(dir.path().join("l.log"), LevelFilter::Debug, 1000, 3, true);
        let meta = |target, level| Metadata::builder().target(target).level(level).build();
        assert!(logger.enabled(&meta("engine", log::Level::Debug)));
        assert!(!logger.enabled(&meta("reqwest::connect", log::Level::Debug)));
        assert!(!logger.enabled(&meta("tiny_http", log::Level::Info)));
        assert!(logger.enabled(&meta("hyper", log::Level::Warn)));
    }

    #[test]
    fn rotates_at_the_limit_and_keeps_three() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sushload.log");
        let logger = FileLogger::new(path.clone(), LevelFilter::Info, 200, 3, true);
        for i in 0..60 {
            logger.write_line(&format!("line {i:03} {}", "x".repeat(30)));
        }
        for n in 1..=3 {
            assert!(numbered(&path, n).exists(), "missing .{n}");
        }
        assert!(!numbered(&path, 4).exists(), "only three are kept");
        for p in [path.clone(), numbered(&path, 1), numbered(&path, 3)] {
            assert!(std::fs::metadata(&p).unwrap().len() <= 200, "{p:?} too big");
        }
        // The newest line is in the live file, older ones moved on.
        let live = std::fs::read_to_string(&path).unwrap();
        assert!(live.contains("line 059"));
        let one = std::fs::read_to_string(numbered(&path, 1)).unwrap();
        assert!(!one.contains("line 059") && one.contains("line 05"));
    }

    #[test]
    fn host_mode_never_rotates_and_appends() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sushload.log");
        let a = FileLogger::new(path.clone(), LevelFilter::Info, 50, 3, false);
        let b = FileLogger::new(path.clone(), LevelFilter::Info, 50, 3, false);
        for i in 0..10 {
            a.write_line(&format!("a {i} {}", "y".repeat(20)));
            b.write_line(&format!("b {i} {}", "y".repeat(20)));
        }
        assert!(!numbered(&path, 1).exists());
        assert_eq!(std::fs::read_to_string(&path).unwrap().lines().count(), 20);
    }
}
