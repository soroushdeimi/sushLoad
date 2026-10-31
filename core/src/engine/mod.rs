//! The download engine. Owns the tokio runtime, the scheduler, live progress and persistence.
//!
//! Every public method is non-blocking and safe to call from the macOS main thread. The scheduler
//! is event driven: it awaits a command channel and sleeps when there is nothing to do, so an idle
//! engine performs no periodic work at all.

mod client;
mod download;
mod ratelimit;
mod segments;
mod state;
mod util;

use crate::categories;
use crate::model::{AddRequest, DownloadId, DownloadView, Settings, Status};
use client::Net;
use ratelimit::RateLimiter;
use state::{DownloadRecord, StateFile};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::{mpsc, watch};

pub(crate) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

pub(crate) struct Entry {
    pub record: DownloadRecord,
    pub speed: u64,
    pub eta_secs: Option<u64>,
    pub connections: u32,
    pub cancel: Option<watch::Sender<bool>>,
}

pub(crate) struct InnerState {
    pub next_id: u64,
    pub settings: Settings,
    pub downloads: Vec<Entry>,
}

impl InnerState {
    pub fn find(&self, id: DownloadId) -> Option<&Entry> {
        self.downloads.iter().find(|entry| entry.record.id == id)
    }

    pub fn find_mut(&mut self, id: DownloadId) -> Option<&mut Entry> {
        self.downloads
            .iter_mut()
            .find(|entry| entry.record.id == id)
    }

    fn snapshot(&self) -> StateFile {
        StateFile {
            next_id: self.next_id,
            settings: self.settings.clone(),
            downloads: self
                .downloads
                .iter()
                .map(|entry| entry.record.clone())
                .collect(),
        }
    }
}

pub(crate) enum Command {
    Schedule,
    Finished(DownloadId),
    DeleteFiles {
        part: PathBuf,
        final_path: PathBuf,
        delete_final: bool,
    },
    Shutdown(std::sync::mpsc::Sender<()>),
}

struct SaveRequest {
    bytes: Vec<u8>,
    ack: Option<std::sync::mpsc::Sender<()>>,
}

pub(crate) struct Inner {
    pub state: Mutex<InnerState>,
    pub limiter: Arc<RateLimiter>,
    net: Mutex<NetSlot>,
    pub tx: mpsc::UnboundedSender<Command>,
    save_tx: std::sync::mpsc::Sender<SaveRequest>,
}

/// The client new downloads use, or why none can be built from the current proxy settings.
struct NetSlot {
    net: Net,
    error: Option<String>,
}

impl Inner {
    /// The client a download should use when it (re)starts.
    pub fn net(&self) -> Result<Net, String> {
        let slot = lock(&self.net);
        match &slot.error {
            Some(error) => Err(format!("Proxy: {error}")),
            None => Ok(slot.net.clone()),
        }
    }

    /// Rebuilds the client after a proxy setting changed. On failure the previous client stays
    /// and new downloads fail with the error until the settings are fixed.
    fn rebuild_client(&self, settings: &Settings) {
        let built = client::build_client(settings);
        let mut slot = lock(&self.net);
        match built {
            Ok(net) => {
                slot.net = net;
                slot.error = None;
            }
            Err(error) => slot.error = Some(error),
        }
    }

    /// Queue an atomic state save without blocking the caller.
    pub fn persist(&self) {
        let file = { lock(&self.state).snapshot() };
        let Ok(bytes) = serde_json::to_vec(&file) else {
            return;
        };
        let _ = self.save_tx.send(SaveRequest { bytes, ack: None });
    }

    /// Queue a save and wait until the state file is on disk (used only by shutdown).
    fn persist_wait(&self) {
        let file = { lock(&self.state).snapshot() };
        let Ok(bytes) = serde_json::to_vec(&file) else {
            return;
        };
        let (ack_tx, ack_rx) = std::sync::mpsc::channel();
        if self
            .save_tx
            .send(SaveRequest {
                bytes,
                ack: Some(ack_tx),
            })
            .is_ok()
        {
            let _ = ack_rx.recv_timeout(Duration::from_secs(2));
        }
    }
}

/// Handle to the download engine. Cheap to clone; all methods are non-blocking and callable
/// from the UI (main) thread. The engine runs its own tokio runtime on background threads.
#[derive(Clone)]
pub struct Engine {
    inner: Arc<Inner>,
}

