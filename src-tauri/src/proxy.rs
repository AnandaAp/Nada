use std::collections::hash_map::RandomState;
use std::collections::{HashMap, VecDeque};
use std::hash::BuildHasher;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const DEFAULT_USER_AGENT: &str =
    "com.google.ios.youtube/20.03.02 (iPhone16,2; U; CPU iOS 18_2_1 like Mac OS X)";
const MAX_DOWNLOADED_LRU: usize = 10;
const MAX_SESSION_METADATA: usize = 300;
const METADATA_WAIT_TIMEOUT: Duration = Duration::from_secs(5);
const TLS_METADATA_WAIT_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_CONSECUTIVE_FAILURES: u32 = 5;
const MAX_RECONNECTS: u32 = 16;
const MAX_ALR_HOPS: u32 = 5;
const MAX_ALR_BODY_BYTES: usize = 8 * 1024;
const MAX_CONCURRENT_CONNECTIONS: usize = 128;
const IDLE_KEEPALIVE_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_HTTP_HEADER_BYTES: usize = 32 * 1024;

#[cfg(windows)]
const FILE_SHARE_ALL: u32 = 0x1 | 0x2 | 0x4;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StreamRegisterHeaders {
    pub user_agent: Option<String>,
    pub origin: Option<String>,
    pub referer: Option<String>,
}

pub struct SharedProgress {
    pub downloaded: AtomicU64,
    pub total: Mutex<Option<u64>>,
    pub mime: Mutex<String>,
    pub headers_received: AtomicBool,
    pub is_complete: AtomicBool,
    pub is_cancelled: AtomicBool,
    pub error: Mutex<Option<String>>,
    pub notify: Condvar,
    pub lock: Mutex<()>,
    async_tx: tokio::sync::watch::Sender<u64>,
}

impl SharedProgress {
    pub fn new() -> Self {
        let (async_tx, _) = tokio::sync::watch::channel(0u64);
        Self {
            downloaded: AtomicU64::new(0),
            total: Mutex::new(None),
            mime: Mutex::new("application/octet-stream".to_string()),
            headers_received: AtomicBool::new(false),
            is_complete: AtomicBool::new(false),
            is_cancelled: AtomicBool::new(false),
            error: Mutex::new(None),
            notify: Condvar::new(),
            lock: Mutex::new(()),
            async_tx,
        }
    }
}

fn notify_progress(progress: &SharedProgress) {
    let _guard = progress.lock.lock().unwrap();
    progress.notify.notify_all();
    progress.async_tx.send_modify(|v| *v = v.wrapping_add(1));
}

pub struct TempFileGuard {
    path: PathBuf,
    delete_on_drop: AtomicBool,
}

impl TempFileGuard {
    fn new(path: PathBuf) -> Self {
        Self {
            path,
            delete_on_drop: AtomicBool::new(false),
        }
    }
}

