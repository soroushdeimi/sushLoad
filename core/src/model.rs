//! Shared types between the engine (`engine`) and the UI (`sushload` app crate).

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Stable id of a download, unique for the lifetime of the state file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct DownloadId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Status {
    /// Waiting for a free slot (`Settings::max_concurrent`).
    Queued,
    /// Sending the probe request / opening connections.
    Connecting,
    Downloading,
    Paused,
    Completed,
    /// Gave up; `DownloadView::error` says why. `resume` retries it.
    Failed,
}

impl Status {
    /// Downloads in these states make `Engine::has_active` return true.
    pub fn is_active(self) -> bool {
        matches!(
            self,
            Status::Queued | Status::Connecting | Status::Downloading
        )
    }

    pub fn label(self) -> &'static str {
        match self {
            Status::Queued => "Queued",
            Status::Connecting => "Connecting",
            Status::Downloading => "Downloading",
            Status::Paused => "Paused",
            Status::Completed => "Completed",
            Status::Failed => "Failed",
        }
    }
}

/// File category, decided from the file extension (see `categories`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Category {
    Video,
    Music,
    Documents,
    Archives,
    Programs,
    Images,
    Other,
}

impl Category {
    pub const ALL: [Category; 7] = [
        Category::Video,
        Category::Music,
        Category::Documents,
        Category::Archives,
        Category::Programs,
        Category::Images,
        Category::Other,
    ];

    /// Also the name of the category's sub-folder under `Settings::download_dir`.
    pub fn label(self) -> &'static str {
        match self {
            Category::Video => "Video",
            Category::Music => "Music",
            Category::Documents => "Documents",
            Category::Archives => "Archives",
            Category::Programs => "Programs",
            Category::Images => "Images",
            Category::Other => "Other",
        }
    }
}

/// One byte range of a download. `end` is inclusive; bytes `start..start+received` are on disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Segment {
    pub start: u64,
    pub end: u64,
    pub received: u64,
}

#[cfg(test)]
mod tests {
    use super::validate_proxy_url as v;

    #[test]
    fn proxy_urls() {
        assert!(v("socks5://127.0.0.1:1080").is_ok());
        assert!(v("socks5h://user:p@ss@proxy.example:1080").is_ok());
        assert!(v("http://10.0.0.1:8080/").is_ok());
        assert!(v("HTTPS://h:443").is_ok());
        assert!(v("ftp://h:21").is_err());
        assert!(v("127.0.0.1:1080").is_err());
        assert!(v("socks5://host").is_err());
        assert!(v("socks5://:1080").is_err());
        assert!(v("http://h:0").is_err());
        assert!(v("http://h:99999").is_err());
    }
}

impl Segment {
    pub fn len(&self) -> u64 {
        self.end - self.start + 1
    }
    pub fn is_empty(&self) -> bool {
        false
    }
    pub fn remaining(&self) -> u64 {
        self.len().saturating_sub(self.received)
    }
    pub fn is_done(&self) -> bool {
        self.received >= self.len()
    }
}

/// Which proxy downloads go through.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ProxyMode {
    /// macOS system proxy settings (System Settings › Network).
    #[default]
    System,
    /// Direct connection, ignoring the system proxy.
    Off,
    /// `Settings::proxy_url`.
    Custom,
}

/// Schemes accepted in `Settings::proxy_url`.
pub const PROXY_SCHEMES: [&str; 4] = ["http", "https", "socks5", "socks5h"];

/// Checks a custom proxy URL: `scheme://[user:pass@]host:port` with a scheme from
/// `PROXY_SCHEMES` (`socks5h` also resolves DNS through the proxy). Returns a message on error.
pub fn validate_proxy_url(url: &str) -> Result<(), String> {
    let url = url.trim();
    let (scheme, rest) = url
        .split_once("://")
        .ok_or("Proxy must look like socks5://host:port or http://host:port")?;
    if !PROXY_SCHEMES.contains(&scheme.to_ascii_lowercase().as_str()) {
        return Err(format!(
            "Unsupported proxy type \"{scheme}\" (use http, https, socks5 or socks5h)"
        ));
    }
    let host_port = rest
        .rsplit_once('@')
        .map_or(rest, |(_, hp)| hp)
        .trim_end_matches('/');
    let (host, port) = host_port
        .rsplit_once(':')
        .ok_or("Proxy needs a port, e.g. 127.0.0.1:1080")?;
    if host.is_empty() || host.contains('/') {
        return Err("Proxy host is missing".into());
    }
    match port.parse::<u16>() {
        Ok(p) if p > 0 => Ok(()),
        _ => Err(format!("Bad proxy port \"{port}\"")),
    }
}

/// User settings, persisted with the state. Changes apply live via `Engine::set_settings`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Base folder; files go to `download_dir/<Category::label()>/` when `sort_into_categories`.
    pub download_dir: PathBuf,
    pub sort_into_categories: bool,
    /// Downloads running at once (1..=10).
    pub max_concurrent: u32,
    /// Connections per download (1..=16).
    pub connections: u32,
    /// Global limit over all downloads in bytes/second; 0 = unlimited.
    pub speed_limit: u64,
    pub proxy_mode: ProxyMode,
    /// Used when `proxy_mode == Custom`; must pass `validate_proxy_url`.
    pub proxy_url: String,
}

impl Default for Settings {
    fn default() -> Self {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_default();
        Settings {
            download_dir: home.join("Downloads").join("sushLoad"),
            sort_into_categories: true,
            max_concurrent: 3,
            connections: 8,
            speed_limit: 0,
            proxy_mode: ProxyMode::System,
            proxy_url: String::new(),
        }
    }
}

/// Read-only snapshot of one download for the UI (`Engine::snapshot`).
#[derive(Debug, Clone, PartialEq)]
pub struct DownloadView {
    pub id: DownloadId,
    pub url: String,
    pub file_name: String,
    /// Final path once completed; the planned destination before that.
    pub path: PathBuf,
    pub category: Category,
    pub status: Status,
    /// None while unknown (before the probe, or the server sent no length).
    pub total: Option<u64>,
    pub received: u64,
    /// Smoothed bytes/second; 0 when not downloading.
    pub speed: u64,
    /// Seconds left; None when unknown or not downloading.
    pub eta_secs: Option<u64>,
    /// Open connections right now.
    pub connections: u32,
    /// Segment layout for a progress bar (empty for single-connection downloads).
    pub segments: Vec<Segment>,
    pub resumable: bool,
    pub error: Option<String>,
    /// Unix seconds.
    pub added_at: u64,
}
