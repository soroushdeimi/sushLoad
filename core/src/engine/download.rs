//! Probing and the actual transfer workers: segmented ranges, dynamic splitting, single-stream
//! fallback, retries, cancellation, speed sampling and finalisation.

use super::client::Net;
use super::ratelimit::RateLimiter;
use super::segments;
use super::util;
use super::{Command, Inner, lock};
use crate::categories;
use crate::model::{DownloadId, Status};
use futures_util::StreamExt;
use reqwest::StatusCode;
use reqwest::header::{
    ACCEPT_ENCODING, CONTENT_DISPOSITION, CONTENT_RANGE, ETAG, LAST_MODIFIED, RANGE, USER_AGENT,
};
use std::fs::{File, OpenOptions};
use std::io::ErrorKind;
use std::os::unix::fs::FileExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};
use tokio::sync::watch;
use tokio::task::JoinSet;

const USER_AGENT_VALUE: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) \
     Version/17.0 Safari/605.1.15 sushLoad/0.1";
const MIN_SEGMENTED_TOTAL: u64 = 2 * 1024 * 1024;
const MAX_ATTEMPTS: u32 = 5;
const SPEED_TICK: Duration = Duration::from_millis(500);
/// Stream chunks are coalesced up to this many bytes before one positioned write.
const WRITE_BUFFER: usize = 256 * 1024;

/// `<final path>.part`
pub(crate) fn part_path(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(".part");
    PathBuf::from(name)
}

/// Errors inside one worker. `Fallback` means the server ignored `Range`; the whole download has
/// to restart as a single stream.
enum WorkerError {
    Cancelled,
    Fallback,
    Retry(String),
    Fatal(String),
}

enum DownloadError {
    Cancelled,
    Fallback,
    Failed(String),
}

impl From<WorkerError> for DownloadError {
    fn from(value: WorkerError) -> Self {
        match value {
            WorkerError::Cancelled => DownloadError::Cancelled,
            WorkerError::Fallback => DownloadError::Fallback,
            WorkerError::Retry(message) | WorkerError::Fatal(message) => {
                DownloadError::Failed(message)
            }
        }
    }
}

fn failed(message: impl Into<String>) -> DownloadError {
    DownloadError::Failed(message.into())
}

fn is_cancelled(cancel: &watch::Receiver<bool>) -> bool {
    *cancel.borrow()
}

/// Resolves when cancellation is requested (or the sender goes away).
async fn wait_cancel(cancel: &mut watch::Receiver<bool>) {
    if *cancel.borrow() {
        return;
    }
    let _ = cancel.changed().await;
}

struct Probe {
    total: Option<u64>,
    resumable: bool,
    etag: Option<String>,
    last_modified: Option<String>,
    file_name: Option<String>,
}

enum ProbeError {
    Cancelled,
    Retry(String),
    Failed(String),
}

/// Task entry point: runs one download and records success/failure, then wakes the scheduler.
pub(crate) async fn run(inner: Arc<Inner>, id: DownloadId, cancel: watch::Receiver<bool>) {
    let result = run_download(&inner, id, cancel).await;
    match result {
        Ok(()) => {}
        Err(DownloadError::Cancelled) => mark_paused(&inner, id),
        Err(DownloadError::Fallback) => {
            mark_failed(
                &inner,
                id,
                "server stopped supporting byte ranges".to_string(),
            );
        }
        Err(DownloadError::Failed(message)) => mark_failed(&inner, id, message),
    }
    let _ = inner.tx.send(Command::Finished(id));
}

fn mark_failed(inner: &Arc<Inner>, id: DownloadId, message: String) {
    {
        let mut state = lock(&inner.state);
        if let Some(entry) = state.find_mut(id)
            && matches!(
                entry.record.status,
                Status::Connecting | Status::Downloading
            )
        {
            entry.record.status = Status::Failed;
            entry.record.error = Some(message);
            entry.speed = 0;
            entry.eta_secs = None;
            entry.connections = 0;
        }
    }
    inner.persist();
}