impl Engine {
    /// Loads `data_dir/state.json` (creating `data_dir`), and starts the runtime. Downloads that
    /// were running when the app last quit come back as `Queued` and start again.
    pub fn start(data_dir: PathBuf) -> Result<Engine, String> {
        std::fs::create_dir_all(&data_dir)
            .map_err(|e| format!("cannot create {}: {e}", data_dir.display()))?;

        let mut file = state::load(&data_dir)?;
        for record in &mut file.downloads {
            if matches!(record.status, Status::Connecting | Status::Downloading) {
                record.status = Status::Queued;
            }
        }
        let highest = file
            .downloads
            .iter()
            .map(|record| record.id.0)
            .max()
            .unwrap_or(0);
        if file.next_id <= highest {
            file.next_id = highest + 1;
        }
        state::save(&data_dir, &file)?;

        let settings = file.settings.clone();
        let speed_limit = settings.speed_limit;
        let (save_tx, save_rx) = std::sync::mpsc::channel();
        spawn_saver(data_dir.clone(), save_rx);

        let (tx, rx) = mpsc::unbounded_channel();
        // A bad saved proxy must not stop the engine: fall back to a plain client and let new
        // downloads fail with the proxy error.
        let net = match client::build_client(&settings) {
            Ok(net) => NetSlot { net, error: None },
            Err(error) => NetSlot {
                net: Net::direct()?,
                error: Some(error),
            },
        };

        let inner = Arc::new(Inner {
            state: Mutex::new(InnerState {
                next_id: file.next_id,
                settings,
                downloads: file
                    .downloads
                    .into_iter()
                    .map(|record| Entry {
                        record,
                        speed: 0,
                        eta_secs: None,
                        connections: 0,
                        cancel: None,
                    })
                    .collect(),
            }),
            limiter: Arc::new(RateLimiter::new(speed_limit)),
            net: Mutex::new(net),
            tx,
            save_tx,
        });

        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<(), String>>();
        let thread_inner = Arc::clone(&inner);
        std::thread::Builder::new()
            .name("sushload-engine".to_string())
            .spawn(move || {
                let runtime = match tokio::runtime::Builder::new_multi_thread()
                    .worker_threads(2)
                    .max_blocking_threads(4)
                    .enable_all()
                    .build()
                {
                    Ok(runtime) => runtime,
                    Err(error) => {
                        let _ = ready_tx.send(Err(format!("cannot start runtime: {error}")));
                        return;
                    }
                };
                if ready_tx.send(Ok(())).is_err() {
                    return;
                }
                runtime.block_on(scheduler(thread_inner, rx));
            })
            .map_err(|e| format!("cannot start engine thread: {e}"))?;

        ready_rx
            .recv()
            .map_err(|_| "engine thread stopped during startup".to_string())??;

        Ok(Engine { inner })
    }

    /// Adds an http(s) URL and queues it. Errors on an invalid URL.
    pub fn add(&self, url: &str) -> Result<DownloadId, String> {
        self.add_request(AddRequest {
            url: url.to_string(),
            ..AddRequest::default()
        })
    }

    /// Like `add`, with the browser's file name, Referer, cookies and User-Agent. The file name
    /// wins over the URL and Content-Disposition; the headers go only to the URL's own origin
    /// (cookies never follow a redirect to another origin) and are kept in `state.json`
    /// (owner-only) but never exposed through `DownloadView` or errors.
    pub fn add_request(&self, request: AddRequest) -> Result<DownloadId, String> {
        let url = request.url.trim();
        let parsed = reqwest::Url::parse(url).map_err(|e| format!("invalid URL: {e}"))?;
        if !matches!(parsed.scheme(), "http" | "https") {
            return Err("only http and https URLs are supported".to_string());
        }
        if parsed.host_str().is_none_or(str::is_empty) {
            return Err("URL has no host".to_string());
        }
        let referer = clean_header(request.referer, "Referer")?;
        let cookies = clean_header(request.cookies, "cookies")?;
        let user_agent = clean_header(request.user_agent, "User-Agent")?;
        let fixed_name = request
            .file_name
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(util::sanitize_file_name);

        let name_fixed = fixed_name.is_some();
        let file_name = fixed_name.unwrap_or_else(|| util::file_name_from_url(&parsed));
        let category = categories::classify(&file_name);
        let settings = { lock(&self.inner.state).settings.clone() };
        let path = categories::destination_dir(&settings, &file_name).join(&file_name);

        let mut state = lock(&self.inner.state);
        let id = DownloadId(state.next_id);
        state.next_id += 1;
        state.downloads.push(Entry {
            record: DownloadRecord {
                id,
                url: url.to_string(),
                file_name,
                path,
                category,
                status: Status::Queued,
                total: None,
                received: 0,
                segments: Vec::new(),
                resumable: false,
                etag: None,
                last_modified: None,
                error: None,
                added_at: now_secs(),
                name_fixed,
                referer,
                cookies,
                user_agent,
            },
            speed: 0,
            eta_secs: None,
            connections: 0,
            cancel: None,
        });
        drop(state);

        self.inner.persist();
        let _ = self.inner.tx.send(Command::Schedule);
        Ok(id)
    }