impl Drop for TempFileGuard {
    fn drop(&mut self) {
        if self.delete_on_drop.load(Ordering::Acquire) {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

struct ActiveReaderGuard {
    active_readers: Arc<AtomicUsize>,
}

impl ActiveReaderGuard {
    fn new(active_readers: &Arc<AtomicUsize>) -> Self {
        active_readers.fetch_add(1, Ordering::AcqRel);
        Self {
            active_readers: Arc::clone(active_readers),
        }
    }
}

impl Drop for ActiveReaderGuard {
    fn drop(&mut self) {
        self.active_readers.fetch_sub(1, Ordering::AcqRel);
    }
}

fn open_shared_write(path: &Path) -> std::io::Result<std::fs::File> {
    let mut opts = std::fs::OpenOptions::new();
    opts.create(true).write(true).truncate(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        opts.share_mode(FILE_SHARE_ALL);
    }
    opts.open(path)
}

fn open_shared_read(path: &Path) -> std::io::Result<std::fs::File> {
    let mut opts = std::fs::OpenOptions::new();
    opts.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        opts.share_mode(FILE_SHARE_ALL);
    }
    opts.open(path)
}

pub struct StreamSession {
    pub token: String,
    pub url: String,
    pub headers: StreamRegisterHeaders,
    pub file_path: PathBuf,
    pub progress: Arc<SharedProgress>,
    pub started: AtomicBool,
    pub active_readers: Arc<AtomicUsize>,
    pub file_guard: Arc<TempFileGuard>,
}

struct SessionRegistry {
    sessions: HashMap<String, Arc<StreamSession>>,
    token_order: VecDeque<String>,
    downloaded_lru: VecDeque<String>,
}

struct ProxyRuntime {
    rt: Option<tokio::runtime::Runtime>,
    shutdown: tokio::sync::watch::Sender<bool>,
}

impl Drop for ProxyRuntime {
    fn drop(&mut self) {
        let _ = self.shutdown.send(true);
        if let Some(rt) = self.rt.take() {
            rt.shutdown_background();
        }
    }
}

pub struct StreamProxy {
    port: u16,
    registry: Arc<Mutex<SessionRegistry>>,
    next_seq: AtomicU64,
    hasher: RandomState,
    cache_dir: PathBuf,
    shutdown: tokio::sync::watch::Sender<bool>,
    _runtime: Arc<ProxyRuntime>,
    _log: Arc<Mutex<std::fs::File>>,
}

fn log_line(log: &Arc<Mutex<std::fs::File>>, line: String) {
    if let Ok(mut f) = log.lock() {
        let _ = writeln!(f, "{line}");
    }
}

async fn wait_shutdown(shutdown_rx: &mut tokio::sync::watch::Receiver<bool>) {
    if *shutdown_rx.borrow() {
        return;
    }
    while shutdown_rx.changed().await.is_ok() {
        if *shutdown_rx.borrow() {
            return;
        }
    }
}

impl StreamProxy {
    #[allow(dead_code)]
    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn start(log_path: PathBuf, cache_dir: PathBuf) -> Result<Self, String> {
        let log = Arc::new(Mutex::new(
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(log_path)
                .map_err(|e| e.to_string())?,
        ));
        let registry = Arc::new(Mutex::new(SessionRegistry {
            sessions: HashMap::new(),
            token_order: VecDeque::new(),
            downloaded_lru: VecDeque::new(),
        }));

        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .read_timeout(Duration::from_secs(30))
            .redirect(reqwest::redirect::Policy::limited(5))
            .build()
            .map_err(|e| e.to_string())?;

        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(4)
            .enable_all()
            .build()
            .map_err(|e| e.to_string())?;

        let listener = rt
            .block_on(tokio::net::TcpListener::bind("127.0.0.1:0"))
            .map_err(|e| e.to_string())?;
        let port = listener.local_addr().map_err(|e| e.to_string())?.port();

        let (shutdown_tx, mut shutdown_rx) = tokio::sync::watch::channel(false);
        let runtime = Arc::new(ProxyRuntime {
            rt: Some(rt),
            shutdown: shutdown_tx.clone(),
        });

        let semaphore = Arc::new(tokio::sync::Semaphore::new(MAX_CONCURRENT_CONNECTIONS));
        let acc_registry = registry.clone();
        let acc_client = client.clone();
        let acc_runtime = runtime.clone();
        let acc_log = log.clone();
        let rt_handle = runtime.rt.as_ref().unwrap().handle().clone();

        rt_handle.spawn(async move {
            let spawn_handle = tokio::runtime::Handle::current();
            loop {
                let permit = tokio::select! {
                    _ = wait_shutdown(&mut shutdown_rx) => break,
                    res = semaphore.clone().acquire_owned() => match res {
                        Ok(p) => p,
                        Err(_) => break,
                    }
                };
                tokio::select! {
                    _ = wait_shutdown(&mut shutdown_rx) => {
                        drop(permit);
                        break;
                    }
                    accepted = listener.accept() => {
                        match accepted {
                            Ok((stream, _)) => {
                                let _ = stream.set_nodelay(true);
                                let reg = acc_registry.clone();
                                let cli = acc_client.clone();
                                let rt_arc = acc_runtime.clone();
                                let lg = acc_log.clone();
                                let sd = shutdown_rx.clone();
                                spawn_handle.spawn(async move {
                                    let _permit = permit;
                                    handle_connection(stream, reg, cli, rt_arc, lg, sd).await;
                                });
                            }
                            Err(_) => {
                                drop(permit);
                                tokio::time::sleep(Duration::from_millis(10)).await;
                            }
                        }
                    }
                }
            }
        });

        Ok(Self {
            port,
            registry,
            next_seq: AtomicU64::new(1),
            hasher: RandomState::new(),
            cache_dir,
            shutdown: shutdown_tx,
            _runtime: runtime,
            _log: log,
        })
    }

    pub fn register(&self, url: &str, total: Option<u64>, mime: Option<&str>) -> String {
        self.register_with_headers(url, total, mime, StreamRegisterHeaders::default())
    }

    pub fn register_with_headers(
        &self,
        url: &str,
        total: Option<u64>,
        mime: Option<&str>,
        headers: StreamRegisterHeaders,
    ) -> String {
        let token = format!(
            "{:016x}",
            self.hasher
                .hash_one((url, &headers.user_agent, &headers.origin, &headers.referer))
        );

        let mut evicted_to_drop: Vec<Arc<StreamSession>> = Vec::new();
        {
            let mut reg = self.registry.lock().unwrap();

            if let Some(existing) = reg.sessions.get(&token).cloned() {
                let cancelled = existing.progress.is_cancelled.load(Ordering::Acquire);
                let has_err = existing.progress.error.lock().unwrap().is_some();
                if !cancelled && !has_err {
                    if let Some(t) = total {
                        let mut total_lock = existing.progress.total.lock().unwrap();
                        if total_lock.is_none() {
                            *total_lock = Some(t);
                        }
                    }
                    if let Some(m) = mime {
                        *existing.progress.mime.lock().unwrap() = m.to_string();
                    }
                    reg.token_order.retain(|t| t != &token);
                    reg.token_order.push_back(token.clone());
                    return format!("http://127.0.0.1:{}/s/{}", self.port, token);
                }

                reg.sessions.remove(&token);
                reg.token_order.retain(|t| t != &token);
                reg.downloaded_lru.retain(|t| t != &token);
                existing
                    .progress
                    .is_cancelled
                    .store(true, Ordering::Release);
                existing
                    .file_guard
                    .delete_on_drop
                    .store(true, Ordering::Release);
                notify_progress(&existing.progress);
                evicted_to_drop.push(existing);
            }

            let seq = self.next_seq.fetch_add(1, Ordering::Relaxed);
            let file_path = self.cache_dir.join(format!("{}_{}.tmp", token, seq));
            let progress = Arc::new(SharedProgress::new());
            if let Some(t) = total {
                *progress.total.lock().unwrap() = Some(t);
            }
            if let Some(m) = mime {
                *progress.mime.lock().unwrap() = m.to_string();
            }

            let session = Arc::new(StreamSession {
                token: token.clone(),
                url: url.to_string(),
                headers,
                file_path: file_path.clone(),
                progress,
                started: AtomicBool::new(false),
                active_readers: Arc::new(AtomicUsize::new(0)),
                file_guard: Arc::new(TempFileGuard::new(file_path)),
            });

            reg.sessions.insert(token.clone(), session);
            reg.token_order.push_back(token.clone());

            while reg.token_order.len() > MAX_SESSION_METADATA {
                let candidate_pos = reg
                    .token_order
                    .iter()
                    .position(|t| {
                        reg.sessions
                            .get(t)
                            .map_or(true, |s| s.active_readers.load(Ordering::Acquire) == 0)
                    })
                    .unwrap_or(0);
                if let Some(evicted_token) = reg.token_order.remove(candidate_pos) {
                    reg.downloaded_lru.retain(|t| t != &evicted_token);
                    if let Some(evicted_session) = reg.sessions.remove(&evicted_token) {
                        evicted_session
                            .progress
                            .is_cancelled
                            .store(true, Ordering::Release);
                        evicted_session
                            .file_guard
                            .delete_on_drop
                            .store(true, Ordering::Release);
                        notify_progress(&evicted_session.progress);
                        evicted_to_drop.push(evicted_session);
                    }
                } else {
                    break;
                }
            }
        }
        drop(evicted_to_drop);

        format!("http://127.0.0.1:{}/s/{}", self.port, token)
    }
}

impl Drop for StreamProxy {
    fn drop(&mut self) {
        let _ = self.shutdown.send(true);
        let mut sessions_to_drop = Vec::new();
        if let Ok(mut reg) = self.registry.lock() {
            for (_, session) in reg.sessions.drain() {
                session.progress.is_cancelled.store(true, Ordering::Release);
                session
                    .file_guard
                    .delete_on_drop
                    .store(true, Ordering::Release);
                notify_progress(&session.progress);
                sessions_to_drop.push(session);
            }
            reg.token_order.clear();
            reg.downloaded_lru.clear();
        }
        drop(sessions_to_drop);
    }
}

#[allow(dead_code)]
pub fn parse_range(value: Option<&str>, total: Option<u64>) -> (u64, Option<u64>) {
    let Some(v) = value else {
        return (0, None);
    };
    let Some(spec) = v.trim().strip_prefix("bytes=") else {
        return (0, None);
    };
    let (left, right) = match spec.split_once('-') {
        Some(pair) => pair,
        None => return (0, None),
    };
    if left.trim().is_empty() {
        let n: u64 = right.trim().parse().unwrap_or(0);
        return match total {
            Some(t) => (t.saturating_sub(n), Some(t.saturating_sub(1))),
            None => (0, None),
        };
    }
    let start: u64 = left.trim().parse().unwrap_or(0);
    if right.trim().is_empty() {
        return (start, None);
    }
    let end: u64 = right.trim().parse().unwrap_or(start);
    (start, Some(end))
}

fn parse_range_checked(value: Option<&str>, total: u64) -> Result<(u64, u64, bool), ()> {
    let Some(v) = value else {
        return if total == 0 {
            Ok((0, 0, false))
        } else {
            Ok((0, total - 1, false))
        };
    };
    if total == 0 {
        return Err(());
    }
    let spec = v.trim().strip_prefix("bytes=").ok_or(())?;
    let (left, right) = spec.split_once('-').ok_or(())?;
    if left.trim().is_empty() {
        let n: u64 = right.trim().parse().map_err(|_| ())?;
        if n == 0 {
            return Err(());
        }
        return Ok((total.saturating_sub(n), total - 1, true));
    }
    let start: u64 = left.trim().parse().map_err(|_| ())?;
    if start >= total {
        return Err(());
    }
    if right.trim().is_empty() {
        return Ok((start, total - 1, true));
    }
    let end: u64 = right.trim().parse().map_err(|_| ())?;
    if end < start {
        return Err(());
    }
    Ok((start, end.min(total - 1), true))
}

fn parse_content_range(val: &str) -> Option<(Option<u64>, Option<u64>, Option<u64>)> {
    let trimmed = val.trim();
    let prefix = trimmed.get(..5)?;
    if !prefix.eq_ignore_ascii_case("bytes") {
        return None;
    }
    let next_b = *trimmed.as_bytes().get(5)?;
    if !next_b.is_ascii_whitespace() {
        return None;
    }
    let rest = trimmed[5..].trim();
    let (range_part, total_part) = rest.split_once('/')?;
    let total = if total_part.trim() == "*" {
        None
    } else {
        Some(total_part.trim().parse::<u64>().ok()?)
    };
    if range_part.trim() == "*" {
        return Some((None, None, total));
    }
    let (start_s, end_s) = range_part.split_once('-')?;
    let start = start_s.trim().parse::<u64>().ok()?;
    let end = end_s.trim().parse::<u64>().ok()?;
    Some((Some(start), Some(end), total))
}

fn sanitize_content_type(mime: &str) -> String {
    let cleaned: String = mime
        .chars()
        .filter(|&c| !c.is_control() && c != '\r' && c != '\n' && c.is_ascii())
        .collect();
    let trimmed = cleaned.trim();
    if trimmed.is_empty() {
        "application/octet-stream".to_string()
    } else {
        trimmed.to_string()
    }
}

fn extract_media_type(ct_header: &str) -> String {
    ct_header
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase()
}

fn is_acceptable_media_type(media_type: &str) -> bool {
    media_type.starts_with("audio/")
        || media_type.starts_with("video/")
        || media_type == "application/octet-stream"
}

fn parse_alr_redirect_url(raw_body: &[u8], current_url: &str) -> Option<String> {
    let text = std::str::from_utf8(raw_body).ok()?;
    let trimmed = text.trim();
    if trimmed.is_empty() || trimmed.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return None;
    }
    let parsed = reqwest::Url::parse(trimmed).ok()?;
    if parsed.host_str().is_none() {
        return None;
    }
    let current_is_https = current_url
        .get(..8)
        .is_some_and(|p| p.eq_ignore_ascii_case("https://"));
    match parsed.scheme() {
        "https" => Some(parsed.to_string()),
        "http" if !current_is_https => Some(parsed.to_string()),
        _ => None,
    }
}

fn apply_session_headers(
    mut req: reqwest::RequestBuilder,
    headers: &StreamRegisterHeaders,
) -> reqwest::RequestBuilder {
    let ua = headers
        .user_agent
        .as_deref()
        .filter(|s| !s.is_empty())
        .unwrap_or(DEFAULT_USER_AGENT);
    req = req.header(reqwest::header::USER_AGENT, ua);
    if let Some(origin) = headers.origin.as_deref().filter(|s| !s.is_empty()) {
        req = req.header(reqwest::header::ORIGIN, origin);
    }
    if let Some(referer) = headers.referer.as_deref().filter(|s| !s.is_empty()) {
        req = req.header(reqwest::header::REFERER, referer);
    }
    req
}

fn download_worker(
    session: Arc<StreamSession>,
    client: reqwest::Client,
    runtime: Arc<ProxyRuntime>,
    log: Arc<Mutex<std::fs::File>>,
) {
    let _file_guard = session.file_guard.clone();
    log_line(
        &log,
        format!("download_worker started for token {}", session.token),
    );

    let mut writer = match open_shared_write(&session.file_path) {
        Ok(f) => f,
        Err(e) => {
            log_line(
                &log,
                format!("download_worker file create error {}: {e}", session.token),
            );
            *session.progress.error.lock().unwrap() = Some(e.to_string());
            notify_progress(&session.progress);
            return;
        }
    };

    let rt = match runtime.rt.as_ref() {
        Some(rt) => rt,
        None => {
            *session.progress.error.lock().unwrap() = Some("proxy runtime stopped".to_string());
            notify_progress(&session.progress);
            return;
        }
    };

    let mut current_url = session.url.clone();
    let mut alr_hops = 0u32;
    let mut offset = 0u64;
    let mut total_size = *session.progress.total.lock().unwrap();
    let mut consecutive_failures = 0u32;
    let mut reconnects = 0u32;

    loop {
        if session.progress.is_cancelled.load(Ordering::Acquire) {
            log_line(
                &log,
                format!("download_worker cancelled for token {}", session.token),
            );
            break;
        }

        if let Some(total) = total_size {
            if offset >= total {
                let _ = writer.flush();
                session.progress.is_complete.store(true, Ordering::Release);
                notify_progress(&session.progress);
                log_line(
                    &log,
                    format!(
                        "download_worker complete for token {} ({} bytes)",
                        session.token,
                        session.progress.downloaded.load(Ordering::Acquire)
                    ),
                );
                break;
            }
        }

        if let Err(e) = writer
            .seek(SeekFrom::Start(offset))
            .and_then(|_| writer.set_len(offset))
        {
            let msg = format!("file seek/truncate error: {e}");
            log_line(
                &log,
                format!("download_worker {} {msg}", session.token),
            );
            *session.progress.error.lock().unwrap() = Some(msg);
            notify_progress(&session.progress);
            break;
        }
        session.progress.downloaded.store(offset, Ordering::Release);

        let range_val = format!("bytes={offset}-");
        let req = apply_session_headers(
            client
                .get(&current_url)
                .header(reqwest::header::RANGE, &range_val),
            &session.headers,
        );

        let mut resp = match rt.block_on(async move { req.send().await }) {
            Ok(r) => r,
            Err(e) => {
                let err_msg = e.without_url().to_string();
                consecutive_failures += 1;
                log_line(
                    &log,
                    format!(
                        "download_worker {} attempt {} req error: {}",
                        session.token, consecutive_failures, err_msg
                    ),
                );
                if consecutive_failures >= MAX_CONSECUTIVE_FAILURES {
                    *session.progress.error.lock().unwrap() = Some(err_msg);
                    notify_progress(&session.progress);
                    break;
                }
                std::thread::sleep(Duration::from_millis(200 * u64::from(consecutive_failures)));
                continue;
            }
        };

        let status = resp.status();
        if status == reqwest::StatusCode::RANGE_NOT_SATISFIABLE {
            let actual_total = resp
                .headers()
                .get(reqwest::header::CONTENT_RANGE)
                .and_then(|v| v.to_str().ok())
                .and_then(parse_content_range)
                .and_then(|(_, _, t)| t);
            if actual_total == Some(offset) || (offset > 0 && total_size.is_none()) {
                total_size = Some(offset);
                *session.progress.total.lock().unwrap() = Some(offset);
                continue;
            }
            let err_msg = format!("upstream status {status}");
            log_line(
                &log,
                format!(
                    "download_worker {} 416 at offset {offset} (actual={actual_total:?})",
                    session.token
                ),
            );
            *session.progress.error.lock().unwrap() = Some(err_msg);
            notify_progress(&session.progress);
            break;
        }

        if status != reqwest::StatusCode::OK && status != reqwest::StatusCode::PARTIAL_CONTENT {
            let err_msg = format!("upstream status {status}");
            log_line(
                &log,
                format!(
                    "download_worker {} range {} {err_msg}",
                    session.token, range_val
                ),
            );
            let is_permanent_4xx = status.is_client_error()
                && status != reqwest::StatusCode::REQUEST_TIMEOUT
                && status != reqwest::StatusCode::TOO_MANY_REQUESTS;
            if is_permanent_4xx {
                *session.progress.error.lock().unwrap() = Some(err_msg);
                notify_progress(&session.progress);
                break;
            }
            consecutive_failures += 1;
            if consecutive_failures >= MAX_CONSECUTIVE_FAILURES {
                *session.progress.error.lock().unwrap() = Some(err_msg);
                notify_progress(&session.progress);
                break;
            }
            std::thread::sleep(Duration::from_millis(200 * u64::from(consecutive_failures)));
            continue;
        }

        let ct_header = match resp.headers().get(reqwest::header::CONTENT_TYPE) {
            None => None,
            Some(hv) => match hv.to_str() {
                Ok(s) => Some(s.to_string()),
                Err(_) => {
                    let err_msg = "upstream non-UTF-8 Content-Type".to_string();
                    log_line(&log, format!("download_worker {} {err_msg}", session.token));
                    *session.progress.error.lock().unwrap() = Some(err_msg);
                    notify_progress(&session.progress);
                    break;
                }
            },
        };

        if let Some(ref ct_str) = ct_header {
            let media_type = extract_media_type(ct_str);
            if media_type == "text/plain" {
                if offset > 0 {
                    let err_msg = format!(
                        "upstream returned non-media text/plain mid-stream at offset {offset}"
                    );
                    log_line(&log, format!("download_worker {} {err_msg}", session.token));
                    *session.progress.error.lock().unwrap() = Some(err_msg);
                    notify_progress(&session.progress);
                    break;
                }
                if alr_hops >= MAX_ALR_HOPS {
                    let err_msg = format!("too many ALR redirects ({alr_hops})");
                    log_line(&log, format!("download_worker {} {err_msg}", session.token));
                    *session.progress.error.lock().unwrap() = Some(err_msg);
                    notify_progress(&session.progress);
                    break;
                }
                if let Some(len) = resp.content_length() {
                    if len > MAX_ALR_BODY_BYTES as u64 {
                        let err_msg = format!(
                            "upstream text/plain body too large ({len} bytes)"
                        );
                        log_line(&log, format!("download_worker {} {err_msg}", session.token));
                        *session.progress.error.lock().unwrap() = Some(err_msg);
                        notify_progress(&session.progress);
                        break;
                    }
                }

                let mut alr_buf = Vec::new();
                let mut alr_read_err: Option<String> = None;
                loop {
                    if session.progress.is_cancelled.load(Ordering::Acquire) {
                        break;
                    }
                    match rt.block_on(async { resp.chunk().await }) {
                        Ok(None) => break,
                        Ok(Some(chunk)) => {
                            if alr_buf.len() + chunk.len() > MAX_ALR_BODY_BYTES {
                                alr_read_err = Some(format!(
                                    "upstream text/plain body exceeded {} bytes",
                                    MAX_ALR_BODY_BYTES
                                ));
                                break;
                            }
                            alr_buf.extend_from_slice(&chunk);
                        }
                        Err(e) => {
                            alr_read_err = Some(e.without_url().to_string());
                            break;
                        }
                    }
                }
                if session.progress.is_cancelled.load(Ordering::Acquire) {
                    break;
                }
                if let Some(err_msg) = alr_read_err {
                    log_line(
                        &log,
                        format!("download_worker {} ALR read error: {err_msg}", session.token),
                    );
                    *session.progress.error.lock().unwrap() = Some(err_msg);
                    notify_progress(&session.progress);
                    break;
                }
                if let Some(next_url) = parse_alr_redirect_url(&alr_buf, &current_url) {
                    alr_hops += 1;
                    consecutive_failures = 0;
                    current_url = next_url;
                    log_line(
                        &log,
                        format!(
                            "download_worker {} following ALR redirect hop {alr_hops}",
                            session.token
                        ),
                    );
                    continue;
                } else {
                    let err_msg = format!(
                        "upstream returned non-media text/plain ({} bytes)",
                        alr_buf.len()
                    );
                    log_line(&log, format!("download_worker {} {err_msg}", session.token));
                    *session.progress.error.lock().unwrap() = Some(err_msg);
                    notify_progress(&session.progress);
                    break;
                }
            } else if !is_acceptable_media_type(&media_type) {
                let err_msg = format!("upstream returned non-media Content-Type: {media_type}");
                log_line(&log, format!("download_worker {} {err_msg}", session.token));
                *session.progress.error.lock().unwrap() = Some(err_msg);
                notify_progress(&session.progress);
                break;
            }
        }

        let mut skip_bytes = if status == reqwest::StatusCode::PARTIAL_CONTENT {
            let cr_parsed = resp
                .headers()
                .get(reqwest::header::CONTENT_RANGE)
                .and_then(|v| v.to_str().ok())
                .and_then(parse_content_range);
            if let Some((start_opt, _, cr_total)) = cr_parsed {
                if let Some(t) = cr_total {
                    total_size = Some(t);
                    *session.progress.total.lock().unwrap() = Some(t);
                }
                match start_opt {
                    Some(s) if s > offset => {
                        consecutive_failures += 1;
                        let err_msg = format!("upstream range start {s} > offset {offset}");
                        log_line(&log, format!("download_worker {} {err_msg}", session.token));
                        if consecutive_failures >= MAX_CONSECUTIVE_FAILURES {
                            *session.progress.error.lock().unwrap() = Some(err_msg);
                            notify_progress(&session.progress);
                            break;
                        }
                        std::thread::sleep(Duration::from_millis(200 * u64::from(consecutive_failures)));
                        continue;
                    }
                    Some(s) => offset - s,
                    None if offset == 0 => {
                        if total_size.is_none() {
                            if let Some(len) = resp.content_length() {
                                total_size = Some(len);
                                *session.progress.total.lock().unwrap() = Some(len);
                            }
                        }
                        0
                    }
                    None => {
                        consecutive_failures += 1;
                        let err_msg = "malformed Content-Range on 206".to_string();
                        if consecutive_failures >= MAX_CONSECUTIVE_FAILURES {
                            *session.progress.error.lock().unwrap() = Some(err_msg);
                            notify_progress(&session.progress);
                            break;
                        }
                        std::thread::sleep(Duration::from_millis(200 * u64::from(consecutive_failures)));
                        continue;
                    }
                }
            } else if offset == 0 {
                if total_size.is_none() {
                    if let Some(len) = resp.content_length() {
                        total_size = Some(len);
                        *session.progress.total.lock().unwrap() = Some(len);
                    }
                }
                0
            } else {
                consecutive_failures += 1;
                let err_msg = "missing Content-Range on 206".to_string();
                if consecutive_failures >= MAX_CONSECUTIVE_FAILURES {
                    *session.progress.error.lock().unwrap() = Some(err_msg);
                    notify_progress(&session.progress);
                    break;
                }
                std::thread::sleep(Duration::from_millis(200 * u64::from(consecutive_failures)));
                continue;
            }
        } else {
            if total_size.is_none() {
                if let Some(len) = resp.content_length() {
                    total_size = Some(len);
                    *session.progress.total.lock().unwrap() = Some(len);
                }
            }
            offset
        };

        if offset == 0 {
            if let Some(ct_str) = ct_header {
                let media_type = extract_media_type(&ct_str);
                if media_type.starts_with("audio/") || media_type.starts_with("video/") {
                    *session.progress.mime.lock().unwrap() = ct_str;
                } else {
                    let mut mime_guard = session.progress.mime.lock().unwrap();
                    let cur_mt = extract_media_type(&mime_guard);
                    if !cur_mt.starts_with("audio/") && !cur_mt.starts_with("video/") {
                        *mime_guard = ct_str;
                    }
                }
            }
        }
        session
            .progress
            .headers_received
            .store(true, Ordering::Release);
        notify_progress(&session.progress);

        let mut bytes_written_this_attempt = 0u64;
        let mut fatal_error: Option<String> = None;

        loop {
            if session.progress.is_cancelled.load(Ordering::Acquire) {
                break;
            }

            match rt.block_on(async { resp.chunk().await }) {
                Ok(None) => {
                    let _ = writer.flush();
                    if total_size.is_none() && skip_bytes == 0 {
                        total_size = Some(offset);
                        *session.progress.total.lock().unwrap() = Some(offset);
                    } else if total_size.is_some_and(|t| offset >= t) {
                        // Top of outer loop completes cleanly
                    } else if bytes_written_this_attempt > 0 {
                        consecutive_failures = 0;
                        reconnects += 1;
                        log_line(
                            &log,
                            format!(
                                "download_worker {} premature EOF at {offset} (reconnect {reconnects})",
                                session.token
                            ),
                        );
                        if reconnects > MAX_RECONNECTS {
                            fatal_error = Some(format!("too many premature EOFs at {offset}"));
                        }
                    } else {
                        consecutive_failures += 1;
                        if consecutive_failures >= MAX_CONSECUTIVE_FAILURES {
                            fatal_error = Some("upstream closed with 0 bytes".to_string());
                        } else {
                            std::thread::sleep(Duration::from_millis(
                                200 * u64::from(consecutive_failures),
                            ));
                        }
                    }
                    break;
                }
                Ok(Some(chunk)) => {
                    let mut slice: &[u8] = &chunk;
                    if skip_bytes > 0 {
                        let to_skip = (skip_bytes as usize).min(slice.len());
                        slice = &slice[to_skip..];
                        skip_bytes -= to_skip as u64;
                    }
                    if slice.is_empty() {
                        continue;
                    }
                    if let Some(total) = total_size {
                        if offset >= total {
                            break;
                        }
                        let rem = (total - offset) as usize;
                        if slice.len() > rem {
                            slice = &slice[..rem];
                        }
                    }
                    if let Err(e) = writer.write_all(slice) {
                        let _ = writer.seek(SeekFrom::Start(offset));
                        let _ = writer.set_len(offset);
                        session.progress.downloaded.store(offset, Ordering::Release);
                        fatal_error = Some(format!("disk write error: {e}"));
                        break;
                    }
                    offset += slice.len() as u64;
                    bytes_written_this_attempt += slice.len() as u64;
                    consecutive_failures = 0;
                    session.progress.downloaded.store(offset, Ordering::Release);
                    notify_progress(&session.progress);
                    if let Some(total) = total_size {
                        if offset >= total {
                            break;
                        }
                    }
                }
                Err(e) => {
                    let err_msg = e.without_url().to_string();
                    let _ = writer.seek(SeekFrom::Start(offset));
                    let _ = writer.set_len(offset);
                    session.progress.downloaded.store(offset, Ordering::Release);

                    if bytes_written_this_attempt > 0 {
                        consecutive_failures = 0;
                        reconnects += 1;
                        log_line(
                            &log,
                            format!(
                                "download_worker {} read error at {offset} (reconnect {reconnects}): {err_msg}",
                                session.token
                            ),
                        );
                        if reconnects > MAX_RECONNECTS {
                            fatal_error = Some(err_msg);
                            break;
                        }
                    } else {
                        consecutive_failures += 1;
                        log_line(
                            &log,
                            format!(
                                "download_worker {} read error at {offset} (failure {consecutive_failures}): {err_msg}",
                                session.token
                            ),
                        );
                        if consecutive_failures >= MAX_CONSECUTIVE_FAILURES {
                            fatal_error = Some(err_msg);
                            break;
                        }
                    }
                    std::thread::sleep(Duration::from_millis(200));
                    break;
                }
            }
        }

        if let Some(err_msg) = fatal_error {
            log_line(
                &log,
                format!("download_worker failed for token {}: {err_msg}", session.token),
            );
            *session.progress.error.lock().unwrap() = Some(err_msg);
            notify_progress(&session.progress);
            break;
        }
    }
}

pub struct CacheRangeReader {
    file: std::fs::File,
    pos: u64,
    end: u64,
    progress: Arc<SharedProgress>,
    abort_on_cancel: bool,
    _reader_guard: Option<ActiveReaderGuard>,
    _file_guard: Option<Arc<TempFileGuard>>,
}

impl std::io::Read for CacheRangeReader {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        use std::io::ErrorKind;
        if out.is_empty() || self.pos > self.end {
            return Ok(0);
        }

