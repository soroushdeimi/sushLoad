//! Persisted download records and the atomic `state.json` reader/writer.

use crate::model::{Category, DownloadId, Segment, Settings, Status};
use crate::sites::SiteEntry;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

/// One download as stored in `state.json` (no runtime-only fields).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadRecord {
    pub id: DownloadId,
    pub url: String,
    pub file_name: String,
    pub path: PathBuf,
    pub category: Category,
    pub status: Status,
    pub total: Option<u64>,
    pub received: u64,
    pub segments: Vec<Segment>,
    pub resumable: bool,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub error: Option<String>,
    pub added_at: u64,
    /// The browser gave the file name; Content-Disposition must not change it.
    #[serde(default)]
    pub name_fixed: bool,
    /// Browser context from the extension. Never shown in the UI or in errors.
    #[serde(default)]
    pub referer: Option<String>,
    #[serde(default)]
    pub cookies: Option<String>,
    #[serde(default)]
    pub user_agent: Option<String>,
    /// Folder chosen in the confirmation dialog (wins over the category folder).
    #[serde(default)]
    pub dir: Option<PathBuf>,
    /// Replace an existing file instead of picking a free `name (1)`.
    #[serde(default)]
    pub overwrite: bool,
    /// Category chosen in the confirmation dialog.
    #[serde(default)]
    pub category_override: Option<Category>,
}

/// The on-disk root object.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StateFile {
    #[serde(default)]
    pub next_id: u64,
    #[serde(default)]
    pub settings: Settings,
    #[serde(default)]
    pub downloads: Vec<DownloadRecord>,
    /// Sites seen so far (site folders): the user's choices and cached AI answers.
    #[serde(default)]
    pub sites: HashMap<String, SiteEntry>,
}

fn state_path(data_dir: &Path) -> PathBuf {
    data_dir.join("state.json")
}

/// Load `data_dir/state.json`, or a default state when the file does not exist yet.
pub fn load(data_dir: &Path) -> Result<StateFile, String> {
    let path = state_path(data_dir);
    match std::fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map_err(|e| format!("cannot parse {}: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(StateFile::default()),
        Err(e) => Err(format!("cannot read {}: {e}", path.display())),
    }
}

/// Write `state.json.tmp` and rename it over `state.json` so readers never see a partial file.
pub fn save_bytes(data_dir: &Path, json: &[u8]) -> Result<(), String> {
    std::fs::create_dir_all(data_dir)
        .map_err(|e| format!("cannot create {}: {e}", data_dir.display()))?;
    let tmp = data_dir.join("state.json.tmp");
    {
        // The file may hold cookies: owner-only, also if a stale tmp file existed.
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&tmp)
            .map_err(|e| format!("cannot create {}: {e}", tmp.display()))?;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))
            .map_err(|e| format!("cannot protect {}: {e}", tmp.display()))?;
        file.write_all(json)
            .map_err(|e| format!("cannot write {}: {e}", tmp.display()))?;
        file.sync_all()
            .map_err(|e| format!("cannot flush {}: {e}", tmp.display()))?;
    }
    std::fs::rename(&tmp, state_path(data_dir))
        .map_err(|e| format!("cannot replace {}: {e}", state_path(data_dir).display()))
}

/// Serialize and atomically write a state file (used during startup).
pub fn save(data_dir: &Path, state: &StateFile) -> Result<(), String> {
    let json = serde_json::to_vec(state).map_err(|e| format!("cannot serialize state: {e}"))?;
    save_bytes(data_dir, &json)
}