    pub fn pause(&self, id: DownloadId) {
        let changed = {
            let mut state = lock(&self.inner.state);
            match state.find_mut(id) {
                Some(entry) if entry.record.status.is_active() => {
                    entry.record.status = Status::Paused;
                    entry.speed = 0;
                    entry.eta_secs = None;
                    entry.connections = 0;
                    if let Some(cancel) = &entry.cancel {
                        let _ = cancel.send(true);
                    }
                    true
                }
                _ => false,
            }
        };
        if changed {
            self.inner.persist();
        }
    }

    /// Re-queues a paused or failed download.
    pub fn resume(&self, id: DownloadId) {
        let changed = {
            let mut state = lock(&self.inner.state);
            match state.find_mut(id) {
                Some(entry) if matches!(entry.record.status, Status::Paused | Status::Failed) => {
                    entry.record.status = Status::Queued;
                    entry.record.error = None;
                    true
                }
                _ => false,
            }
        };
        if changed {
            self.inner.persist();
            let _ = self.inner.tx.send(Command::Schedule);
        }
    }

    /// Stops and forgets the download; with `delete_file` also removes the partial/finished file.
    pub fn remove(&self, id: DownloadId, delete_file: bool) {
        let paths = {
            let mut state = lock(&self.inner.state);
            let Some(position) = state
                .downloads
                .iter()
                .position(|entry| entry.record.id == id)
            else {
                return;
            };
            if let Some(cancel) = &state.downloads[position].cancel {
                let _ = cancel.send(true);
            }
            let record = &state.downloads[position].record;
            let paths = (download::part_path(&record.path), record.path.clone());
            state.downloads.remove(position);
            paths
        };
        self.inner.persist();
        let _ = self.inner.tx.send(Command::DeleteFiles {
            part: paths.0,
            final_path: paths.1,
            delete_final: delete_file,
        });
    }

    pub fn pause_all(&self) {
        let ids = {
            let state = lock(&self.inner.state);
            state
                .downloads
                .iter()
                .filter(|entry| entry.record.status.is_active())
                .map(|entry| entry.record.id)
                .collect::<Vec<_>>()
        };
        for id in ids {
            self.pause(id);
        }
    }

    pub fn resume_all(&self) {
        let ids = {
            let state = lock(&self.inner.state);
            state
                .downloads
                .iter()
                .filter(|entry| matches!(entry.record.status, Status::Paused | Status::Failed))
                .map(|entry| entry.record.id)
                .collect::<Vec<_>>()
        };
        for id in ids {
            self.resume(id);
        }
    }

    /// Newest first. Cheap (copies under a short lock); the UI calls it about twice a second
    /// while `has_active()` and never otherwise.
    pub fn snapshot(&self) -> Vec<DownloadView> {
        let state = lock(&self.inner.state);
        let mut views = state
            .downloads
            .iter()
            .map(|entry| DownloadView {
                id: entry.record.id,
                url: entry.record.url.clone(),
                file_name: entry.record.file_name.clone(),
                path: entry.record.path.clone(),
                category: entry.record.category,
                status: entry.record.status,
                total: entry.record.total,
                received: entry.record.received,
                speed: entry.speed,
                eta_secs: entry.eta_secs,
                connections: entry.connections,
                segments: entry.record.segments.clone(),
                resumable: entry.record.resumable,
                error: entry.record.error.clone(),
                added_at: entry.record.added_at,
            })
            .collect::<Vec<_>>();
        views.sort_by(|a, b| {
            b.added_at
                .cmp(&a.added_at)
                .then_with(|| b.id.0.cmp(&a.id.0))
        });
        views
    }

    pub fn has_active(&self) -> bool {
        lock(&self.inner.state)
            .downloads
            .iter()
            .any(|entry| entry.record.status.is_active())
    }

    pub fn settings(&self) -> Settings {
        lock(&self.inner.state).settings.clone()
    }

    pub fn set_settings(&self, settings: Settings) {
        let proxy_changed = {
            let mut state = lock(&self.inner.state);
            let changed = state.settings.proxy_mode != settings.proxy_mode
                || state.settings.proxy_url != settings.proxy_url;
            state.settings = settings.clone();
            changed
        };
        if proxy_changed {
            self.inner.rebuild_client(&settings);
        }
        self.inner.limiter.set_rate(settings.speed_limit);
        self.inner.persist();
        let _ = self.inner.tx.send(Command::Schedule);
    }