async fn run_download(
    inner: &Arc<Inner>,
    id: DownloadId,
    mut cancel: watch::Receiver<bool>,
) -> Result<(), DownloadError> {
    if is_cancelled(&cancel) {
        return Err(DownloadError::Cancelled);
    }

    let (
        url,
        current_name,
        current_path,
        old_total,
        old_etag,
        old_last_modified,
        old_resumable,
        had_progress,
    ) = {
        let mut state = lock(&inner.state);
        let Some(entry) = state.find_mut(id) else {
            return Err(DownloadError::Cancelled);
        };
        if !matches!(entry.record.status, Status::Connecting) {
            return Err(DownloadError::Cancelled);
        }
        entry.record.status = Status::Connecting;
        entry.record.error = None;
        entry.speed = 0;
        entry.eta_secs = None;
        entry.connections = 0;
        let record = &entry.record;
        let had_progress = record.received > 0 || !record.segments.is_empty();
        (
            record.url.clone(),
            record.file_name.clone(),
            record.path.clone(),
            record.total,
            record.etag.clone(),
            record.last_modified.clone(),
            record.resumable,
            had_progress,
        )
    };
    inner.persist();

    if is_cancelled(&cancel) {
        return Err(DownloadError::Cancelled);
    }

    let net = inner.net().map_err(DownloadError::Failed)?;
    let probe = probe_with_retry(&net, &url, &mut cancel).await?;
    let settings = { lock(&inner.state).settings.clone() };

    let (path, file_name, category) = if had_progress {
        let name = current_path
            .file_name()
            .and_then(|name| name.to_str())
            .map(|name| name.to_string())
            .unwrap_or(current_name);
        let category = categories::classify(&name);
        (current_path, name, category)
    } else {
        let name = probe
            .file_name
            .as_deref()
            .map(util::sanitize_file_name)
            .unwrap_or(current_name);
        let dir = categories::destination_dir(&settings, &name);
        std::fs::create_dir_all(&dir)
            .map_err(|e| failed(format!("cannot create {}: {e}", dir.display())))?;
        let path = categories::unique_path(&dir, &name);
        let category = categories::classify(&name);
        (path, name, category)
    };

    let part = part_path(&path);
    let mut reset = !had_progress;
    if had_progress {
        if !part.exists() {
            reset = true;
        }
        if !probe.resumable || old_resumable != probe.resumable {
            reset = true;
        }
        match (old_total, probe.total) {
            (Some(old), Some(new)) if old != new => reset = true,
            (Some(_), None) => reset = true,
            _ => {}
        }
        if differs(&old_etag, &probe.etag) || differs(&old_last_modified, &probe.last_modified) {
            reset = true;
        }
    }

    {
        let mut state = lock(&inner.state);
        let Some(entry) = state.find_mut(id) else {
            return Err(DownloadError::Cancelled);
        };
        entry.record.file_name = file_name;
        entry.record.path = path.clone();
        entry.record.category = category;
        entry.record.total = probe.total;
        entry.record.resumable = probe.resumable;
        entry.record.etag = probe.etag.clone();
        entry.record.last_modified = probe.last_modified.clone();
        entry.record.error = None;
        if reset {
            entry.record.segments.clear();
            entry.record.received = 0;
        } else {
            entry.record.received = entry.record.segments.iter().map(|s| s.received).sum();
        }
    }

    let total = probe.total;
    let segmented = probe.resumable
        && total.is_some_and(|total| total >= MIN_SEGMENTED_TOTAL)
        && total.is_some();

    if segmented {
        let total = total.unwrap_or(0);
        match run_segmented(
            inner,
            id,
            &net,
            &url,
            total,
            &path,
            &settings,
            cancel.clone(),
        )
        .await
        {
            Ok(()) => return Ok(()),
            Err(DownloadError::Fallback) => {}
            Err(error) => return Err(error),
        }
    }

    run_single(inner, id, &net, &url, &path, total, cancel).await
}

fn differs(old: &Option<String>, new: &Option<String>) -> bool {
    match (old, new) {
        (Some(old), Some(new)) => old != new,
        _ => false,
    }
}

async fn probe_with_retry(
    net: &Net,
    url: &str,
    cancel: &mut watch::Receiver<bool>,
) -> Result<Probe, DownloadError> {
    let mut attempt = 0u32;
    loop {
        attempt += 1;
        match probe_once(net, url, cancel).await {
            Ok(probe) => return Ok(probe),
            Err(ProbeError::Cancelled) => return Err(DownloadError::Cancelled),
            Err(ProbeError::Failed(message)) => return Err(DownloadError::Failed(message)),
            Err(ProbeError::Retry(message)) => {
                if attempt >= MAX_ATTEMPTS {
                    return Err(DownloadError::Failed(message));
                }
                backoff(attempt, cancel).await?;
            }
        }
    }
}