        let mut zero_reads = 0u32;
        loop {
            if self.progress.is_cancelled.load(Ordering::Acquire) {
                let kind = if self.abort_on_cancel {
                    ErrorKind::ConnectionAborted
                } else {
                    ErrorKind::Interrupted
                };
                return Err(std::io::Error::new(kind, "stream cancelled"));
            }

            let downloaded = self.progress.downloaded.load(Ordering::Acquire);
            if self.pos < downloaded {
                let max_avail = (downloaded - self.pos).min((self.end - self.pos + 1) as u64);
                let to_read = (out.len() as u64).min(max_avail) as usize;

                self.file.seek(SeekFrom::Start(self.pos))?;
                let n = self.file.read(&mut out[..to_read])?;
                if n > 0 {
                    self.pos += n as u64;
                    return Ok(n);
                }
                if self.progress.is_complete.load(Ordering::Acquire)
                    || self.progress.error.lock().unwrap().is_some()
                {
                    return Ok(0);
                }
                zero_reads += 1;
                if zero_reads > 50 {
                    return Err(std::io::Error::new(
                        ErrorKind::UnexpectedEof,
                        "cache file EOF before downloaded",
                    ));
                }
                std::thread::sleep(Duration::from_millis(10));
                continue;
            }

            if self.progress.is_complete.load(Ordering::Acquire) {
                let downloaded = self.progress.downloaded.load(Ordering::Acquire);
                if self.pos >= downloaded {
                    return Ok(0);
                }
                continue;
            }

            if let Some(err) = self.progress.error.lock().unwrap().as_ref() {
                let downloaded = self.progress.downloaded.load(Ordering::Acquire);
                if self.pos >= downloaded {
                    return Err(std::io::Error::new(ErrorKind::Other, err.clone()));
                }
                continue;
            }

            let guard = self.progress.lock.lock().unwrap();
            if self.progress.is_cancelled.load(Ordering::Acquire)
                || self.pos < self.progress.downloaded.load(Ordering::Acquire)
                || self.progress.is_complete.load(Ordering::Acquire)
                || self.progress.error.lock().unwrap().is_some()
            {
                continue;
            }
            let _ = self
                .progress
                .notify
                .wait_timeout(guard, Duration::from_millis(200));
        }
    }
}

