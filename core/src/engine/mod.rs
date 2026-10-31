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
use crate::model::{
    AddRequest, DownloadId, DownloadView, Event, ProbeInfo, Settings, SiteFolders, Status,
};
use crate::redact::{redact_proxy, redact_url};
use crate::sites::{self, SiteEntry, SiteGroup, SiteView};
use crate::sites_ai::{self, SiteQuery};
use client::{Net, RequestContext};
use ratelimit::RateLimiter;
use state::{DownloadRecord, StateFile};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::{mpsc, watch};

pub use util::sanitize_file_name;

/// Longest a probe may take, as seen by the engine runtime; callers wait a little longer.
const PROBE_TIMEOUT: Duration = Duration::from_secs(8);

type EventHandler = Arc<dyn Fn(&Event) + Send + Sync>;

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
    /// Domains seen by the site folders, with the user's choices and cached AI answers.
    pub sites: HashMap<String, SiteEntry>,
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
            sites: self.sites.clone(),
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
    /// Set by the engine thread once its runtime exists; used to run probes.
    handle: std::sync::OnceLock<tokio::runtime::Handle>,
    events: Mutex<Option<EventHandler>>,
    /// Test/override path of the AI helper; normally it is located on demand.
    ai_helper: Mutex<Option<PathBuf>>,
    /// Domains the AI was asked about (or failed on) during this run: never asked twice.
    ai_asked: Mutex<HashSet<String>>,
    /// Result of the helper's `--check`, remembered once positive.
    ai_ok: Mutex<bool>,
}