async fn probe_once(
    net: &Net,
    url: &str,
    cancel: &mut watch::Receiver<bool>,
) -> Result<Probe, ProbeError> {
    let request = net
        .get(url)
        .header(RANGE, "bytes=0-0")
        .header(ACCEPT_ENCODING, "identity")
        .header(USER_AGENT, USER_AGENT_VALUE);

    let response = tokio::select! {
        response = request.send() => response,
        _ = wait_cancel(cancel) => return Err(ProbeError::Cancelled),
    };
    let response = match response {
        Ok(response) => response,
        Err(error) if net.is_proxy_unreachable(&error) => {
            return Err(ProbeError::Failed(net.describe(&error)));
        }
        Err(error) => return Err(ProbeError::Retry(net.describe(&error))),
    };

    let status = response.status();
    if status == StatusCode::TOO_MANY_REQUESTS || status == StatusCode::SERVICE_UNAVAILABLE {
        return Err(ProbeError::Retry(format!("HTTP {}", status.as_u16())));
    }
    if !status.is_success() {
        return Err(ProbeError::Failed(format!("HTTP {}", status.as_u16())));
    }

    let headers = response.headers();
    let etag = header_string(headers.get(ETAG));
    let last_modified = header_string(headers.get(LAST_MODIFIED));
    let file_name = headers
        .get(CONTENT_DISPOSITION)
        .and_then(|value| value.to_str().ok())
        .and_then(util::parse_content_disposition)
        .map(|name| util::sanitize_file_name(&name));

    let (total, resumable) = if status == StatusCode::PARTIAL_CONTENT {
        let range = headers
            .get(CONTENT_RANGE)
            .and_then(|value| value.to_str().ok())
            .and_then(util::parse_content_range);
        let total = range.and_then(|range| range.total);
        (total, total.is_some())
    } else {
        (response.content_length(), false)
    };

    drop(response);

    Ok(Probe {
        total,
        resumable,
        etag,
        last_modified,
        file_name,
    })
}

fn header_string(value: Option<&reqwest::header::HeaderValue>) -> Option<String> {
    value
        .and_then(|value| value.to_str().ok())
        .map(|value| value.to_string())
}