struct ParsedHttpRequest {
    method: String,
    target: String,
    range: Option<String>,
    keep_alive: bool,
}

fn parse_http_request(raw: &[u8]) -> Option<ParsedHttpRequest> {
    let text = std::str::from_utf8(raw).ok()?;
    let mut lines = text.split("\r\n");
    let first = lines.next()?;
    let mut parts = first.split_whitespace();
    let method = parts.next()?.to_string();
    let target = parts.next()?.to_string();
    let version = parts.next().unwrap_or("HTTP/1.1");
    if parts.next().is_some() {
        return None;
    }

    let mut range: Option<String> = None;
    let mut saw_close = false;
    let mut saw_keep_alive = false;
    let mut has_body = false;

    for line in lines {
        if line.is_empty() {
            continue;
        }
        let Some((k, v)) = line.split_once(':') else {
            return None;
        };
        let key = k.trim();
        let val = v.trim();
        if key.eq_ignore_ascii_case("range") {
            range = Some(val.to_string());
        } else if key.eq_ignore_ascii_case("connection") {
            for tok in val.split(',').map(|s| s.trim()) {
                if tok.eq_ignore_ascii_case("close") {
                    saw_close = true;
                } else if tok.eq_ignore_ascii_case("keep-alive") {
                    saw_keep_alive = true;
                }
            }
        } else if key.eq_ignore_ascii_case("content-length") {
            if val.is_empty() || !val.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            match val.parse::<u64>() {
                Ok(n) if n > 0 => has_body = true,
                Ok(_) => {}
                Err(_) => return None,
            }
        } else if key.eq_ignore_ascii_case("transfer-encoding") {
            has_body = true;
        }
    }

    let is_http_10 = version.eq_ignore_ascii_case("HTTP/1.0");
    let mut keep_alive = !is_http_10;
    if saw_close {
        keep_alive = false;
    } else if saw_keep_alive && is_http_10 {
        keep_alive = true;
    }
    if has_body {
        keep_alive = false;
    }

    Some(ParsedHttpRequest {
        method,
        target,
        range,
        keep_alive,
    })
}

async fn write_simple_response(
    stream: &mut tokio::net::TcpStream,
    status: u16,
    reason: &str,
    body: &str,
    extra_header: Option<(&str, &str)>,
    keep_alive: bool,
) -> bool {
    let conn = if keep_alive { "keep-alive" } else { "close" };
    let mut hdr = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: {conn}\r\n",
        body.len()
    );
    if let Some((k, v)) = extra_header {
        hdr.push_str(&format!("{k}: {v}\r\n"));
    }
    hdr.push_str("\r\n");

    if stream.write_all(hdr.as_bytes()).await.is_err() {
        return false;
    }
    if !body.is_empty() && stream.write_all(body.as_bytes()).await.is_err() {
        return false;
    }
    if stream.flush().await.is_err() {
        return false;
    }
    if !keep_alive {
        let _ = stream.shutdown().await;
    }
    keep_alive
}