/// What `Inner::placed_dir` needs to know about the download to pick its site folder.
pub(crate) struct SiteCtx<'a> {
    pub url: &'a str,
    pub referer: Option<&'a str>,
    pub title: Option<&'a str>,
    pub file_name: &'a str,
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

    /// Tells the registered handler (if any) about a state change. Called outside any lock.
    pub fn emit(&self, event: Event) {
        let handler = lock(&self.events).clone();
        if let Some(handler) = handler {
            handler(&event);
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

/// Most domains remembered; beyond that new ones are filed but not recorded.
const MAX_SITES: usize = 5000;

impl Inner {
    /// Folder a new file of `category` goes to when the dialog didn't pick one: the category
    /// folder, plus the site levels when site folders are on. With `record`, a first-seen
    /// domain is remembered (one log line) and, if it is unknown, the AI is asked in the
    /// background; until it answers the group is `Other`.
    pub(crate) fn placed_dir(
        self: &Arc<Self>,
        settings: &Settings,
        category: crate::model::Category,
        site: &SiteCtx<'_>,
        record: bool,
    ) -> PathBuf {
        if settings.site_folders == SiteFolders::Off {
            return categories::dir_for(settings, category);
        }
        let Some(domain) = sites::site_domain(site.url, site.referer) else {
            return categories::dir_for(settings, category);
        };
        let (group, source, folder, fresh, ask_ai) = {
            let mut state = lock(&self.state);
            let fresh = record && !state.sites.contains_key(&domain);
            if fresh && state.sites.len() < MAX_SITES {
                state.sites.insert(domain.clone(), SiteEntry::default());
            }
            let entry = state.sites.get(&domain);
            let (group, source) = sites::decide(&domain, entry);
            let folder = sites::folder_name(&domain, entry);
            let ask_ai = record && sites::wants_ai(settings, &domain, entry);
            (group, source, folder, fresh, ask_ai)
        };
        if fresh {
            log::info!(
                target: "engine",
                "site: {domain} \u{2192} {} ({})",
                group.label(),
                source.describe()
            );
            self.persist();
        } else if record {
            log::debug!(
                target: "engine",
                "site: {domain} \u{2192} {} (cached, {})",
                group.label(),
                source.describe()
            );
        }
        if ask_ai {
            let path = reqwest::Url::parse(site.url)
                .map(|u| u.path().to_string())
                .unwrap_or_default();
            self.ask_ai(
                domain,
                SiteQuery {
                    domain: String::new(),
                    page_title: site.title.map(|t| t.chars().take(200).collect()),
                    url_path: path.chars().take(200).collect(),
                    file_name: site.file_name.chars().take(120).collect(),
                },
            );
        }
        sites::site_dir(settings, category, group, &folder)
    }

    /// Asks the helper about `domain` on a background thread, once per domain and run. The
    /// answer (or "unsure") is cached in the state file; failures are only logged.
    fn ask_ai(self: &Arc<Self>, domain: String, mut query: SiteQuery) {
        if !lock(&self.ai_asked).insert(domain.clone()) {
            return;
        }
        query.domain = domain.clone();
        let inner = Arc::clone(self);
        let spawned = std::thread::Builder::new()
            .name("sushload-site-ai".to_string())
            .spawn(move || {
                let Some(helper) = inner.ai_helper_path() else {
                    log::debug!(target: "engine", "site: {domain}: no AI helper installed");
                    return;
                };
                if !inner.ai_available(&helper) {
                    log::info!(target: "engine", "site: {domain}: AI helper unavailable, using rules only");
                    return;
                }
                log::debug!(
                    target: "engine",
                    "site: {domain}: asking the on-device model (title: {:?})",
                    query.page_title.as_deref().unwrap_or("").chars().take(80).collect::<String>()
                );
                let started = Instant::now();
                match sites_ai::classify(&helper, &query) {
                    Ok(choice) => {
                        {
                            let mut state = lock(&inner.state);
                            let entry = state.sites.entry(domain.clone()).or_default();
                            entry.ai_asked = true;
                            entry.ai = choice;
                        }
                        inner.persist();
                        log::info!(
                            target: "engine",
                            "site: {domain} \u{2192} {} (ai, {} ms)",
                            choice.map_or("unsure", SiteGroup::label),
                            started.elapsed().as_millis()
                        );
                    }
                    Err(error) => {
                        log::warn!(target: "engine", "site: {domain}: AI failed: {error}");
                    }
                }
            });
        if let Err(error) = spawned {
            log::warn!(target: "engine", "site: cannot start the AI thread: {error}");
        }
    }

    fn ai_helper_path(&self) -> Option<PathBuf> {
        lock(&self.ai_helper)
            .clone()
            .or_else(sites_ai::locate_helper)
    }

    fn ai_available(&self, helper: &std::path::Path) -> bool {
        if *lock(&self.ai_ok) {
            return true;
        }
        let ok = sites_ai::is_available(helper);
        *lock(&self.ai_ok) = ok;
        ok
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
                sites: file.sites,
            }),
            limiter: Arc::new(RateLimiter::new(speed_limit)),
            net: Mutex::new(net),
            tx,
            save_tx,
            handle: std::sync::OnceLock::new(),
            events: Mutex::new(None),
            ai_helper: Mutex::new(None),
            ai_asked: Mutex::new(HashSet::new()),
            ai_ok: Mutex::new(false),
        });
        log::info!(
            target: "engine",
            "loaded {} download(s) from {}",
            lock(&inner.state).downloads.len(),
            data_dir.display()
        );

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
                let _ = thread_inner.handle.set(runtime.handle().clone());
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

    /// Registers the function that hears about completed and failed downloads. It runs on an
    /// engine thread right after the state change, so it must be quick (spawn, don't wait).
    pub fn set_event_handler(&self, handler: impl Fn(&Event) + Send + Sync + 'static) {
        *lock(&self.inner.events) = Some(Arc::new(handler));
    }

    /// Like `add`, with the browser's file name, Referer, cookies and User-Agent. The file name
    /// wins over the URL and Content-Disposition; the headers go only to the URL's own origin
    /// (cookies never follow a redirect to another origin) and are kept in `state.json`
    /// (owner-only) but never exposed through `DownloadView` or errors. `dir`, `category`,
    /// `start_paused` and `overwrite` come from the confirmation dialog.
    pub fn add_request(&self, request: AddRequest) -> Result<DownloadId, String> {
        let prepared = prepare(&request)?;
        let name_fixed = prepared.fixed_name.is_some();
        let file_name = prepared
            .fixed_name
            .unwrap_or_else(|| util::file_name_from_url(&prepared.parsed));
        let category = request
            .category
            .unwrap_or_else(|| categories::classify(&file_name));
        let settings = { lock(&self.inner.state).settings.clone() };
        let dir = request.dir.clone().unwrap_or_else(|| {
            self.inner.placed_dir(
                &settings,
                category,
                &SiteCtx {
                    url: &prepared.url,
                    referer: prepared.referer.as_deref(),
                    title: request.page_title.as_deref(),
                    file_name: &file_name,
                },
                true,
            )
        });
        let path = dir.join(&file_name);
        let status = if request.start_paused {
            Status::Paused
        } else {
            Status::Queued
        };

        let mut state = lock(&self.inner.state);
        let id = DownloadId(state.next_id);
        state.next_id += 1;
        state.downloads.push(Entry {
            record: DownloadRecord {
                id,
                url: prepared.url.clone(),
                file_name: file_name.clone(),
                path,
                category,
                status,
                total: None,
                received: 0,
                segments: Vec::new(),
                resumable: false,
                etag: None,
                last_modified: None,
                error: None,
                added_at: now_secs(),
                name_fixed,
                referer: prepared.referer,
                cookies: prepared.cookies,
                user_agent: prepared.user_agent,
                dir: request.dir.clone(),
                overwrite: request.overwrite,
                category_override: request.category,
            },
            speed: 0,
            eta_secs: None,
            connections: 0,
            cancel: None,
        });
        drop(state);

        log::info!(
            target: "engine",
            "add id={} url={} name={} dir={} paused={} overwrite={}",
            id.0,
            redact_url(&prepared.url),
            file_name,
            request
                .dir
                .as_ref()
                .map_or_else(|| "(default)".to_string(), |d| d.display().to_string()),
            request.start_paused,
            request.overwrite
        );
        self.inner.persist();
        if !request.start_paused {
            let _ = self.inner.tx.send(Command::Schedule);
        }
        Ok(id)
    }

    /// One `Range: bytes=0-0` request (same headers and cookie rules as the download itself)
    /// to learn name, size and resumability, plus where the file would go. Blocks the caller
    /// for at most ~10 s; the request runs on the engine runtime.
    pub fn probe(&self, request: &AddRequest) -> Result<ProbeInfo, String> {
        let prepared = prepare(request)?;
        let settings = self.settings();
        let net = self.inner.net()?.with_context(RequestContext::new(
            &prepared.url,
            prepared.referer.clone(),
            prepared.cookies.clone(),
            prepared.user_agent.clone(),
        ));
        let handle = self.inner.handle.get().ok_or("engine is not running")?;
        let (tx, rx) = std::sync::mpsc::channel();
        let url = prepared.url.clone();
        handle.spawn(async move {
            // Keep the sender alive: a dropped one would read as "cancelled".
            let (_keep, mut cancel) = tokio::sync::watch::channel(false);
            let result =
                tokio::time::timeout(PROBE_TIMEOUT, download::probe_once(&net, &url, &mut cancel))
                    .await;
            let _ = tx.send(match result {
                Ok(Ok(probe)) => Ok(probe),
                Ok(Err(download::ProbeError::Cancelled)) => Err("cancelled".to_string()),
                Ok(Err(download::ProbeError::Retry(m) | download::ProbeError::Failed(m))) => Err(m),
                Err(_) => Err("the server did not answer in time".to_string()),
            });
        });
        let result = rx
            .recv_timeout(PROBE_TIMEOUT + Duration::from_secs(2))
            .map_err(|_| "the server did not answer in time".to_string())
            .and_then(|r| r);
        let probe = match result {
            Ok(probe) => probe,
            Err(error) => {
                log::warn!(
                    target: "engine",
                    "probe {} failed: {}",
                    redact_url(&prepared.url),
                    crate::redact::redact_text(&error)
                );
                return Err(error);
            }
        };

        let file_name = prepared
            .fixed_name
            .or(probe.file_name)
            .unwrap_or_else(|| util::file_name_from_url(&prepared.parsed));
        let category = request
            .category
            .unwrap_or_else(|| categories::classify(&file_name));
        let dir = request.dir.clone().unwrap_or_else(|| {
            self.inner.placed_dir(
                &settings,
                category,
                &SiteCtx {
                    url: &prepared.url,
                    referer: prepared.referer.as_deref(),
                    title: request.page_title.as_deref(),
                    file_name: &file_name,
                },
                true,
            )
        });
        let exists = categories::name_taken(&dir, &file_name);
        log::info!(
            target: "engine",
            "probe {} -> name={} size={} resumable={} exists={}",
            redact_url(&prepared.url),
            file_name,
            probe.total.map_or_else(|| "unknown".to_string(), |t| t.to_string()),
            probe.resumable,
            exists
        );
        Ok(ProbeInfo {
            file_name,
            total: probe.total,
            resumable: probe.resumable,
            category,
            dir,
            exists,
        })
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
            log::info!(target: "engine", "pause id={}", id.0);
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
            log::info!(target: "engine", "resume id={}", id.0);
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
        log::info!(target: "engine", "remove id={} delete_file={}", id.0, delete_file);
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

    /// Every domain seen by the site folders, with its effective group and folder.
    pub fn sites(&self) -> Vec<SiteView> {
        let state = lock(&self.inner.state);
        let mut list: Vec<SiteView> = state
            .sites
            .iter()
            .map(|(domain, entry)| {
                let (group, source) = sites::decide(domain, Some(entry));
                SiteView {
                    domain: domain.clone(),
                    group,
                    origin: source.badge(),
                    folder: sites::folder_name(domain, Some(entry)),
                    user_group: entry.user_group,
                    user_folder: entry.user_folder.clone(),
                }
            })
            .collect();
        list.sort_by(|a, b| a.domain.cmp(&b.domain));
        list
    }

    /// Sets (or with `None` clears) the user's group and folder name for a domain. Applies to
    /// new downloads only.
    pub fn set_site(
        &self,
        domain: &str,
        group: Option<SiteGroup>,
        folder: Option<String>,
    ) -> Result<(), String> {
        let domain = domain.trim().to_ascii_lowercase();
        if domain.is_empty() || domain.len() > 253 {
            return Err("domain is empty".to_string());
        }
        let folder = folder
            .map(|f| sites::sanitize_folder(&f))
            .filter(|f| !f.is_empty() && *f != domain);
        {
            let mut state = lock(&self.inner.state);
            if !state.sites.contains_key(&domain) && state.sites.len() >= MAX_SITES {
                return Err("too many sites".to_string());
            }
            let entry = state.sites.entry(domain.clone()).or_default();
            entry.user_group = group;
            entry.user_folder = folder.clone();
        }
        log::info!(
            target: "engine",
            "site: {domain} set by user: group={} folder={}",
            group.map_or("auto", SiteGroup::label),
            folder.as_deref().unwrap_or("(domain)")
        );
        self.inner.persist();
        Ok(())
    }

    /// Forgets the user's group and folder name for a domain (back to automatic).
    pub fn reset_site(&self, domain: &str) {
        let domain = domain.trim().to_ascii_lowercase();
        let changed = {
            let mut state = lock(&self.inner.state);
            match state.sites.get_mut(&domain) {
                Some(entry) => {
                    let had = entry.user_group.is_some() || entry.user_folder.is_some();
                    entry.user_group = None;
                    entry.user_folder = None;
                    had
                }
                None => false,
            }
        };
        if changed {
            log::info!(target: "engine", "site: {domain} reset to automatic");
            self.inner.persist();
        }
    }

    /// The site levels (`University/sharif.edu`) a download like this would get below its
    /// category folder; None when site folders are off or the URL has no host. Changes
    /// nothing, so the UI may call it freely.
    pub fn site_subpath(&self, request: &AddRequest) -> Option<PathBuf> {
        let state = lock(&self.inner.state);
        if state.settings.site_folders == SiteFolders::Off {
            return None;
        }
        let referer = request.referer.as_deref().filter(|r| !r.trim().is_empty());
        let domain = sites::site_domain(&request.url, referer)?;
        let entry = state.sites.get(&domain);
        let (group, _) = sites::decide(&domain, entry);
        let folder = sites::folder_name(&domain, entry);
        sites::site_subpath(state.settings.site_folders, group, &folder)
    }

    /// Points the engine at a specific AI helper (tests); `None` goes back to searching.
    pub fn set_ai_helper(&self, path: Option<PathBuf>) {
        *lock(&self.inner.ai_helper) = path;
        *lock(&self.inner.ai_ok) = false;
    }

    /// Whether the AI helper exists and says it can classify right now. Runs the helper's
    /// `--check`, so it may take a moment; call it from a request thread, never the UI thread.
    pub fn ai_available(&self) -> bool {
        match self.inner.ai_helper_path() {
            Some(helper) => sites_ai::is_available(&helper),
            None => false,
        }
    }

    pub fn settings(&self) -> Settings {
        lock(&self.inner.state).settings.clone()
    }

    pub fn set_settings(&self, settings: Settings) {
        let (proxy_changed, old) = {
            let mut state = lock(&self.inner.state);
            let changed = state.settings.proxy_mode != settings.proxy_mode
                || state.settings.proxy_url != settings.proxy_url;
            let old = std::mem::replace(&mut state.settings, settings.clone());
            (changed, old)
        };
        log_settings_change(&old, &settings);
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

fn log_settings_change(old: &Settings, new: &Settings) {
    if old == new {
        return;
    }
    let mut changes: Vec<String> = Vec::new();
    if old.download_dir != new.download_dir {
        changes.push(format!("download_dir={}", new.download_dir.display()));
    }
    if old.sort_into_categories != new.sort_into_categories {
        changes.push(format!("sort_into_categories={}", new.sort_into_categories));
    }
    if old.max_concurrent != new.max_concurrent {
        changes.push(format!("max_concurrent={}", new.max_concurrent));
    }
    if old.connections != new.connections {
        changes.push(format!("connections={}", new.connections));
    }
    if old.speed_limit != new.speed_limit {
        changes.push(format!("speed_limit={}", new.speed_limit));
    }
    if old.proxy_mode != new.proxy_mode || old.proxy_url != new.proxy_url {
        let url = if new.proxy_mode == crate::model::ProxyMode::Custom {
            redact_proxy(&new.proxy_url)
        } else {
            "-".to_string()
        };
        changes.push(format!("proxy={:?} {url}", new.proxy_mode));
    }
    if old.confirm_new_downloads != new.confirm_new_downloads {
        changes.push(format!(
            "confirm_new_downloads={}",
            new.confirm_new_downloads
        ));
    }
    if old.notify_on_complete != new.notify_on_complete {
        changes.push(format!("notify_on_complete={}", new.notify_on_complete));
    }
    if old.site_folders != new.site_folders {
        changes.push(format!("site_folders={}", new.site_folders.id()));
    }
    if old.ai_site_groups != new.ai_site_groups {
        changes.push(format!("ai_site_groups={}", new.ai_site_groups));
    }
    if old.category_dirs != new.category_dirs {
        let mut dirs: Vec<String> = new
            .category_dirs
            .iter()
            .map(|(c, d)| format!("{}={}", c.label(), d.display()))
            .collect();
        dirs.sort();
        changes.push(format!("category_dirs=[{}]", dirs.join(", ")));
    }
    log::info!(target: "engine", "settings changed: {}", changes.join("; "));
}

/// A request that passed validation, with its headers cleaned.
struct Prepared {
    url: String,
    parsed: reqwest::Url,
    referer: Option<String>,
    cookies: Option<String>,
    user_agent: Option<String>,
    fixed_name: Option<String>,
}

/// Checks URL, headers and folder of a request without touching the engine.
fn prepare(request: &AddRequest) -> Result<Prepared, String> {
    let url = request.url.trim();
    let parsed = reqwest::Url::parse(url).map_err(|e| format!("invalid URL: {e}"))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err("only http and https URLs are supported".to_string());
    }
    if parsed.host_str().is_none_or(str::is_empty) {
        return Err("URL has no host".to_string());
    }
    if let Some(dir) = &request.dir
        && !dir.is_absolute()
    {
        return Err("the folder must be an absolute path".to_string());
    }
    let fixed_name = request
        .file_name
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(util::sanitize_file_name);
    Ok(Prepared {
        url: url.to_string(),
        parsed,
        referer: clean_header(request.referer.clone(), "Referer")?,
        cookies: clean_header(request.cookies.clone(), "cookies")?,
        user_agent: clean_header(request.user_agent.clone(), "User-Agent")?,
        fixed_name,
    })
}

/// Same checks `add_request` does, for callers that queue a request before adding it.
pub fn validate_request(request: &AddRequest) -> Result<(), String> {
    prepare(request).map(|_| ())
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