async fn backoff(attempt: u32, cancel: &mut watch::Receiver<bool>) -> Result<(), DownloadError> {
    let shift = attempt.saturating_sub(1).min(4);
    let seconds = 1u64 << shift;
    tokio::select! {
        _ = tokio::time::sleep(Duration::from_secs(seconds)) => Ok(()),
        _ = wait_cancel(cancel) => Err(DownloadError::Cancelled),
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_segmented(
    inner: &Arc<Inner>,
    id: DownloadId,
    net: &Net,
    url: &str,
    total: u64,
    path: &Path,
    settings: &crate::model::Settings,
    cancel: watch::Receiver<bool>,
) -> Result<(), DownloadError> {
    let (pending, already_done) = {
        let mut state = lock(&inner.state);
        let Some(entry) = state.find_mut(id) else {
            return Err(DownloadError::Cancelled);
        };
        if entry.record.segments.is_empty()
            || segments::check_invariants(&entry.record.segments, total).is_err()
        {
            entry.record.segments = segments::split_equal(total, settings.connections);
            entry.record.received = 0;
        }
        entry.record.received = entry.record.segments.iter().map(|s| s.received).sum();
        entry.record.status = Status::Downloading;
        entry.record.error = None;

        let pending: Vec<usize> = entry
            .record
            .segments
            .iter()
            .enumerate()
            .filter(|(_, segment)| !segment.is_done())
            .map(|(index, _)| index)
            .collect();
        let done = pending.is_empty() && entry.record.received >= total;
        (pending, done)
    };

    if pending.is_empty() {
        if already_done {
            let file = open_part(path, Some(total))?;
            return finalize(inner, id, path, Arc::new(file), total).await;
        }
        return Err(failed("no resumable segments left"));
    }

    let worker_total = pending.len() as u32;
    let file = Arc::new(open_part(path, Some(total))?);
    inner.persist();

    let workers_alive = Arc::new(AtomicU32::new(0));
    let worker_limit = Arc::new(AtomicU32::new(worker_total));
    let mut set: JoinSet<Result<(), WorkerError>> = JoinSet::new();

    for index in pending {
        workers_alive.fetch_add(1, Ordering::SeqCst);
        set.spawn(segment_worker(
            Arc::clone(inner),
            id,
            index,
            net.clone(),
            url.to_string(),
            Arc::clone(&file),
            Arc::clone(&inner.limiter),
            cancel.clone(),
            Arc::clone(&workers_alive),
            Arc::clone(&worker_limit),
        ));
    }

    let monitored = monitor(inner, id, &mut set, cancel, worker_total, Some(total)).await;
    monitored?;

    let all_done = {
        let state = lock(&inner.state);
        state.find(id).is_some_and(|entry| {
            entry
                .record
                .segments
                .iter()
                .all(|segment| segment.is_done())
                && entry.record.received >= total
        })
    };
    if !all_done {
        return Err(failed("download ended before every segment was received"));
    }

    finalize(inner, id, path, file, total).await
}

#[allow(clippy::too_many_arguments)]
async fn segment_worker(
    inner: Arc<Inner>,
    id: DownloadId,
    mut index: usize,
    client: Net,
    url: String,
    file: Arc<File>,
    limiter: Arc<RateLimiter>,
    mut cancel: watch::Receiver<bool>,
    workers_alive: Arc<AtomicU32>,
    worker_limit: Arc<AtomicU32>,
) -> Result<(), WorkerError> {
    let _guard = AliveGuard(Arc::clone(&workers_alive));

    loop {
        if is_cancelled(&cancel) {
            return Err(WorkerError::Cancelled);
        }

        let next = {
            let mut state = lock(&inner.state);
            let Some(entry) = state.find_mut(id) else {
                return Err(WorkerError::Cancelled);
            };
            let Some(segment) = entry.record.segments.get(index) else {
                return Ok(());
            };
            if segment.is_done() {
                let alive = workers_alive.load(Ordering::SeqCst);
                let limit = worker_limit.load(Ordering::SeqCst).max(1);
                if alive > limit {
                    return Ok(());
                }
                match segments::split_largest(&mut entry.record.segments) {
                    Some(new_index) => {
                        index = new_index;
                        continue;
                    }
                    None => return Ok(()),
                }
            }
            (segment.start + segment.received, segment.end)
        };

        let (next_start, end) = next;
        if next_start > end {
            continue;
        }

        let mut attempts = 0u32;
        loop {
            if attempts >= MAX_ATTEMPTS {
                return Err(WorkerError::Fatal(format!(
                    "giving up on bytes {next_start}-{end} after {MAX_ATTEMPTS} attempts"
                )));
            }
            match serve_segment(
                &inner,
                id,
                index,
                &client,
                &url,
                &file,
                &limiter,
                &mut cancel,
                &worker_limit,
            )
            .await
            {
                Ok(true) => break,
                Ok(false) => {
                    attempts += 1;
                    if attempts >= MAX_ATTEMPTS {
                        return Err(WorkerError::Fatal(
                            "connection closed before the segment was complete".to_string(),
                        ));
                    }
                    sleep_backoff(attempts, &mut cancel).await?;
                    if is_cancelled(&cancel) {
                        return Err(WorkerError::Cancelled);
                    }
                }
                Err(WorkerError::Cancelled) => return Err(WorkerError::Cancelled),
                Err(WorkerError::Fallback) => return Err(WorkerError::Fallback),
                Err(WorkerError::Fatal(message)) => return Err(WorkerError::Fatal(message)),
                Err(WorkerError::Retry(message)) => {
                    attempts += 1;
                    if attempts >= MAX_ATTEMPTS {
                        return Err(WorkerError::Fatal(message));
                    }
                    sleep_backoff(attempts, &mut cancel).await?;
                }
            }
        }
    }
}

async fn sleep_backoff(
    attempt: u32,
    cancel: &mut watch::Receiver<bool>,
) -> Result<(), WorkerError> {
    let seconds = 1u64 << attempt.saturating_sub(1).min(4);
    tokio::select! {
        _ = tokio::time::sleep(Duration::from_secs(seconds)) => Ok(()),
        _ = wait_cancel(cancel) => Err(WorkerError::Cancelled),
    }
}

/// Serve the current segment once. `Ok(true)` means the segment is complete; `Ok(false)` means the
/// connection ended early and the caller should retry from `received`.
#[allow(clippy::too_many_arguments)]
async fn serve_segment(
    inner: &Arc<Inner>,
    id: DownloadId,
    index: usize,
    client: &Net,
    url: &str,
    file: &Arc<File>,
    limiter: &RateLimiter,
    cancel: &mut watch::Receiver<bool>,
    worker_limit: &AtomicU32,
) -> Result<bool, WorkerError> {
    let (next_start, end) = {
        let state = lock(&inner.state);
        let Some(entry) = state.find(id) else {
            return Err(WorkerError::Cancelled);
        };
        let Some(segment) = entry.record.segments.get(index) else {
            return Ok(true);
        };
        (segment.start + segment.received, segment.end)
    };
    if next_start > end {
        return Ok(true);
    }

    let request = client
        .get(url)
        .header(RANGE, format!("bytes={next_start}-{end}"))
        .header(ACCEPT_ENCODING, "identity")
        .header(USER_AGENT, USER_AGENT_VALUE);

    let response = tokio::select! {
        response = request.send() => response,
        _ = wait_cancel(cancel) => return Err(WorkerError::Cancelled),
    };
    let response = match response {
        Ok(response) => response,
        Err(error) => return Err(WorkerError::Retry(client.describe(&error))),
    };

    let status = response.status();
    if status == StatusCode::TOO_MANY_REQUESTS || status == StatusCode::SERVICE_UNAVAILABLE {
        lower_limit(worker_limit);
        return Err(WorkerError::Retry(format!("HTTP {}", status.as_u16())));
    }
    if status == StatusCode::OK {
        return Err(WorkerError::Fallback);
    }
    if status != StatusCode::PARTIAL_CONTENT {
        return Err(WorkerError::Fatal(format!("HTTP {}", status.as_u16())));
    }

    let mut stream = response.bytes_stream();
    let mut buffer: Vec<u8> = Vec::with_capacity(WRITE_BUFFER);
    loop {
        let remaining = {
            let state = lock(&inner.state);
            match state
                .find(id)
                .and_then(|entry| entry.record.segments.get(index))
            {
                Some(segment) => segment.remaining(),
                None => return Ok(true),
            }
        };
        if remaining == 0 {
            break;
        }

        let item = tokio::select! {
            item = stream.next() => item,
            _ = wait_cancel(cancel) => return Err(WorkerError::Cancelled),
        };
        let Some(item) = item else {
            break;
        };
        let chunk = match item {
            Ok(chunk) => chunk,
            Err(error) => {
                // Keep what already arrived so the retry resumes after it.
                let _ = flush_segment_buffer(inner, id, index, file, &mut buffer, cancel).await;
                return Err(WorkerError::Retry(client.describe(&error)));
            }
        };

        tokio::select! {
            _ = limiter.acquire(chunk.len() as u64) => {}
            _ = wait_cancel(cancel) => return Err(WorkerError::Cancelled),
        }

        buffer.extend_from_slice(&chunk);
        if buffer.len() >= WRITE_BUFFER || buffer.len() as u64 >= remaining {
            flush_segment_buffer(inner, id, index, file, &mut buffer, cancel).await?;
        }
    }
    flush_segment_buffer(inner, id, index, file, &mut buffer, cancel).await?;

    let complete = {
        let state = lock(&inner.state);
        state
            .find(id)
            .and_then(|entry| entry.record.segments.get(index))
            .is_some_and(|segment| segment.is_done())
    };
    Ok(complete)
}

fn lower_limit(limit: &AtomicU32) {
    let mut current = limit.load(Ordering::SeqCst);
    loop {
        let next = current.saturating_sub(1).max(1);
        match limit.compare_exchange(current, next, Ordering::SeqCst, Ordering::SeqCst) {
            Ok(_) => return,
            Err(actual) => current = actual,
        }
    }
}

/// Write one coalesced buffer at the segment's current write offset, then record how many bytes
/// actually reached the file. `received` is never advanced ahead of the write, so a crash or a
/// cancellation between the write and the state update merely causes those bytes to be fetched
/// again on resume; it can never leave a hole behind `received`.
async fn flush_segment_buffer(
    inner: &Arc<Inner>,
    id: DownloadId,
    index: usize,
    file: &Arc<File>,
    buffer: &mut Vec<u8>,
    cancel: &watch::Receiver<bool>,
) -> Result<(), WorkerError> {
    if buffer.is_empty() {
        return Ok(());
    }
    if is_cancelled(cancel) {
        buffer.clear();
        return Err(WorkerError::Cancelled);
    }

    let (offset, take) = {
        let state = lock(&inner.state);
        let Some(entry) = state.find(id) else {
            buffer.clear();
            return Err(WorkerError::Cancelled);
        };
        let Some(segment) = entry.record.segments.get(index) else {
            buffer.clear();
            return Ok(());
        };
        let take = (buffer.len() as u64).min(segment.remaining()) as usize;
        if take == 0 {
            buffer.clear();
            return Ok(());
        }
        (segment.start + segment.received, take)
    };

    let data = buffer[..take].to_vec();
    let writer = Arc::clone(file);
    let written = tokio::task::spawn_blocking(move || write_all_at(&writer, &data, offset))
        .await
        .map_err(|error| WorkerError::Retry(error.to_string()))?;
    if let Err(error) = written {
        buffer.clear();
        return Err(WorkerError::Retry(error.to_string()));
    }

    {
        let mut state = lock(&inner.state);
        let Some(entry) = state.find_mut(id) else {
            buffer.clear();
            return Err(WorkerError::Cancelled);
        };
        let Some(segment) = entry.record.segments.get_mut(index) else {
            buffer.clear();
            return Ok(());
        };
        let take = take.min(segment.remaining() as usize);
        segment.received += take as u64;
        entry.record.received += take as u64;
    }
    buffer.clear();
    Ok(())
}

fn write_all_at(file: &File, mut data: &[u8], mut offset: u64) -> std::io::Result<()> {
    while !data.is_empty() {
        match file.write_at(data, offset) {
            Ok(0) => {
                return Err(std::io::Error::new(
                    ErrorKind::WriteZero,
                    "write_at returned zero",
                ));
            }
            Ok(written) => {
                data = &data[written..];
                offset += written as u64;
            }
            Err(error) if error.kind() == ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

async fn run_single(
    inner: &Arc<Inner>,
    id: DownloadId,
    net: &Net,
    url: &str,
    path: &Path,
    expected_total: Option<u64>,
    cancel: watch::Receiver<bool>,
) -> Result<(), DownloadError> {
    {
        let mut state = lock(&inner.state);
        let Some(entry) = state.find_mut(id) else {
            return Err(DownloadError::Cancelled);
        };
        entry.record.segments.clear();
        entry.record.received = 0;
        entry.record.total = expected_total;
        entry.record.resumable = false;
        entry.record.status = Status::Downloading;
        entry.record.error = None;
    }

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| failed(format!("cannot create {}: {e}", parent.display())))?;
    }
    let part = part_path(path);
    let file = Arc::new(
        OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(true)
            .open(&part)
            .map_err(|e| failed(format!("cannot open {}: {e}", part.display())))?,
    );
    inner.persist();

    let mut set: JoinSet<Result<(), WorkerError>> = JoinSet::new();
    set.spawn(single_worker(
        Arc::clone(inner),
        id,
        net.clone(),
        url.to_string(),
        Arc::clone(&file),
        Arc::clone(&inner.limiter),
        cancel.clone(),
        expected_total,
    ));

    monitor(inner, id, &mut set, cancel, 1, expected_total).await?;

    let received = {
        let state = lock(&inner.state);
        state
            .find(id)
            .map(|entry| entry.record.received)
            .unwrap_or(0)
    };
    let total = expected_total.unwrap_or(received);
    finalize(inner, id, path, file, total).await
}

#[allow(clippy::too_many_arguments)]
async fn single_worker(
    inner: Arc<Inner>,
    id: DownloadId,
    client: Net,
    url: String,
    file: Arc<File>,
    limiter: Arc<RateLimiter>,
    mut cancel: watch::Receiver<bool>,
    expected_total: Option<u64>,
) -> Result<(), WorkerError> {
    let mut attempts = 0u32;
    loop {
        if attempts >= MAX_ATTEMPTS {
            return Err(WorkerError::Fatal(
                "giving up on the single-connection download".to_string(),
            ));
        }
        match serve_single(
            &inner,
            id,
            &client,
            &url,
            &file,
            &limiter,
            &mut cancel,
            expected_total,
        )
        .await
        {
            Ok(()) => return Ok(()),
            Err(WorkerError::Cancelled) => return Err(WorkerError::Cancelled),
            Err(WorkerError::Fatal(message)) => return Err(WorkerError::Fatal(message)),
            Err(WorkerError::Fallback) | Err(WorkerError::Retry(_)) => {
                attempts += 1;
                reset_single(&inner, id, &file).await?;
                sleep_backoff(attempts, &mut cancel).await?;
            }
        }
    }
}

async fn reset_single(
    inner: &Arc<Inner>,
    id: DownloadId,
    file: &Arc<File>,
) -> Result<(), WorkerError> {
    {
        let mut state = lock(&inner.state);
        let Some(entry) = state.find_mut(id) else {
            return Err(WorkerError::Cancelled);
        };
        entry.record.received = 0;
    }
    let reset = Arc::clone(file);
    tokio::task::spawn_blocking(move || reset.set_len(0))
        .await
        .map_err(|error| WorkerError::Fatal(error.to_string()))?
        .map_err(|error| WorkerError::Fatal(error.to_string()))
}

#[allow(clippy::too_many_arguments)]
async fn serve_single(
    inner: &Arc<Inner>,
    id: DownloadId,
    client: &Net,
    url: &str,
    file: &Arc<File>,
    limiter: &RateLimiter,
    cancel: &mut watch::Receiver<bool>,
    expected_total: Option<u64>,
) -> Result<(), WorkerError> {
    let request = client
        .get(url)
        .header(ACCEPT_ENCODING, "identity")
        .header(USER_AGENT, USER_AGENT_VALUE);

    let response = tokio::select! {
        response = request.send() => response,
        _ = wait_cancel(cancel) => return Err(WorkerError::Cancelled),
    };
    let response = match response {
        Ok(response) => response,
        Err(error) => return Err(WorkerError::Retry(client.describe(&error))),
    };

    let status = response.status();
    if status == StatusCode::TOO_MANY_REQUESTS || status == StatusCode::SERVICE_UNAVAILABLE {
        return Err(WorkerError::Retry(format!("HTTP {}", status.as_u16())));
    }
    if status != StatusCode::OK && status != StatusCode::PARTIAL_CONTENT {
        return Err(WorkerError::Fatal(format!("HTTP {}", status.as_u16())));
    }

    let mut stream = response.bytes_stream();
    let mut buffer: Vec<u8> = Vec::with_capacity(WRITE_BUFFER);
    loop {
        let room = {
            let state = lock(&inner.state);
            let Some(entry) = state.find(id) else {
                return Err(WorkerError::Cancelled);
            };
            expected_total
                .map(|total| total.saturating_sub(entry.record.received))
                .unwrap_or(u64::MAX)
        };
        if room == 0 {
            break;
        }

        let item = tokio::select! {
            item = stream.next() => item,
            _ = wait_cancel(cancel) => return Err(WorkerError::Cancelled),
        };
        let Some(item) = item else {
            break;
        };
        let chunk = match item {
            Ok(chunk) => chunk,
            Err(error) => {
                // Keep what already arrived so the retry resumes after it.
                let _ =
                    flush_single_buffer(inner, id, file, &mut buffer, expected_total, cancel).await;
                return Err(WorkerError::Retry(client.describe(&error)));
            }
        };

        tokio::select! {
            _ = limiter.acquire(chunk.len() as u64) => {}
            _ = wait_cancel(cancel) => return Err(WorkerError::Cancelled),
        }

        buffer.extend_from_slice(&chunk);
        if buffer.len() >= WRITE_BUFFER || buffer.len() as u64 >= room {
            flush_single_buffer(inner, id, file, &mut buffer, expected_total, cancel).await?;
        }
    }
    flush_single_buffer(inner, id, file, &mut buffer, expected_total, cancel).await?;

    let received = {
        let state = lock(&inner.state);
        state
            .find(id)
            .map(|entry| entry.record.received)
            .unwrap_or(0)
    };
    match expected_total {
        Some(total) if received < total => Err(WorkerError::Retry(format!(
            "stream ended at {received} of {total} bytes"
        ))),
        _ => Ok(()),
    }
}

async fn flush_single_buffer(
    inner: &Arc<Inner>,
    id: DownloadId,
    file: &Arc<File>,
    buffer: &mut Vec<u8>,
    expected_total: Option<u64>,
    cancel: &watch::Receiver<bool>,
) -> Result<(), WorkerError> {
    if buffer.is_empty() {
        return Ok(());
    }
    if is_cancelled(cancel) {
        buffer.clear();
        return Err(WorkerError::Cancelled);
    }

    let (offset, take) = {
        let state = lock(&inner.state);
        let Some(entry) = state.find(id) else {
            buffer.clear();
            return Err(WorkerError::Cancelled);
        };
        let room = expected_total
            .map(|total| total.saturating_sub(entry.record.received))
            .unwrap_or(u64::MAX);
        let take = (buffer.len() as u64).min(room) as usize;
        if take == 0 {
            buffer.clear();
            return Ok(());
        }
        (entry.record.received, take)
    };

    let data = buffer[..take].to_vec();
    let writer = Arc::clone(file);
    let written = tokio::task::spawn_blocking(move || write_all_at(&writer, &data, offset))
        .await
        .map_err(|error| WorkerError::Retry(error.to_string()))?;
    if let Err(error) = written {
        buffer.clear();
        return Err(WorkerError::Retry(error.to_string()));
    }

    {
        let mut state = lock(&inner.state);
        let Some(entry) = state.find_mut(id) else {
            buffer.clear();
            return Err(WorkerError::Cancelled);
        };
        let room = expected_total
            .map(|total| total.saturating_sub(entry.record.received))
            .unwrap_or(u64::MAX);
        let take = take.min(room as usize);
        entry.record.received += take as u64;
    }
    buffer.clear();
    Ok(())
}

async fn monitor(
    inner: &Arc<Inner>,
    id: DownloadId,
    set: &mut JoinSet<Result<(), WorkerError>>,
    mut cancel: watch::Receiver<bool>,
    worker_count: u32,
    total_hint: Option<u64>,
) -> Result<(), DownloadError> {
    {
        let mut state = lock(&inner.state);
        if let Some(entry) = state.find_mut(id) {
            entry.connections = worker_count;
        }
    }

    let mut ticker = tokio::time::interval(SPEED_TICK);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    let mut last_received = current_received(inner, id);
    let mut last_sample = Instant::now();
    let mut save_ticks = 0u32;
    let mut remaining_workers = worker_count;

    loop {
        tokio::select! {
            _ = wait_cancel(&mut cancel) => {
                while set.join_next().await.is_some() {}
                mark_paused(inner, id);
                return Err(DownloadError::Cancelled);
            }
            joined = set.join_next() => {
                match joined {
                    None => break,
                    Some(Ok(Ok(()))) => {
                        remaining_workers = remaining_workers.saturating_sub(1);
                        set_connections(inner, id, remaining_workers);
                    }
                    Some(Ok(Err(WorkerError::Cancelled))) => {
                        while set.join_next().await.is_some() {}
                        mark_paused(inner, id);
                        return Err(DownloadError::Cancelled);
                    }
                    Some(Ok(Err(WorkerError::Fallback))) => {
                        while set.join_next().await.is_some() {}
                        return Err(DownloadError::Fallback);
                    }
                    Some(Ok(Err(WorkerError::Retry(message)))) | Some(Ok(Err(WorkerError::Fatal(message)))) => {
                        set.abort_all();
                        while set.join_next().await.is_some() {}
                        return Err(DownloadError::Failed(message));
                    }
                    Some(Err(error)) => {
                        set.abort_all();
                        while set.join_next().await.is_some() {}
                        return Err(DownloadError::Failed(format!("worker stopped: {error}")));
                    }
                }
            }
            _ = ticker.tick() => {
                let now = Instant::now();
                let received = current_received(inner, id);
                let elapsed = now.duration_since(last_sample).as_secs_f64();
                if elapsed > 0.0 {
                    let instant = received.saturating_sub(last_received) as f64 / elapsed;
                    let mut state = lock(&inner.state);
                    if let Some(entry) = state.find_mut(id) {
                        if entry.speed == 0 {
                            entry.speed = instant as u64;
                        } else {
                            entry.speed = (entry.speed as f64 * 0.7 + instant * 0.3) as u64;
                        }
                        let total = total_hint.or(entry.record.total);
                        if let Some(total) = total {
                            let remaining = total.saturating_sub(received);
                            entry.eta_secs = remaining.checked_div(entry.speed);
                        }
                    }
                }
                last_received = received;
                last_sample = now;
                save_ticks += 1;
                if save_ticks.is_multiple_of(4) {
                    inner.persist();
                }
            }
        }
    }

    if let Some(entry) = lock(&inner.state).find_mut(id) {
        entry.connections = 0;
    }
    Ok(())
}

fn current_received(inner: &Arc<Inner>, id: DownloadId) -> u64 {
    lock(&inner.state)
        .find(id)
        .map(|entry| entry.record.received)
        .unwrap_or(0)
}

fn set_connections(inner: &Arc<Inner>, id: DownloadId, connections: u32) {
    if let Some(entry) = lock(&inner.state).find_mut(id) {
        entry.connections = connections;
    }
}

fn mark_paused(inner: &Arc<Inner>, id: DownloadId) {
    let changed = {
        let mut state = lock(&inner.state);
        match state.find_mut(id) {
            Some(entry)
                if matches!(
                    entry.record.status,
                    Status::Connecting | Status::Downloading
                ) =>
            {
                entry.record.status = Status::Paused;
                entry.speed = 0;
                entry.eta_secs = None;
                entry.connections = 0;
                true
            }
            _ => false,
        }
    };
    if changed {
        inner.persist();
    }
}

fn open_part(path: &Path, total: Option<u64>) -> Result<File, DownloadError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| failed(format!("cannot create {}: {e}", parent.display())))?;
    }
    let part = part_path(path);
    let file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(&part)
        .map_err(|e| failed(format!("cannot open {}: {e}", part.display())))?;
    if let Some(total) = total {
        file.set_len(total)
            .map_err(|e| failed(format!("cannot size {}: {e}", part.display())))?;
    }
    Ok(file)
}

