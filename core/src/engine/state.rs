//! Persisted download records and the atomic `state.json` reader/writer.

use crate::model::{Category, DownloadId, Segment, Settings, Status};
use serde::{Deserialize, Serialize};
use std::io::Write;
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
        let mut file = std::fs::File::create(&tmp)
            .map_err(|e| format!("cannot create {}: {e}", tmp.display()))?;
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