    /// Pauses running transfers, writes the state file and returns once it is on disk.
    pub fn shutdown(&self) {
        let (ack_tx, ack_rx) = std::sync::mpsc::channel();
        if self.inner.tx.send(Command::Shutdown(ack_tx)).is_ok() {
            let _ = ack_rx.recv_timeout(Duration::from_secs(2));
        }
    }
}

/// Trims a header value; empty means absent. Errors (without echoing the value) when it
/// cannot be sent as an HTTP header.
fn clean_header(value: Option<String>, what: &str) -> Result<Option<String>, String> {
    let Some(value) = value else {
        return Ok(None);
    };
    let value = value.trim();
    if value.is_empty() {
        return Ok(None);
    }
    reqwest::header::HeaderValue::from_str(value)
        .map_err(|_| format!("{what} contains characters that cannot be sent"))?;
    Ok(Some(value.to_string()))
}

fn spawn_saver(data_dir: PathBuf, receiver: std::sync::mpsc::Receiver<SaveRequest>) {
    let _ = std::thread::Builder::new()
        .name("sushload-state".to_string())
        .spawn(move || {
            while let Ok(request) = receiver.recv() {
                let _ = state::save_bytes(&data_dir, &request.bytes);
                if let Some(ack) = request.ack {
                    let _ = ack.send(());
                }
            }
        });
}

async fn scheduler(inner: Arc<Inner>, mut commands: mpsc::UnboundedReceiver<Command>) {
    let mut tasks: HashMap<DownloadId, tokio::task::JoinHandle<()>> = HashMap::new();
    schedule_queued(&inner, &mut tasks);

    while let Some(command) = commands.recv().await {
        match command {
            Command::Schedule => schedule_queued(&inner, &mut tasks),
            Command::Finished(id) => {
                tasks.remove(&id);
                schedule_queued(&inner, &mut tasks);
            }
            Command::DeleteFiles {
                part,
                final_path,
                delete_final,
            } => {
                let _ = tokio::task::spawn_blocking(move || {
                    let _ = std::fs::remove_file(&part);
                    if delete_final {
                        let _ = std::fs::remove_file(&final_path);
                    }
                })
                .await;
            }
            Command::Shutdown(ack) => {
                {
                    let mut state = lock(&inner.state);
                    for entry in &mut state.downloads {
                        if entry.record.status.is_active() {
                            entry.record.status = Status::Paused;
                            entry.speed = 0;
                            entry.eta_secs = None;
                            entry.connections = 0;
                            if let Some(cancel) = &entry.cancel {
                                let _ = cancel.send(true);
                            }
                        }
                    }
                }

                let deadline = tokio::time::Instant::now() + Duration::from_millis(1500);
                for (id, mut handle) in tasks.drain() {
                    let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
                    if tokio::time::timeout(remaining, &mut handle).await.is_err() {
                        handle.abort();
                        reset_progress(&inner, id);
                    }
                }

                inner.persist_wait();
                let _ = ack.send(());
                return;
            }
        }
    }
}

fn schedule_queued(
    inner: &Arc<Inner>,
    tasks: &mut HashMap<DownloadId, tokio::task::JoinHandle<()>>,
) {
    let mut started = false;

    loop {
        let (id, cancel_rx) = {
            let mut state = lock(&inner.state);
            let maximum = state.settings.max_concurrent.max(1) as usize;
            let active = state
                .downloads
                .iter()
                .filter(|entry| {
                    matches!(
                        entry.record.status,
                        Status::Connecting | Status::Downloading
                    )
                })
                .count();
            if active >= maximum {
                break;
            }

            let next = state
                .downloads
                .iter()
                .filter(|entry| {
                    entry.record.status == Status::Queued && !tasks.contains_key(&entry.record.id)
                })
                .min_by_key(|entry| (entry.record.added_at, entry.record.id.0))
                .map(|entry| entry.record.id);
            let Some(id) = next else {
                break;
            };
            let Some(entry) = state.find_mut(id) else {
                break;
            };

            entry.record.status = Status::Connecting;
            entry.record.error = None;
            entry.speed = 0;
            entry.eta_secs = None;
            entry.connections = 0;
            let (cancel_tx, cancel_rx) = watch::channel(false);
            entry.cancel = Some(cancel_tx);
            (id, cancel_rx)
        };

        let handle = tokio::spawn(download::run(Arc::clone(inner), id, cancel_rx));
        tasks.insert(id, handle);
        started = true;
    }

    if started {
        inner.persist();
    }
}

fn reset_progress(inner: &Arc<Inner>, id: DownloadId) {
    if let Some(entry) = lock(&inner.state).find_mut(id) {
        entry.record.segments.clear();
        entry.record.received = 0;
    }
}