async fn finalize(
    inner: &Arc<Inner>,
    id: DownloadId,
    path: &Path,
    file: Arc<File>,
    total: u64,
) -> Result<(), DownloadError> {
    let exists = { lock(&inner.state).find(id).is_some() };
    if !exists {
        return Err(DownloadError::Cancelled);
    }

    let flush = Arc::clone(&file);
    tokio::task::spawn_blocking(move || flush.sync_all())
        .await
        .map_err(|e| failed(e.to_string()))?
        .map_err(|e| failed(format!("cannot flush partial file: {e}")))?;
    drop(file);

    let part = part_path(path);
    let destination = path.to_path_buf();
    tokio::task::spawn_blocking(move || std::fs::rename(&part, &destination))
        .await
        .map_err(|e| failed(e.to_string()))?
        .map_err(|e| failed(format!("cannot move completed file: {e}")))?;

    {
        let mut state = lock(&inner.state);
        let Some(entry) = state.find_mut(id) else {
            let _ = std::fs::remove_file(path);
            return Err(DownloadError::Cancelled);
        };
        entry.record.status = Status::Completed;
        entry.record.total = Some(total);
        entry.record.received = total;
        entry.record.error = None;
        entry.speed = 0;
        entry.eta_secs = None;
        entry.connections = 0;
    }
    inner.persist();
    Ok(())
}

/// Keeps `workers_alive` in sync even when a worker exits through `?`.
struct AliveGuard(Arc<AtomicU32>);

impl Drop for AliveGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}