async fn wait_peer_disconnect(stream: &tokio::net::TcpStream, has_pipelined: bool) {
    if has_pipelined {
        std::future::pending::<()>().await;
        return;
    }
    let mut tmp = [0u8; 1];
    loop {
        match stream.peek(&mut tmp).await {
            Ok(0) => return,
            Ok(_) => {
                std::future::pending::<()>().await;
                return;
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => continue,
            Err(_) => return,
        }
    }
}

async fn handle_connection(
    mut stream: tokio::net::TcpStream,
    registry: Arc<Mutex<SessionRegistry>>,
    client: reqwest::Client,
    runtime: Arc<ProxyRuntime>,
    log: Arc<Mutex<std::fs::File>>,
    mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
) {
    let mut buf: Vec<u8> = Vec::with_capacity(4096);
    let mut tmp = [0u8; 4096];

    loop {
        let hdr_end = loop {
            if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                break pos;
            }
            if buf.len() > MAX_HTTP_HEADER_BYTES {
                let _ = write_simple_response(
                    &mut stream,
                    400,
                    "Bad Request",
                    "headers too large",
                    None,
                    false,
                )
                .await;
                return;
            }
            let read_res = tokio::select! {
                _ = wait_shutdown(&mut shutdown_rx) => {
                    let _ = stream.shutdown().await;
                    return;
                }
                res = tokio::time::timeout(IDLE_KEEPALIVE_TIMEOUT, stream.read(&mut tmp)) => {
                    match res {
                        Ok(r) => r,
                        Err(_) => {
                            let _ = stream.shutdown().await;
                            return;
                        }
                    }
                }
            };
            match read_res {
                Ok(0) => return,
                Ok(n) => buf.extend_from_slice(&tmp[..n]),
                Err(_) => return,
            }
        };

        let raw_hdr = buf[..hdr_end].to_vec();
        buf.drain(..hdr_end + 4);

        let Some(req) = parse_http_request(&raw_hdr) else {
            let _ = write_simple_response(
                &mut stream,
                400,
                "Bad Request",
                "bad request",
                None,
                false,
            )
            .await;
            return;
        };

        if req.method != "GET" && req.method != "HEAD" {
            let _ = write_simple_response(
                &mut stream,
                405,
                "Method Not Allowed",
                "method not allowed",
                None,
                false,
            )
            .await;
            return;
        }

        let raw_path = if req.target.starts_with("http://") || req.target.starts_with("https://") {
            req.target
                .find("://")
                .and_then(|i| req.target[i + 3..].find('/').map(|j| &req.target[i + 3 + j..]))
                .unwrap_or("/")
        } else {
            req.target.as_str()
        };
        let path = raw_path.split(['?', '#']).next().unwrap_or("");
        let Some(token) = path.strip_prefix("/s/") else {
            log_line(&log, format!("404 {}", req.target));
            if !write_simple_response(&mut stream, 404, "Not Found", "not found", None, req.keep_alive).await {
                return;
            }
            continue;
        };

        let lookup = {
            let mut reg = registry.lock().unwrap();
            let session = reg.sessions.get(token).cloned();
            match session {
                Some(s) if s.progress.is_cancelled.load(Ordering::Acquire) => {
                    Some(Err("404 cancelled"))
                }
                Some(s) => {
                    let reader_guard = ActiveReaderGuard::new(&s.active_readers);

                    reg.downloaded_lru.retain(|t| t != token);
                    reg.downloaded_lru.push_back(token.to_string());

                    let should_spawn = s
                        .started
                        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                        .is_ok();

                    let mut evicted_to_drop = Vec::new();
                    while reg.downloaded_lru.len() > MAX_DOWNLOADED_LRU {
                        if let Some(evicted_token) = reg.downloaded_lru.pop_front() {
                            reg.token_order.retain(|t| t != &evicted_token);
                            if let Some(evicted_session) = reg.sessions.remove(&evicted_token) {
                                evicted_session
                                    .progress
                                    .is_cancelled
                                    .store(true, Ordering::Release);
                                evicted_session
                                    .file_guard
                                    .delete_on_drop
                                    .store(true, Ordering::Release);
                                notify_progress(&evicted_session.progress);
                                evicted_to_drop.push(evicted_session);
                            }
                        }
                    }

                    Some(Ok((s, reader_guard, should_spawn, evicted_to_drop)))
                }
                None => None,
            }
        };

        let (session, reader_guard, should_spawn, evicted_to_drop) = match lookup {
            Some(Ok(v)) => v,
            Some(Err(_)) => {
                log_line(&log, format!("404 cancelled {}", req.target));
                if !write_simple_response(&mut stream, 404, "Not Found", "not found", None, req.keep_alive).await {
                    return;
                }
                continue;
            }
            None => {
                log_line(&log, format!("404 {}", req.target));
                if !write_simple_response(&mut stream, 404, "Not Found", "not found", None, req.keep_alive).await {
                    return;
                }
                continue;
            }
        };
        drop(evicted_to_drop);

        if should_spawn {
            let s = session.clone();
            let c = client.clone();
            let r = runtime.clone();
            let l = log.clone();
            std::thread::spawn(move || {
                download_worker(s, c, r, l);
            });
        }

        let head = req.method == "HEAD";
        let mut progress_rx = session.progress.async_tx.subscribe();
        let wait_timeout = if session
            .url
            .get(..8)
            .is_some_and(|p| p.eq_ignore_ascii_case("https://"))
        {
            TLS_METADATA_WAIT_TIMEOUT
        } else {
            METADATA_WAIT_TIMEOUT
        };
        let deadline = tokio::time::Instant::now() + wait_timeout;
        loop {
            progress_rx.mark_unchanged();
            let total_opt = *session.progress.total.lock().unwrap();
            let downloaded = session.progress.downloaded.load(Ordering::Acquire);
            let headers_ready = session.progress.headers_received.load(Ordering::Acquire);
            let done = session.progress.is_complete.load(Ordering::Acquire);
            let has_err = session.progress.error.lock().unwrap().is_some();
            let cancelled = session.progress.is_cancelled.load(Ordering::Acquire);
            let meta_ready = if let Some(t) = total_opt {
                if head {
                    headers_ready || downloaded > 0 || done
                } else if done {
                    true
                } else {
                    let (req_start, _) = parse_range(req.range.as_deref(), Some(t));
                    (downloaded > 0 || done) && (req_start >= t || downloaded > req_start)
                }
            } else {
                false
            };
            if meta_ready || done || has_err || cancelled {
                break;
            }
            let now = tokio::time::Instant::now();
            if now >= deadline {
                break;
            }
            let remaining = deadline - now;
            tokio::select! {
                _ = progress_rx.changed() => {}
                _ = tokio::time::sleep(remaining) => break,
                _ = wait_peer_disconnect(&stream, !buf.is_empty()) => {
                    return;
                }
                _ = wait_shutdown(&mut shutdown_rx) => {
                    let _ = stream.shutdown().await;
                    return;
                }
            }
        }

        if session.progress.is_cancelled.load(Ordering::Acquire) {
            drop(reader_guard);
            log_line(&log, format!("503 cancelled for {}", req.target));
            if !write_simple_response(
                &mut stream,
                503,
                "Service Unavailable",
                "stream cancelled",
                None,
                req.keep_alive,
            )
            .await
            {
                return;
            }
            continue;
        }

        let err_opt = session.progress.error.lock().unwrap().clone();
        if let Some(err) = err_opt {
            drop(reader_guard);
            log_line(&log, format!("502 error for {}: {err}", req.target));
            if !write_simple_response(
                &mut stream,
                502,
                "Bad Gateway",
                &format!("upstream error: {err}"),
                None,
                req.keep_alive,
            )
            .await
            {
                return;
            }
            continue;
        }

        let total = *session.progress.total.lock().unwrap();
        let downloaded = session.progress.downloaded.load(Ordering::Acquire);
        let headers_ready = session.progress.headers_received.load(Ordering::Acquire);
        let done = session.progress.is_complete.load(Ordering::Acquire);
        let Some(total) = total else {
            drop(reader_guard);
            log_line(
                &log,
                format!("504 timeout waiting for stream data for {}", req.target),
            );
            if !write_simple_response(
                &mut stream,
                504,
                "Gateway Timeout",
                "stream timeout",
                None,
                req.keep_alive,
            )
            .await
            {
                return;
            }
            continue;
        };
        if !done && downloaded == 0 && !(head && headers_ready) {
            drop(reader_guard);
            log_line(
                &log,
                format!("504 timeout waiting for stream data for {}", req.target),
            );
            if !write_simple_response(
                &mut stream,
                504,
                "Gateway Timeout",
                "stream timeout",
                None,
                req.keep_alive,
            )
            .await
            {
                return;
            }
            continue;
        }

        let Ok((start, end, is_partial)) = parse_range_checked(req.range.as_deref(), total) else {
            drop(reader_guard);
            let cr_hdr = format!("bytes */{total}");
            if !write_simple_response(
                &mut stream,
                416,
                "Range Not Satisfiable",
                "",
                Some(("Content-Range", &cr_hdr)),
                req.keep_alive,
            )
            .await
            {
                return;
            }
            continue;
        };

        let safe_mime = sanitize_content_type(&session.progress.mime.lock().unwrap());
        let len = if total == 0 && !is_partial {
            0u64
        } else {
            end - start + 1
        };

        let mut file_opt = None;
        if !head && len > 0 {
            for _ in 0..20 {
                if session.progress.is_cancelled.load(Ordering::Acquire)
                    || session.progress.error.lock().unwrap().is_some()
                {
                    break;
                }
                if let Ok(f) = open_shared_read(&session.file_path) {
                    file_opt = Some(f);
                    break;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }

            if file_opt.is_none() {
                drop(reader_guard);
                if session.progress.is_cancelled.load(Ordering::Acquire) {
                    log_line(&log, format!("503 cancelled for {}", req.target));
                    if !write_simple_response(
                        &mut stream,
                        503,
                        "Service Unavailable",
                        "stream cancelled",
                        None,
                        req.keep_alive,
                    )
                    .await
                    {
                        return;
                    }
                    continue;
                }
                let err_opt = session.progress.error.lock().unwrap().clone();
                if let Some(err) = err_opt {
                    log_line(&log, format!("502 error for {}: {err}", req.target));
                    if !write_simple_response(
                        &mut stream,
                        502,
                        "Bad Gateway",
                        &format!("upstream error: {err}"),
                        None,
                        req.keep_alive,
                    )
                    .await
                    {
                        return;
                    }
                    continue;
                }
                log_line(
                    &log,
                    format!("500 file open error {}", session.file_path.display()),
                );
                if !write_simple_response(
                    &mut stream,
                    500,
                    "Internal Server Error",
                    "file error",
                    None,
                    req.keep_alive,
                )
                .await
                {
                    return;
                }
                continue;
            }
        }

        let (status_code, reason) = if is_partial {
            (206u16, "Partial Content")
        } else {
            (200u16, "OK")
        };

        log_line(
            &log,
            format!(
                "{} {} status={} mime={safe_mime} client={:?} serve={start}-{end}/{total}",
                req.method, req.target, status_code, req.range
            ),
        );

        let conn_hdr = if req.keep_alive {
            "keep-alive"
        } else {
            "close"
        };
        let mut resp_hdr = format!(
            "HTTP/1.1 {status_code} {reason}\r\nContent-Type: {safe_mime}\r\nAccept-Ranges: bytes\r\nContent-Length: {len}\r\nConnection: {conn_hdr}\r\n"
        );
        if is_partial {
            resp_hdr.push_str(&format!("Content-Range: bytes {start}-{end}/{total}\r\n"));
        }
        resp_hdr.push_str("\r\n");

        if stream.write_all(resp_hdr.as_bytes()).await.is_err() {
            return;
        }
        if head || len == 0 {
            drop(reader_guard);
            if stream.flush().await.is_err() {
                return;
            }
            if !req.keep_alive {
                let _ = stream.shutdown().await;
                return;
            }
            continue;
        }

        if stream.flush().await.is_err() {
            return;
        }

        let mut file = file_opt.unwrap();
        let file_guard = session.file_guard.clone();
        let mut pos = start;
        let mut read_buf = vec![0u8; 64 * 1024];
        let mut zero_reads = 0u32;

        while pos <= end {
            if session.progress.is_cancelled.load(Ordering::Acquire) {
                drop(file);
                drop(reader_guard);
                drop(file_guard);
                let _ = stream.shutdown().await;
                return;
            }

            let downloaded = session.progress.downloaded.load(Ordering::Acquire);
            if pos < downloaded {
                let max_avail = (downloaded - pos).min(end - pos + 1);
                let to_read = (read_buf.len() as u64).min(max_avail) as usize;
                let read_res = file
                    .seek(SeekFrom::Start(pos))
                    .and_then(|_| file.read(&mut read_buf[..to_read]));
                match read_res {
                    Ok(0) => {
                        if session.progress.is_cancelled.load(Ordering::Acquire)
                            || session.progress.is_complete.load(Ordering::Acquire)
                            || session.progress.error.lock().unwrap().is_some()
                        {
                            drop(file);
                            drop(reader_guard);
                            drop(file_guard);
                            let _ = stream.shutdown().await;
                            return;
                        }
                        zero_reads += 1;
                        if zero_reads > 50 {
                            log_line(
                                &log,
                                format!(
                                    "file EOF before downloaded ({pos}/{downloaded}) for {}",
                                    req.target
                                ),
                            );
                            drop(file);
                            drop(reader_guard);
                            drop(file_guard);
                            let _ = stream.shutdown().await;
                            return;
                        }
                        tokio::time::sleep(Duration::from_millis(10)).await;
                        continue;
                    }
                    Ok(n) => {
                        zero_reads = 0;
                        if stream.write_all(&read_buf[..n]).await.is_err() {
                            return;
                        }
                        pos += n as u64;
                        continue;
                    }
                    Err(_) => {
                        drop(file);
                        drop(reader_guard);
                        drop(file_guard);
                        let _ = stream.shutdown().await;
                        return;
                    }
                }
            }

            if session.progress.is_complete.load(Ordering::Acquire) {
                let downloaded = session.progress.downloaded.load(Ordering::Acquire);
                if pos < downloaded {
                    continue;
                }
                drop(file);
                drop(reader_guard);
                drop(file_guard);
                let _ = stream.shutdown().await;
                return;
            }

            if session.progress.error.lock().unwrap().is_some() {
                let downloaded = session.progress.downloaded.load(Ordering::Acquire);
                if pos < downloaded {
                    continue;
                }
                drop(file);
                drop(reader_guard);
                drop(file_guard);
                let _ = stream.shutdown().await;
                return;
            }

            progress_rx.mark_unchanged();
            if session.progress.is_cancelled.load(Ordering::Acquire)
                || pos < session.progress.downloaded.load(Ordering::Acquire)
                || session.progress.is_complete.load(Ordering::Acquire)
                || session.progress.error.lock().unwrap().is_some()
            {
                continue;
            }

            tokio::select! {
                _ = progress_rx.changed() => {}
                _ = wait_peer_disconnect(&stream, !buf.is_empty()) => {
                    return;
                }
                _ = wait_shutdown(&mut shutdown_rx) => {
                    let _ = stream.shutdown().await;
                    return;
                }
            }
        }

        drop(file);
        drop(reader_guard);
        drop(file_guard);
        if stream.flush().await.is_err() {
            return;
        }
        if !req.keep_alive {
            let _ = stream.shutdown().await;
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use std::net::TcpListener;
    use std::time::Instant;

    #[test]
    fn parse_range_cases() {
        assert_eq!(parse_range(None, Some(1000)), (0, None));
        assert_eq!(parse_range(Some("bytes=0-499"), Some(1000)), (0, Some(499)));
        assert_eq!(parse_range(Some("bytes=500-"), Some(1000)), (500, None));
        assert_eq!(
            parse_range(Some("bytes=-200"), Some(1000)),
            (800, Some(999))
        );
        assert_eq!(parse_range(Some("bytes=0-1"), Some(1000)), (0, Some(1)));
        assert_eq!(parse_range(Some("bytes=0-0"), Some(1000)), (0, Some(0)));
        assert_eq!(parse_range(Some("bytes=-0"), Some(1000)), (1000, Some(999)));
    }

    #[test]
    fn cache_range_reader_reads_across_chunks() {
        let temp_dir = std::env::temp_dir();
        let file_path = temp_dir.join(format!("test_cache_{}.tmp", std::process::id()));
        let mut file = std::fs::File::create(&file_path).unwrap();
        file.write_all(b"hello world streaming data").unwrap();
        file.flush().unwrap();

        let progress = Arc::new(SharedProgress::new());
        progress.downloaded.store(26, Ordering::Release);
        progress.is_complete.store(true, Ordering::Release);
        *progress.total.lock().unwrap() = Some(26);

        let read_file = std::fs::File::open(&file_path).unwrap();
        let mut reader = CacheRangeReader {
            file: read_file,
            pos: 6,
            end: 16,
            progress,
            abort_on_cancel: false,
            _reader_guard: None,
            _file_guard: None,
        };

        let mut buf = vec![0u8; 11];
        reader.read_exact(&mut buf).unwrap();
        assert_eq!(&buf, b"world strea");

        let _ = std::fs::remove_file(&file_path);
    }

    #[test]
    fn cache_range_reader_cancels_cleanly() {
        let temp_dir = std::env::temp_dir();
        let file_path = temp_dir.join(format!("test_cancel_{}.tmp", std::process::id()));
        let _ = std::fs::File::create(&file_path).unwrap();

        let progress = Arc::new(SharedProgress::new());
        progress.is_cancelled.store(true, Ordering::Release);

        let read_file = std::fs::File::open(&file_path).unwrap();
        let mut reader = CacheRangeReader {
            file: read_file,
            pos: 0,
            end: 100,
            progress,
            abort_on_cancel: false,
            _reader_guard: None,
            _file_guard: None,
        };

        let mut buf = [0u8; 10];
        let res = reader.read(&mut buf);
        assert!(res.is_err());
        assert_eq!(res.unwrap_err().kind(), std::io::ErrorKind::Interrupted);

        let _ = std::fs::remove_file(&file_path);
    }

    #[derive(Clone)]
    struct RecordedRequest {
        method: String,
        path: String,
        headers: HashMap<String, String>,
    }

    #[derive(Clone)]
    struct MockRoute {
        payload: Arc<Vec<u8>>,
        mime: String,
        status_override: Option<u16>,
        drop_after_bytes: Option<usize>,
        drop_only_first: bool,
        force_200_ignore_range: bool,
        omit_content_length_on_200: bool,
        trickle_chunk: usize,
        trickle_delay: Duration,
        stall_headers: Option<Duration>,
    }

    impl MockRoute {
        fn new(payload: Arc<Vec<u8>>, mime: &str) -> Self {
            Self {
                payload,
                mime: mime.to_string(),
                status_override: None,
                drop_after_bytes: None,
                drop_only_first: false,
                force_200_ignore_range: false,
                omit_content_length_on_200: false,
                trickle_chunk: 64 * 1024,
                trickle_delay: Duration::ZERO,
                stall_headers: None,
            }
        }
    }

    struct MockUpstream {
        port: u16,
        routes: Arc<Mutex<HashMap<String, MockRoute>>>,
        recorded: Arc<Mutex<Vec<RecordedRequest>>>,
        stop: Arc<AtomicBool>,
    }

    impl MockUpstream {
        fn start() -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            listener.set_nonblocking(true).unwrap();
            let port = listener.local_addr().unwrap().port();
            let routes: Arc<Mutex<HashMap<String, MockRoute>>> =
                Arc::new(Mutex::new(HashMap::new()));
            let hits: Arc<Mutex<HashMap<String, usize>>> = Arc::new(Mutex::new(HashMap::new()));
            let recorded: Arc<Mutex<Vec<RecordedRequest>>> = Arc::new(Mutex::new(Vec::new()));
            let stop = Arc::new(AtomicBool::new(false));

            let r = routes.clone();
            let h = hits.clone();
            let rec = recorded.clone();
            let s = stop.clone();

            std::thread::spawn(move || {
                while !s.load(Ordering::Relaxed) {
                    match listener.accept() {
                        Ok((mut stream, _)) => {
                            let r = r.clone();
                            let h = h.clone();
                            let rec = rec.clone();
                            std::thread::spawn(move || {
                                let _ = stream.set_nonblocking(false);
                                let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
                                let mut buf = Vec::new();
                                let mut tmp = [0u8; 4096];
                                loop {
                                    match stream.read(&mut tmp) {
                                        Ok(0) => return,
                                        Ok(n) => {
                                            buf.extend_from_slice(&tmp[..n]);
                                            if buf.windows(4).any(|w| w == b"\r\n\r\n") {
                                                break;
                                            }
                                        }
                                        Err(_) => return,
                                    }
                                }
                                let text = String::from_utf8_lossy(&buf);
                                let mut lines = text.split("\r\n");
                                let first = lines.next().unwrap_or("");
                                let mut parts = first.split_whitespace();
                                let method = parts.next().unwrap_or("GET").to_string();
                                let path = parts.next().unwrap_or("/").to_string();
                                let mut hdrs = HashMap::new();
                                for line in lines {
                                    if line.is_empty() {
                                        break;
                                    }
                                    if let Some((k, v)) = line.split_once(':') {
                                        hdrs.insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
                                    }
                                }
                                rec.lock().unwrap().push(RecordedRequest {
                                    method: method.clone(),
                                    path: path.clone(),
                                    headers: hdrs.clone(),
                                });

                                let route = r.lock().unwrap().get(&path).cloned();
                                let Some(route) = route else {
                                    let _ = stream.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
                                    return;
                                };

                                let hit = {
                                    let mut map = h.lock().unwrap();
                                    let c = map.entry(path.clone()).or_insert(0);
                                    *c += 1;
                                    *c
                                };

                                if let Some(stall) = route.stall_headers {
                                    std::thread::sleep(stall);
                                }

                                if let Some(st) = route.status_override {
                                    let resp = format!("HTTP/1.1 {st} Err\r\nContent-Length: 3\r\nConnection: close\r\n\r\nerr");
                                    let _ = stream.write_all(resp.as_bytes());
                                    return;
                                }

                                let total = route.payload.len();
                                let range = hdrs.get("range").map(|s| s.as_str());
                                let (start, end, is_partial) = if route.force_200_ignore_range {
                                    (0, total.saturating_sub(1), false)
                                } else if let Some(rv) = range {
                                    let (s, e_opt) = parse_range(Some(rv), Some(total as u64));
                                    let e = e_opt.unwrap_or((total as u64).saturating_sub(1)).min((total as u64).saturating_sub(1));
                                    (s as usize, e as usize, true)
                                } else {
                                    (0, total.saturating_sub(1), false)
                                };

                                if total == 0 || start > end || start >= total {
                                    let resp = format!("HTTP/1.1 416 Range Not Satisfiable\r\nContent-Range: bytes */{total}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
                                    let _ = stream.write_all(resp.as_bytes());
                                    return;
                                }

                                let slice = &route.payload[start..=end];
                                let header_str = if is_partial {
                                    format!(
                                        "HTTP/1.1 206 Partial Content\r\nContent-Type: {}\r\nContent-Range: bytes {}-{}/{}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                                        route.mime, start, end, total, slice.len()
                                    )
                                } else if route.omit_content_length_on_200 {
                                    format!(
                                        "HTTP/1.1 200 OK\r\nContent-Type: {}\r\nConnection: close\r\n\r\n",
                                        route.mime
                                    )
                                } else {
                                    format!(
                                        "HTTP/1.1 200 OK\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                                        route.mime, slice.len()
                                    )
                                };
                                if stream.write_all(header_str.as_bytes()).is_err() {
                                    return;
                                }
                                if method.eq_ignore_ascii_case("HEAD") {
                                    return;
                                }
                                let drop_at = if route.drop_only_first && hit > 1 {
                                    None
                                } else {
                                    route.drop_after_bytes
                                };
                                let mut pos = 0usize;
                                while pos < slice.len() {
                                    let abs = start + pos;
                                    if let Some(d) = drop_at {
                                        if abs >= d {
                                            return;
                                        }
                                    }
                                    let mut next = (pos + route.trickle_chunk).min(slice.len());
                                    if let Some(d) = drop_at {
                                        if start + next > d && d > abs {
                                            next = d - start;
                                        }
                                    }
                                    if stream.write_all(&slice[pos..next]).is_err() {
                                        return;
                                    }
                                    let _ = stream.flush();
                                    pos = next;
                                    if !route.trickle_delay.is_zero() && pos < slice.len() {
                                        std::thread::sleep(route.trickle_delay);
                                    }
                                }
                            });
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(Duration::from_millis(5));
                        }
                        Err(_) => break,
                    }
                }
            });

            Self {
                port,
                routes,
                recorded,
                stop,
            }
        }

        fn set(&self, path: &str, route: MockRoute) {
            self.routes.lock().unwrap().insert(path.to_string(), route);
        }

        fn url(&self, path: &str) -> String {
            format!("http://127.0.0.1:{}{}", self.port, path)
        }

        fn requests_for(&self, path: &str) -> Vec<RecordedRequest> {
            self.recorded
                .lock()
                .unwrap()
                .iter()
                .filter(|r| r.path == path)
                .cloned()
                .collect()
        }
    }

    impl Drop for MockUpstream {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Relaxed);
        }
    }

    struct TempEnv(PathBuf);
    impl TempEnv {
        fn new(name: &str) -> Self {
            static SEQ: AtomicUsize = AtomicUsize::new(0);
            let n = SEQ.fetch_add(1, Ordering::Relaxed);
            let dir = std::env::temp_dir().join(format!(
                "nada-proxy-unit-{name}-{}-{n}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(dir.join("cache")).unwrap();
            Self(dir)
        }
        fn proxy(&self) -> StreamProxy {
            StreamProxy::start(self.0.join("proxy.log"), self.0.join("cache")).unwrap()
        }
        fn cache_dir(&self) -> PathBuf {
            self.0.join("cache")
        }
    }
    impl Drop for TempEnv {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn make_payload(len: usize) -> Arc<Vec<u8>> {
        Arc::new((0..len).map(|i| (i % 251) as u8).collect())
    }

    fn test_http() -> reqwest::blocking::Client {
        reqwest::blocking::Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(30))
            .build()
            .unwrap()
    }

    #[test]
    fn streams_over_1mib_unsegmented() {
        let up = MockUpstream::start();
        let data = make_payload(1_500_000);
        up.set("/big", MockRoute::new(data.clone(), "audio/mp4"));

        let env = TempEnv::new("unseg");
        let proxy = env.proxy();
        let pu = proxy.register(&up.url("/big"), None, None);

        let resp = test_http().get(&pu).send().unwrap();
        assert_eq!(resp.status(), reqwest::StatusCode::OK);
        assert_eq!(
            resp.headers().get("content-length").and_then(|v| v.to_str().ok()),
            Some("1500000")
        );
        assert_eq!(
            resp.headers().get("accept-ranges").and_then(|v| v.to_str().ok()),
            Some("bytes")
        );
        let body = resp.bytes().unwrap();
        assert_eq!(&body[..], &data[..]);

        let reqs = up.requests_for("/big");
        assert_eq!(reqs.len(), 1);
        assert_eq!(reqs[0].method, "GET");
        assert_eq!(reqs[0].headers.get("range").map(String::as_str), Some("bytes=0-"));
    }

    #[test]
    fn head_and_range_probe_0_1_and_open_ended() {
        let up = MockUpstream::start();
        let data = make_payload(1_500_000);
        up.set("/probe", MockRoute::new(data.clone(), "audio/webm"));

        let env = TempEnv::new("probe");
        let proxy = env.proxy();
        let pu = proxy.register(&up.url("/probe"), None, None);

        let h = test_http().head(&pu).send().unwrap();
        assert_eq!(h.status(), reqwest::StatusCode::OK);
        assert_eq!(
            h.headers().get("content-length").and_then(|v| v.to_str().ok()),
            Some("1500000")
        );
        assert_eq!(
            h.headers().get("accept-ranges").and_then(|v| v.to_str().ok()),
            Some("bytes")
        );
        assert!(h.bytes().unwrap().is_empty());

        let r01 = test_http()
            .get(&pu)
            .header("Range", "bytes=0-1")
            .send()
            .unwrap();
        assert_eq!(r01.status(), reqwest::StatusCode::PARTIAL_CONTENT);
        assert_eq!(
            r01.headers().get("content-range").and_then(|v| v.to_str().ok()),
            Some("bytes 0-1/1500000")
        );
        assert_eq!(
            r01.headers().get("content-length").and_then(|v| v.to_str().ok()),
            Some("2")
        );
        assert_eq!(&r01.bytes().unwrap()[..], &data[..2]);

        let rfull = test_http()
            .get(&pu)
            .header("Range", "bytes=0-")
            .send()
            .unwrap();
        assert_eq!(rfull.status(), reqwest::StatusCode::PARTIAL_CONTENT);
        assert_eq!(
            rfull.headers().get("content-range").and_then(|v| v.to_str().ok()),
            Some("bytes 0-1499999/1500000")
        );
        assert_eq!(&rfull.bytes().unwrap()[..], &data[..]);
    }

    #[test]
    fn forward_and_backward_seeks_from_cache() {
        let up = MockUpstream::start();
        let data = make_payload(1_200_000);
        let mut route = MockRoute::new(data.clone(), "video/mp4");
        route.trickle_chunk = 32 * 1024;
        route.trickle_delay = Duration::from_millis(5);
        up.set("/seek", route);

        let env = TempEnv::new("seek");
        let proxy = env.proxy();
        let pu = proxy.register(&up.url("/seek"), Some(1_200_000), Some("video/mp4"));

        let fwd = test_http()
            .get(&pu)
            .header("Range", "bytes=900000-1049999")
            .send()
            .unwrap();
        assert_eq!(fwd.status(), reqwest::StatusCode::PARTIAL_CONTENT);
        assert_eq!(&fwd.bytes().unwrap()[..], &data[900000..=1049999]);

        let back = test_http()
            .get(&pu)
            .header("Range", "bytes=128-65535")
            .send()
            .unwrap();
        assert_eq!(back.status(), reqwest::StatusCode::PARTIAL_CONTENT);
        assert_eq!(&back.bytes().unwrap()[..], &data[128..=65535]);

        let oob = test_http()
            .get(&pu)
            .header("Range", "bytes=1200000-")
            .send()
            .unwrap();
        assert_eq!(oob.status(), reqwest::StatusCode::RANGE_NOT_SATISFIABLE);
        assert_eq!(up.requests_for("/seek").len(), 1);
    }

    #[test]
    fn transient_midstream_drop_resumes_without_corruption() {
        let up = MockUpstream::start();
        let data = make_payload(1_200_000);

        // Case 1: 206 resume from offset
        let mut r1 = MockRoute::new(data.clone(), "audio/mp4");
        r1.drop_after_bytes = Some(400_000);
        r1.drop_only_first = true;
        up.set("/drop206", r1);

        // Case 2: 200 OK full-restart fallback on reconnect (skip_bytes = 400_000)
        let mut r2 = MockRoute::new(data.clone(), "audio/mp4");
        r2.drop_after_bytes = Some(400_000);
        r2.drop_only_first = true;
        r2.force_200_ignore_range = true;
        up.set("/drop200", r2);

        let env = TempEnv::new("drop");
        let proxy = env.proxy();

        let pu1 = proxy.register(&up.url("/drop206"), Some(1_200_000), Some("audio/mp4"));
        let b1 = test_http().get(&pu1).send().unwrap().bytes().unwrap();
        assert_eq!(&b1[..], &data[..]);
        let reqs1 = up.requests_for("/drop206");
        assert_eq!(reqs1.len(), 2);
        assert_eq!(
            reqs1[1].headers.get("range").map(String::as_str),
            Some("bytes=400000-")
        );

        let pu2 = proxy.register(&up.url("/drop200"), Some(1_200_000), Some("audio/mp4"));
        let b2 = test_http().get(&pu2).send().unwrap().bytes().unwrap();
        assert_eq!(&b2[..], &data[..]);
    }

    #[test]
    fn query_string_and_custom_headers_forwarded() {
        let up = MockUpstream::start();
        let data = make_payload(64 * 1024);
        up.set("/hdrs", MockRoute::new(data.clone(), "audio/mp4"));

        let env = TempEnv::new("hdrs");
        let proxy = env.proxy();
        let pu = proxy.register_with_headers(
            &up.url("/hdrs"),
            Some(data.len() as u64),
            Some("audio/mp4"),
            StreamRegisterHeaders {
                user_agent: Some("NadaCustomUA/1.0".to_string()),
                origin: Some("https://music.youtube.com".to_string()),
                referer: Some("https://music.youtube.com/".to_string()),
            },
        );

        let h = test_http().head(format!("{pu}?probe=1")).send().unwrap();
        assert_eq!(h.status(), reqwest::StatusCode::OK);

        let g = test_http().get(format!("{pu}?t=1&cb=99")).send().unwrap();
        assert_eq!(g.status(), reqwest::StatusCode::OK);
        assert_eq!(&g.bytes().unwrap()[..], &data[..]);

        let reqs = up.requests_for("/hdrs");
        assert_eq!(reqs.len(), 1);
        assert_eq!(reqs[0].headers.get("user-agent").map(String::as_str), Some("NadaCustomUA/1.0"));
        assert_eq!(reqs[0].headers.get("origin").map(String::as_str), Some("https://music.youtube.com"));
        assert_eq!(reqs[0].headers.get("referer").map(String::as_str), Some("https://music.youtube.com/"));
    }

    #[test]
    fn lru_eviction_removes_from_sessions_and_allows_reregister() {
        let up = MockUpstream::start();
        let data = make_payload(32 * 1024);
        for i in 1..=11 {
            up.set(&format!("/lru/{i}"), MockRoute::new(data.clone(), "audio/mp4"));
        }

        let env = TempEnv::new("lru");
        let proxy = env.proxy();
        let u1 = up.url("/lru/1");
        let p1 = proxy.register(&u1, Some(data.len() as u64), Some("audio/mp4"));
        assert_eq!(&test_http().get(&p1).send().unwrap().bytes().unwrap()[..], &data[..]);

        for i in 2..=11 {
            let pi = proxy.register(&up.url(&format!("/lru/{i}")), Some(data.len() as u64), Some("audio/mp4"));
            assert_eq!(&test_http().get(&pi).send().unwrap().bytes().unwrap()[..], &data[..]);
        }

        // u1 must have been removed from sessions too (returns 404)
        let evicted_resp = test_http().get(&p1).send().unwrap();
        assert_eq!(evicted_resp.status(), reqwest::StatusCode::NOT_FOUND);

        // Re-register u1 cleanly creates a new {token}_{seq}.tmp and succeeds
        let p1_new = proxy.register(&u1, Some(data.len() as u64), Some("audio/mp4"));
        let r_new = test_http().get(&p1_new).send().unwrap();
        assert_eq!(r_new.status(), reqwest::StatusCode::OK);
        assert_eq!(&r_new.bytes().unwrap()[..], &data[..]);
    }

    #[test]
    fn register_never_reuses_errored_or_cancelled_session() {
        let up = MockUpstream::start();
        let data = make_payload(32 * 1024);
        let mut bad = MockRoute::new(data.clone(), "audio/mp4");
        bad.status_override = Some(403);
        let raw_url = up.url("/err");
        up.set("/err", bad);

        let env = TempEnv::new("err");
        let proxy = env.proxy();
        let p1 = proxy.register(&raw_url, Some(data.len() as u64), Some("audio/mp4"));
        let r1 = test_http().get(&p1).send().unwrap();
        assert_eq!(r1.status(), reqwest::StatusCode::BAD_GATEWAY);

        up.set("/err", MockRoute::new(data.clone(), "audio/mp4"));
        let p2 = proxy.register(&raw_url, Some(data.len() as u64), Some("audio/mp4"));
        let r2 = test_http().get(&p2).send().unwrap();
        assert_eq!(r2.status(), reqwest::StatusCode::OK);
        assert_eq!(&r2.bytes().unwrap()[..], &data[..]);
    }

    #[test]
    fn lru_eviction_with_active_reader_cleans_tmp_on_drop() {
        let up = MockUpstream::start();
        let slow_data = make_payload(512 * 1024);
        let fast_data = make_payload(32 * 1024);
        let mut slow = MockRoute::new(slow_data.clone(), "audio/mp4");
        slow.trickle_chunk = 8 * 1024;
        slow.trickle_delay = Duration::from_millis(50);
        up.set("/act/1", slow);
        for i in 2..=12 {
            up.set(&format!("/act/{i}"), MockRoute::new(fast_data.clone(), "audio/mp4"));
        }

        let env = TempEnv::new("act");
        let proxy = env.proxy();
        let p1 = proxy.register(&up.url("/act/1"), Some(slow_data.len() as u64), Some("audio/mp4"));
        let mut resp1 = test_http().get(&p1).send().unwrap();
        assert_eq!(resp1.status(), reqwest::StatusCode::OK);

        // Read first 8 KiB while keeping resp1 open, then trigger LRU eviction
        let mut head = [0u8; 8192];
        resp1.read_exact(&mut head).unwrap();

        for i in 2..=12 {
            let pi = proxy.register(&up.url(&format!("/act/{i}")), Some(fast_data.len() as u64), Some("audio/mp4"));
            let bi = test_http().get(&pi).send().unwrap().bytes().unwrap();
            assert_eq!(&bi[..], &fast_data[..]);
        }

        // Drain/drop resp1; mid-body cancellation must unblock in < 2s (never 30s Keep-Alive hang)
        let unblock_start = Instant::now();
        let mut rest = Vec::new();
        let read_res = resp1.read_to_end(&mut rest);
        let unblock_elapsed = unblock_start.elapsed();
        assert!(read_res.is_err(), "cancelled stream must abort mid-body");
        assert!(
            unblock_elapsed < Duration::from_secs(2),
            "cancelled active reader took {unblock_elapsed:?} to unblock"
        );
        drop(resp1);
        drop(proxy);

        // Wait briefly for detached worker thread to drop its Arc<TempFileGuard>
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            let leftover: Vec<_> = std::fs::read_dir(env.cache_dir())
                .unwrap()
                .filter_map(|e| e.ok())
                .collect();
            if leftover.is_empty() {
                break;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        let leftover: Vec<_> = std::fs::read_dir(env.cache_dir())
            .unwrap()
            .filter_map(|e| e.ok())
            .collect();
        assert!(leftover.is_empty(), "Leaked temp files: {leftover:?}");
    }

    #[test]
    fn drop_after_200_without_content_length_learns_total_at_eof() {
        let up = MockUpstream::start();
        let data = make_payload(128 * 1024);
        let mut route = MockRoute::new(data.clone(), "audio/mp4");
        route.omit_content_length_on_200 = true;
        up.set("/nocl", route);

        let env = TempEnv::new("nocl");
        let proxy = env.proxy();
        let pu = proxy.register(&up.url("/nocl"), None, Some("audio/mp4"));
        let resp = test_http().get(&pu).send().unwrap();
        assert_eq!(resp.status(), reqwest::StatusCode::OK);
        assert_eq!(&resp.bytes().unwrap()[..], &data[..]);
    }

    #[test]
    fn mid_body_cancel_and_error_unblock_under_500ms_with_long_timeout() {
        let up = MockUpstream::start();
        let slow_data = make_payload(512 * 1024);
        let fast_data = make_payload(32 * 1024);

        // Case A: LRU eviction cancels active mid-body reader
        let mut slow = MockRoute::new(slow_data.clone(), "audio/mp4");
        slow.trickle_chunk = 8 * 1024;
        slow.trickle_delay = Duration::from_secs(2);
        up.set("/mid_cancel/0", slow);
        for i in 1..=11 {
            up.set(
                &format!("/mid_cancel/{i}"),
                MockRoute::new(fast_data.clone(), "audio/mp4"),
            );
        }

        let env = TempEnv::new("mid_abort");
        let proxy = env.proxy();
        let long_client = reqwest::blocking::Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(20))
            .build()
            .unwrap();

        let p0 = proxy.register(
            &up.url("/mid_cancel/0"),
            Some(slow_data.len() as u64),
            Some("audio/mp4"),
        );
        let mut resp0 = long_client.get(&p0).send().unwrap();
        assert_eq!(resp0.status(), reqwest::StatusCode::OK);
        let mut first_chunk = [0u8; 8192];
        resp0.read_exact(&mut first_chunk).unwrap();

        for i in 1..=11 {
            let pi = proxy.register(
                &up.url(&format!("/mid_cancel/{i}")),
                Some(fast_data.len() as u64),
                Some("audio/mp4"),
            );
            assert_eq!(
                &long_client.get(&pi).send().unwrap().bytes().unwrap()[..],
                &fast_data[..]
            );
        }

        let cancel_t0 = Instant::now();
        let mut tail = Vec::new();
        let res0 = resp0.read_to_end(&mut tail);
        let cancel_elapsed = cancel_t0.elapsed();
        assert!(res0.is_err(), "expected mid-body cancel error");
        assert!(
            cancel_elapsed < Duration::from_millis(500),
            "mid-body cancel hung for {cancel_elapsed:?}"
        );

        // Case B: Mid-body upstream drop + 403 error unblocks active reader < 500ms
        let mut drop_then_403 = MockRoute::new(slow_data.clone(), "audio/mp4");
        drop_then_403.trickle_chunk = 16 * 1024;
        drop_then_403.drop_after_bytes = Some(16 * 1024);
        drop_then_403.drop_only_first = true;
        up.set("/mid_err", drop_then_403);

        let pe = proxy.register(
            &up.url("/mid_err"),
            Some(slow_data.len() as u64),
            Some("audio/mp4"),
        );
        let mut resp_e = long_client.get(&pe).send().unwrap();
        assert_eq!(resp_e.status(), reqwest::StatusCode::OK);
        let mut first_16k = [0u8; 16 * 1024];
        resp_e.read_exact(&mut first_16k).unwrap();

        let mut bad_403 = MockRoute::new(slow_data.clone(), "audio/mp4");
        bad_403.status_override = Some(403);
        up.set("/mid_err", bad_403);

        let err_t0 = Instant::now();
        let mut err_tail = Vec::new();
        let res_e = resp_e.read_to_end(&mut err_tail);
        let err_elapsed = err_t0.elapsed();
        assert!(res_e.is_err(), "expected mid-body error abort");
        assert!(
            err_elapsed < Duration::from_millis(500),
            "mid-body error hung for {err_elapsed:?}"
        );
    }

    #[test]
    fn aborted_forward_seeks_do_not_starve_unrelated_fast_track() {
        let up = MockUpstream::start();
        let slow_data = make_payload(2_000_000);
        let fast_data = make_payload(32 * 1024);

        let mut slow = MockRoute::new(slow_data.clone(), "audio/mp4");
        slow.trickle_chunk = 16 * 1024;
        slow.trickle_delay = Duration::from_secs(1);
        up.set("/slow_seek", slow);
        up.set("/fast_track", MockRoute::new(fast_data.clone(), "audio/mp4"));

        let env = TempEnv::new("no_starve");
        let proxy = env.proxy();
        let p_slow = proxy.register(
            &up.url("/slow_seek"),
            Some(slow_data.len() as u64),
            Some("audio/mp4"),
        );
        let p_fast = proxy.register(
            &up.url("/fast_track"),
            Some(fast_data.len() as u64),
            Some("audio/mp4"),
        );

        let slow_tok = p_slow.rsplit('/').next().unwrap();
        for i in 0..10 {
            let offset = 1_000_000 + i * 50_000;
            let mut tcp =
                std::net::TcpStream::connect(("127.0.0.1", proxy.port())).unwrap();
            let req = format!(
                "GET /s/{slow_tok} HTTP/1.1\r\nHost: 127.0.0.1\r\nRange: bytes={offset}-\r\n\r\n"
            );
            tcp.write_all(req.as_bytes()).unwrap();
            tcp.flush().unwrap();
            std::thread::sleep(Duration::from_millis(20));
            drop(tcp);
        }

        let fast_t0 = Instant::now();
        let resp = test_http().get(&p_fast).send().unwrap();
        assert_eq!(resp.status(), reqwest::StatusCode::OK);
        let body = resp.bytes().unwrap();
        let fast_elapsed = fast_t0.elapsed();
        assert_eq!(&body[..], &fast_data[..]);
        assert!(
            fast_elapsed < Duration::from_secs(1),
            "/fast_track blocked for {fast_elapsed:?}"
        );
    }

    #[test]
    fn stalled_upstream_with_prepopulated_total_returns_504_on_head() {
        let up = MockUpstream::start();
        let data = make_payload(50_000);
        let mut route = MockRoute::new(data.clone(), "audio/mp4");
        route.stall_headers = Some(Duration::from_millis(5500));
        route.status_override = Some(403);
        up.set("/stalled_head", route);

        let env = TempEnv::new("stalled_total");
        let proxy = env.proxy();
        let pu = proxy.register(
            &up.url("/stalled_head"),
            Some(data.len() as u64),
            Some("audio/mp4"),
        );

        let resp = test_http().head(&pu).send().unwrap();
        assert_eq!(resp.status(), reqwest::StatusCode::GATEWAY_TIMEOUT);
    }

    #[test]
    fn parse_content_range_case_insensitive_and_sanitize_content_type() {
        assert_eq!(
            parse_content_range("Bytes 0-99/100"),
            Some((Some(0), Some(99), Some(100)))
        );
        assert_eq!(
            parse_content_range("BYTES */500"),
            Some((None, None, Some(500)))
        );
        assert_eq!(parse_content_range("items 0-9/10"), None);
        assert_eq!(
            sanitize_content_type("audio/mp4;\r\nX-Evil: 1"),
            "audio/mp4;X-Evil: 1"
        );
        assert_eq!(sanitize_content_type("\r\n\0"), "application/octet-stream");
    }

    #[test]
    fn parse_http_request_http10_connection_close_wins_and_malformed_content_length() {
        let req1 = parse_http_request(
            b"GET /s/abc HTTP/1.0\r\nHost: 127.0.0.1\r\nConnection: close, keep-alive\r\n",
        )
        .expect("HTTP/1.0 close, keep-alive should parse");
        assert!(!req1.keep_alive, "close must win over keep-alive on HTTP/1.0");

        let req2 = parse_http_request(
            b"GET /s/abc HTTP/1.0\r\nHost: 127.0.0.1\r\nConnection: keep-alive\r\n",
        )
        .expect("HTTP/1.0 keep-alive should parse");
        assert!(req2.keep_alive);

        let req3 = parse_http_request(
            b"GET /s/abc HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: keep-alive\r\nConnection: close\r\n",
        )
        .expect("multiple Connection headers should parse");
        assert!(!req3.keep_alive);

        let bad_cl = parse_http_request(
            b"GET /s/abc HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: notanumber\r\n",
        );
        assert!(bad_cl.is_none(), "malformed Content-Length must be rejected");

        let plus_cl = parse_http_request(
            b"GET /s/abc HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: +5\r\n",
        );
        assert!(plus_cl.is_none(), "signed Content-Length (+5) must be rejected");

        let empty_cl = parse_http_request(
            b"GET /s/abc HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: \r\n",
        );
        assert!(empty_cl.is_none(), "empty Content-Length must be rejected");
    }

    #[test]
    fn fragment_and_query_stripped_from_proxy_target() {
        let up = MockUpstream::start();
        let data = make_payload(16 * 1024);
        up.set("/frag", MockRoute::new(data.clone(), "audio/mp4"));

        let env = TempEnv::new("frag");
        let proxy = env.proxy();
        let pu = proxy.register(&up.url("/frag"), Some(data.len() as u64), Some("audio/mp4"));
        let tok = pu.rsplit('/').next().unwrap();

        let mut tcp = std::net::TcpStream::connect(("127.0.0.1", proxy.port())).unwrap();
        let req = format!(
            "GET /s/{tok}#section?t=1 HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n"
        );
        tcp.write_all(req.as_bytes()).unwrap();
        tcp.flush().unwrap();
        let mut resp_bytes = Vec::new();
        tcp.read_to_end(&mut resp_bytes).unwrap();
        let split_at = resp_bytes
            .windows(4)
            .position(|w| w == b"\r\n\r\n")
            .expect("HTTP response headers");
        let hdrs = String::from_utf8_lossy(&resp_bytes[..split_at]);
        assert!(hdrs.starts_with("HTTP/1.1 200 OK"), "unexpected headers: {hdrs}");
        assert_eq!(&resp_bytes[split_at + 4..], &data[..]);
    }

    #[test]
    fn short_cache_file_ok0_aborts_without_busy_spin() {
        let env = TempEnv::new("ok0_spin");
        let proxy = env.proxy();
        let pu = proxy.register("http://127.0.0.1:1/unused", Some(4096), Some("audio/mp4"));
        let tok = pu.rsplit('/').next().unwrap().to_string();

        // Pre-create a 0-byte file while claiming 1024 bytes downloaded and !is_complete
        {
            let reg = proxy.registry.lock().unwrap();
            let sess = reg.sessions.get(&tok).unwrap();
            sess.started.store(true, Ordering::SeqCst);
            let _f = open_shared_write(&sess.file_path).unwrap();
            sess.progress.downloaded.store(1024, Ordering::Release);
            sess.progress.headers_received.store(true, Ordering::Release);
        }

        let t0 = Instant::now();
        let res = test_http().get(&pu).send();
        let body_res = res.and_then(|r| r.bytes());
        let elapsed = t0.elapsed();
        assert!(body_res.is_err(), "truncated file must abort cleanly");
        assert!(
            elapsed < Duration::from_secs(2),
            "Ok(0) busy-spin or hang took {elapsed:?}"
        );
    }

    #[test]
    fn head_succeeds_immediately_once_headers_received_even_before_first_byte() {
        let up = MockUpstream::start();
        let data = make_payload(100_000);
        let mut route = MockRoute::new(data.clone(), "audio/mp4");
        route.trickle_chunk = 50_000;
        route.trickle_delay = Duration::from_millis(200);
        up.set("/hdr_ready", route);

        let env = TempEnv::new("hdr_ready");
        let proxy = env.proxy();
        let pu = proxy.register(&up.url("/hdr_ready"), Some(data.len() as u64), Some("audio/mp4"));
        let tok = pu.rsplit('/').next().unwrap().to_string();

        // Simulate headers_received = true while downloaded == 0
        {
            let reg = proxy.registry.lock().unwrap();
            let sess = reg.sessions.get(&tok).unwrap();
            sess.started.store(true, Ordering::SeqCst);
            sess.progress.headers_received.store(true, Ordering::Release);
        }

        let t0 = Instant::now();
        let resp = test_http().head(&pu).send().unwrap();
        assert_eq!(resp.status(), reqwest::StatusCode::OK);
        assert!(t0.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn alr_text_plain_redirect_follows_and_serves_actual_media() {
        let up = MockUpstream::start();
        let media = make_payload(256 * 1024);
        up.set("/alr/media", MockRoute::new(media.clone(), "video/mp4"));

        let mut hop2 = MockRoute::new(
            Arc::new(up.url("/alr/media").into_bytes()),
            "text/plain",
        );
        hop2.force_200_ignore_range = true;
        up.set("/alr/hop2", hop2);

        let mut hop1 = MockRoute::new(
            Arc::new(format!("{}\r\n", up.url("/alr/hop2")).into_bytes()),
            "text/plain; charset=utf-8",
        );
        hop1.force_200_ignore_range = true;
        up.set("/alr/hop1", hop1);

        let env = TempEnv::new("alr_ok");
        let proxy = env.proxy();
        let pu = proxy.register(&up.url("/alr/hop1"), None, Some("video/mp4"));

        let head = test_http().head(&pu).send().unwrap();
        assert_eq!(head.status(), reqwest::StatusCode::OK);
        assert_eq!(
            head.headers().get("content-type").and_then(|v| v.to_str().ok()),
            Some("video/mp4")
        );
        assert_eq!(
            head.headers().get("content-length").and_then(|v| v.to_str().ok()),
            Some("262144")
        );

        let get = test_http().get(&pu).send().unwrap();
        assert_eq!(get.status(), reqwest::StatusCode::OK);
        assert_eq!(
            get.headers().get("content-type").and_then(|v| v.to_str().ok()),
            Some("video/mp4")
        );
        let body = get.bytes().unwrap();
        assert_eq!(&body[..], &media[..]);

        let seek = test_http()
            .get(&pu)
            .header("Range", "bytes=65536-131071")
            .send()
            .unwrap();
        assert_eq!(seek.status(), reqwest::StatusCode::PARTIAL_CONTENT);
        assert_eq!(&seek.bytes().unwrap()[..], &media[65536..=131071]);

        // Also test when session initially had mime=None and target is audio/webm
        up.set("/alr/audio", MockRoute::new(media.clone(), "audio/webm; codecs=\"opus\""));
        let mut hop_a = MockRoute::new(
            Arc::new(up.url("/alr/audio").into_bytes()),
            "text/plain",
        );
        hop_a.force_200_ignore_range = true;
        up.set("/alr/hop_a", hop_a);
        let pu_a = proxy.register(&up.url("/alr/hop_a"), None, None);
        let resp_a = test_http().get(&pu_a).send().unwrap();
        assert_eq!(resp_a.status(), reqwest::StatusCode::OK);
        assert_eq!(
            resp_a.headers().get("content-type").and_then(|v| v.to_str().ok()),
            Some("audio/webm; codecs=\"opus\"")
        );
        assert_eq!(&resp_a.bytes().unwrap()[..], &media[..]);
    }

    #[test]
    fn non_media_content_type_and_alr_loop_rejected_with_502() {
        let up = MockUpstream::start();

        // 1. text/plain non-URL body -> 502, never 200 OK text/plain
        let mut text_err = MockRoute::new(
            Arc::new(b"Error 403: edge rejected request".to_vec()),
            "text/plain",
        );
        text_err.force_200_ignore_range = true;
        up.set("/nonmedia/text", text_err);

        // 2. text/html error page -> 502
        let mut html_err = MockRoute::new(
            Arc::new(b"<html><body>blocked</body></html>".to_vec()),
            "text/html; charset=utf-8",
        );
        html_err.force_200_ignore_range = true;
        up.set("/nonmedia/html", html_err);

        // 3. ALR loop (> MAX_ALR_HOPS) -> 502
        let mut loop_a = MockRoute::new(
            Arc::new(up.url("/nonmedia/loop_b").into_bytes()),
            "text/plain",
        );
        loop_a.force_200_ignore_range = true;
        let mut loop_b = MockRoute::new(
            Arc::new(up.url("/nonmedia/loop_a").into_bytes()),
            "text/plain",
        );
        loop_b.force_200_ignore_range = true;
        up.set("/nonmedia/loop_a", loop_a);
        up.set("/nonmedia/loop_b", loop_b);

        let env = TempEnv::new("nonmedia");
        let proxy = env.proxy();

        let p_text = proxy.register(&up.url("/nonmedia/text"), None, Some("video/mp4"));
        let r_text = test_http().get(&p_text).send().unwrap();
        assert_eq!(r_text.status(), reqwest::StatusCode::BAD_GATEWAY);
        {
            let tok = p_text.rsplit('/').next().unwrap();
            let reg = proxy.registry.lock().unwrap();
            let sess = reg.sessions.get(tok).unwrap();
            assert_eq!(&*sess.progress.mime.lock().unwrap(), "video/mp4");
            assert!(!sess.progress.is_complete.load(Ordering::Acquire));
            assert!(sess.progress.total.lock().unwrap().is_none());
        }

        let p_html = proxy.register(&up.url("/nonmedia/html"), Some(33), Some("audio/mp4"));
        assert_eq!(
            test_http().head(&p_html).send().unwrap().status(),
            reqwest::StatusCode::BAD_GATEWAY
        );
        assert_eq!(
            test_http().get(&p_html).send().unwrap().status(),
            reqwest::StatusCode::BAD_GATEWAY
        );

        let p_loop = proxy.register(&up.url("/nonmedia/loop_a"), None, Some("video/mp4"));
        assert_eq!(
            test_http().get(&p_loop).send().unwrap().status(),
            reqwest::StatusCode::BAD_GATEWAY
        );

        // parse_alr_redirect_url downgrade & validation checks
        assert!(parse_alr_redirect_url(b"https://cdn.example/v?a=1\n", "https://edge.example/v").is_some());
        assert!(parse_alr_redirect_url(b"http://cdn.example/v", "https://edge.example/v").is_none());
        assert!(parse_alr_redirect_url(b"http://127.0.0.1:8080/v", "http://127.0.0.1:8080/a").is_some());
        assert!(parse_alr_redirect_url(b"not a url", "http://127.0.0.1:8080/a").is_none());
        assert!(parse_alr_redirect_url(b"ftp://cdn.example/v", "http://127.0.0.1:8080/a").is_none());
    }
}
