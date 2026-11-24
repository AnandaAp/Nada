use std::collections::hash_map::RandomState;
use std::collections::{HashMap, VecDeque};
use std::hash::BuildHasher;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

pub struct SharedProgress {
    pub downloaded: AtomicU64,
    pub total: Mutex<Option<u64>>,
    pub mime: Mutex<String>,
    pub is_complete: AtomicBool,
    pub is_cancelled: AtomicBool,
    pub error: Mutex<Option<String>>,
    pub notify: Condvar,
    pub lock: Mutex<()>,
}

impl SharedProgress {
    pub fn new() -> Self {
        Self {
            downloaded: AtomicU64::new(0),
            total: Mutex::new(None),
            mime: Mutex::new("application/octet-stream".to_string()),
            is_complete: AtomicBool::new(false),
            is_cancelled: AtomicBool::new(false),
            error: Mutex::new(None),
            notify: Condvar::new(),
            lock: Mutex::new(()),
        }
    }
}

pub struct StreamSession {
    pub token: String,
    pub url: String,
    pub file_path: PathBuf,
    pub progress: Arc<SharedProgress>,
    pub started: AtomicBool,
}

pub struct StreamProxy {
    port: u16,
    sessions: Arc<Mutex<HashMap<String, Arc<StreamSession>>>>,
    token_order: Arc<Mutex<VecDeque<String>>>,
    _downloaded_lru: Arc<Mutex<VecDeque<String>>>,
    hasher: RandomState,
    cache_dir: PathBuf,
    _log: Arc<Mutex<std::fs::File>>,
}

fn log_line(log: &Arc<Mutex<std::fs::File>>, line: String) {
    use std::io::Write;
    if let Ok(mut f) = log.lock() {
        let _ = writeln!(f, "{line}");
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
        let server = tiny_http::Server::http("127.0.0.1:0").map_err(|e| e.to_string())?;
        let port = server
            .server_addr()
            .to_ip()
            .expect("proxy bound to ip")
            .port();
        let server = Arc::new(server);
        let sessions: Arc<Mutex<HashMap<String, Arc<StreamSession>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let token_order: Arc<Mutex<VecDeque<String>>> = Arc::new(Mutex::new(VecDeque::new()));
        let downloaded_lru: Arc<Mutex<VecDeque<String>>> = Arc::new(Mutex::new(VecDeque::new()));

        let client = reqwest::blocking::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(20))
            .redirect(reqwest::redirect::Policy::limited(5))
            .user_agent(
                "com.google.ios.youtube/19.29.1 (iPhone16,2; U; CPU iOS 17_5_1 like Mac OS X; en_US)",
            )
            .build()
            .map_err(|e| e.to_string())?;

        for _ in 0..8 {
            let server = server.clone();
            let sessions = sessions.clone();
            let downloaded_lru = downloaded_lru.clone();
            let client = client.clone();
            let log = log.clone();
            std::thread::spawn(move || {
                for request in server.incoming_requests() {
                    serve(request, &sessions, &downloaded_lru, &client, &log);
                }
            });
        }

        Ok(Self {
            port,
            sessions,
            token_order,
            _downloaded_lru: downloaded_lru,
            hasher: RandomState::new(),
            cache_dir,
            _log: log,
        })
    }

    pub fn register(&self, url: &str, total: Option<u64>, mime: Option<&str>) -> String {
        let token = format!("{:016x}", self.hasher.hash_one(url));
        let mut sessions = self.sessions.lock().unwrap();
        let mut tokens = self.token_order.lock().unwrap();

        if let Some(session) = sessions.get(&token) {
            if let Some(t) = total {
                let mut total_lock = session.progress.total.lock().unwrap();
                if total_lock.is_none() {
                    *total_lock = Some(t);
                }
            }
            if let Some(m) = mime {
                *session.progress.mime.lock().unwrap() = m.to_string();
            }
            tokens.retain(|t| t != &token);
            tokens.push_back(token.clone());
            return format!("http://127.0.0.1:{}/s/{}", self.port, token);
        }

        let file_path = self.cache_dir.join(format!("{}.tmp", token));
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
            file_path,
            progress,
            started: AtomicBool::new(false),
        });

        sessions.insert(token.clone(), session);
        tokens.push_back(token.clone());

        // Retain metadata mappings in memory (up to 300 stream tokens = ~15 tracks)
        while tokens.len() > 300 {
            if let Some(evicted_token) = tokens.pop_front() {
                if let Some(evicted_session) = sessions.remove(&evicted_token) {
                    evicted_session
                        .progress
                        .is_cancelled
                        .store(true, Ordering::Relaxed);
                    evicted_session.progress.notify.notify_all();
                    let _ = std::fs::remove_file(&evicted_session.file_path);
                }
            }
        }

        format!("http://127.0.0.1:{}/s/{}", self.port, token)
    }
}

pub fn parse_range(value: Option<&str>, total: Option<u64>) -> (u64, Option<u64>) {
    let Some(v) = value else {
        return (0, None);
    };
    let Some(spec) = v.strip_prefix("bytes=") else {
        return (0, None);
    };
    let (left, right) = match spec.split_once('-') {
        Some(pair) => pair,
        None => return (0, None),
    };
    if left.is_empty() {
        let n: u64 = right.parse().unwrap_or(0);
        return match total {
            Some(t) => (t.saturating_sub(n), Some(t.saturating_sub(1))),
            None => (0, None),
        };
    }
    let start: u64 = left.parse().unwrap_or(0);
    if right.is_empty() {
        return (start, None);
    }
    let end: u64 = right.parse().unwrap_or(start);
    (start, Some(end))
}

fn download_worker(
    session: Arc<StreamSession>,
    client: reqwest::blocking::Client,
    log: Arc<Mutex<std::fs::File>>,
) {
    use std::io::{Read, Write};
    log_line(
        &log,
        format!("download_worker started for token {}", session.token),
    );

    let file = match std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&session.file_path)
    {
        Ok(f) => f,
        Err(e) => {
            log_line(
                &log,
                format!("download_worker file create error {}: {e}", session.token),
            );
            *session.progress.error.lock().unwrap() = Some(e.to_string());
            session.progress.notify.notify_all();
            return;
        }
    };
    let mut writer = file;

    let chunk_size = 512 * 1024u64; // 512 KB chunks for smooth, throttled-free streaming
    let mut offset = 0u64;
    let mut total_size = *session.progress.total.lock().unwrap();

    loop {
        if session.progress.is_cancelled.load(Ordering::Relaxed) {
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
                session.progress.notify.notify_all();
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

        let chunk_end_spec = match total_size {
            Some(total) => format!("{}-{}", offset, (offset + chunk_size - 1).min(total - 1)),
            None => format!("{}-{}", offset, offset + chunk_size - 1),
        };

        let mut success = false;
        let mut last_error = String::new();

        for attempt in 1..=3 {
            if session.progress.is_cancelled.load(Ordering::Relaxed) {
                break;
            }
            let req_builder = client
                .get(&session.url)
                .header(reqwest::header::RANGE, format!("bytes={}", chunk_end_spec));
            let req_for_log = req_builder.try_clone();
            let resp_res = req_builder.send();

            let mut resp = match resp_res {
                Ok(r) => r,
                Err(e) => {
                    last_error = e.without_url().to_string();
                    log_line(
                        &log,
                        format!(
                            "download_worker {} attempt {} req error: {}",
                            session.token, attempt, last_error
                        ),
                    );
                    std::thread::sleep(Duration::from_millis(200 * attempt));
                    continue;
                }
            };

            let status = resp.status();
            if !status.is_success() && status != reqwest::StatusCode::PARTIAL_CONTENT {
                last_error = format!("upstream status {status}");
                let built_url = req_for_log
                    .and_then(|r| r.build().ok())
                    .map(|r| r.url().to_string())
                    .unwrap_or_default();
                log_line(
                    &log,
                    format!(
                        "download_worker {} attempt {} range {} {last_error} url={built_url}",
                        session.token, attempt, chunk_end_spec
                    ),
                );
                if status == reqwest::StatusCode::RANGE_NOT_SATISFIABLE {
                    success = true;
                    total_size = Some(offset);
                    *session.progress.total.lock().unwrap() = Some(offset);
                    break;
                }
                std::thread::sleep(Duration::from_millis(200 * attempt));
                continue;
            }

            if offset == 0 {
                if let Some(ct) = resp.headers().get(reqwest::header::CONTENT_TYPE) {
                    if let Ok(mime_str) = ct.to_str() {
                        *session.progress.mime.lock().unwrap() = mime_str.to_string();
                    }
                }
                if total_size.is_none() {
                    if let Some(cr) = resp.headers().get(reqwest::header::CONTENT_RANGE) {
                        if let Some(t) = cr
                            .to_str()
                            .ok()
                            .and_then(|s| s.rsplit('/').next())
                            .and_then(|s| s.parse::<u64>().ok())
                        {
                            total_size = Some(t);
                            *session.progress.total.lock().unwrap() = Some(t);
                        }
                    }
                }
            }

            let mut chunk_buf = [0u8; 64 * 1024];
            let mut chunk_downloaded = 0u64;
            let mut read_ok = true;

            loop {
                match resp.read(&mut chunk_buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        if let Err(e) = writer.write_all(&chunk_buf[..n]) {
                            last_error = e.to_string();
                            read_ok = false;
                            break;
                        }
                        chunk_downloaded += n as u64;
                        session
                            .progress
                            .downloaded
                            .fetch_add(n as u64, Ordering::Release);
                        session.progress.notify.notify_all();
                    }
                    Err(e) => {
                        last_error = e.to_string();
                        read_ok = false;
                        break;
                    }
                }
            }

            if read_ok {
                let _ = writer.flush();
                offset += chunk_downloaded;
                success = true;
                if chunk_downloaded == 0 {
                    total_size = Some(offset);
                    *session.progress.total.lock().unwrap() = Some(offset);
                }
                break;
            } else {
                std::thread::sleep(Duration::from_millis(200 * attempt));
            }
        }

        if !success {
            log_line(
                &log,
                format!("download_worker failed for token {}: {last_error}", session.token),
            );
            *session.progress.error.lock().unwrap() = Some(last_error);
            session.progress.notify.notify_all();
            break;
        }
    }
}

pub struct CacheRangeReader {
    file: std::fs::File,
    pos: u64,
    end: u64,
    progress: Arc<SharedProgress>,
}

impl std::io::Read for CacheRangeReader {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        use std::io::{ErrorKind, Seek, SeekFrom};
        if out.is_empty() || self.pos > self.end {
            return Ok(0);
        }

        loop {
            if self.progress.is_cancelled.load(Ordering::Acquire) {
                return Err(std::io::Error::new(
                    ErrorKind::Interrupted,
                    "stream cancelled",
                ));
            }
            if let Some(err) = self.progress.error.lock().unwrap().as_ref() {
                return Err(std::io::Error::new(ErrorKind::Other, err.clone()));
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
                if self.progress.is_complete.load(Ordering::Acquire) {
                    return Ok(0);
                }
            } else if self.progress.is_complete.load(Ordering::Acquire) {
                return Ok(0);
            }

            let guard = self.progress.lock.lock().unwrap();
            let _ = self
                .progress
                .notify
                .wait_timeout(guard, Duration::from_millis(200));
        }
    }
}

fn serve(
    request: tiny_http::Request,
    sessions: &Arc<Mutex<HashMap<String, Arc<StreamSession>>>>,
    downloaded_lru: &Arc<Mutex<VecDeque<String>>>,
    client: &reqwest::blocking::Client,
    log: &Arc<Mutex<std::fs::File>>,
) {
    let token = request.url().trim_start_matches("/s/").to_string();
    let session = {
        let map = sessions.lock().unwrap();
        map.get(&token).cloned()
    };
    let Some(session) = session else {
        log_line(log, format!("404 {}", request.url()));
        let _ =
            request.respond(tiny_http::Response::from_string("not found").with_status_code(404));
        return;
    };

    // Lazy worker spawn: only launch download_worker on the first request for this session
    if session
        .started
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_ok()
    {
        // Enforce max 10 actively downloaded files on disk
        let mut downloaded = downloaded_lru.lock().unwrap();
        downloaded.retain(|t| t != &token);
        downloaded.push_back(token.clone());
        while downloaded.len() > 10 {
            if let Some(evicted_token) = downloaded.pop_front() {
                if let Some(evicted_session) = sessions.lock().unwrap().get(&evicted_token).cloned() {
                    evicted_session
                        .progress
                        .is_cancelled
                        .store(true, Ordering::Relaxed);
                    evicted_session.progress.notify.notify_all();
                    let _ = std::fs::remove_file(&evicted_session.file_path);
                }
            }
        }

        let s = session.clone();
        let c = client.clone();
        let l = log.clone();
        std::thread::spawn(move || {
            download_worker(s, c, l);
        });
    }

    // Contract: wait up to 5.0s for total to be determined AND first bytes to arrive
    let start_wait = Instant::now();
    loop {
        if (session.progress.total.lock().unwrap().is_some()
            && (session.progress.downloaded.load(Ordering::Acquire) > 0
                || session.progress.is_complete.load(Ordering::Acquire)))
            || session.progress.is_complete.load(Ordering::Acquire)
        {
            break;
        }
        if session.progress.error.lock().unwrap().is_some()
            || session.progress.is_cancelled.load(Ordering::Acquire)
        {
            break;
        }
        if start_wait.elapsed() >= Duration::from_millis(5000) {
            break;
        }
        let guard = session.progress.lock.lock().unwrap();
        let _ = session
            .progress
            .notify
            .wait_timeout(guard, Duration::from_millis(50));
    }

    if let Some(err) = session.progress.error.lock().unwrap().as_ref() {
        log_line(log, format!("502 error for {}: {err}", request.url()));
        let _ = request.respond(
            tiny_http::Response::from_string(format!("upstream error: {err}"))
                .with_status_code(502),
        );
        return;
    }

    let total = *session.progress.total.lock().unwrap();
    let Some(total) = total else {
        log_line(
            log,
            format!("504 timeout waiting for stream data for {}", request.url()),
        );
        let _ = request
            .respond(tiny_http::Response::from_string("stream timeout").with_status_code(504));
        return;
    };

    let client_range = request
        .headers()
        .iter()
        .find(|h| h.field.equiv("Range"))
        .map(|h| h.value.as_str().to_string());

    let (start, end_opt) = parse_range(client_range.as_deref(), Some(total));
    let end = end_opt
        .unwrap_or(total.saturating_sub(1))
        .min(total.saturating_sub(1));

    if total == 0 || start > end {
        let _ = request.respond(tiny_http::Response::from_string("").with_status_code(416));
        return;
    }

    let mime = session.progress.mime.lock().unwrap().clone();
    let head = request.method() == &tiny_http::Method::Head;
    let len = (end - start + 1) as usize;

    let headers = vec![
        tiny_http::Header::from_bytes(b"Content-Type", mime.as_bytes()).unwrap(),
        tiny_http::Header::from_bytes(
            b"Content-Range",
            format!("bytes {start}-{end}/{total}").as_bytes(),
        )
        .unwrap(),
        tiny_http::Header::from_bytes(b"Accept-Ranges", b"bytes").unwrap(),
    ];

    log_line(
        log,
        format!(
            "{} {} mime={mime} client={:?} serve={start}-{end}/{total}",
            request.method(),
            request.url(),
            client_range
        ),
    );

    let reader: Box<dyn std::io::Read + Send> = if head {
        Box::new(std::io::empty())
    } else {
        let mut file_opt = None;
        for _ in 0..20 {
            if let Ok(f) = std::fs::File::open(&session.file_path) {
                file_opt = Some(f);
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }

        let Some(file) = file_opt else {
            log_line(
                log,
                format!(
                    "500 file open error {}",
                    session.file_path.display()
                ),
            );
            let _ = request
                .respond(tiny_http::Response::from_string("file error").with_status_code(500));
            return;
        };

        Box::new(CacheRangeReader {
            file,
            pos: start,
            end,
            progress: session.progress.clone(),
        })
    };

    let response = tiny_http::Response::new(
        tiny_http::StatusCode(206),
        headers,
        reader,
        if head { Some(0) } else { Some(len) },
        None,
    );
    let _ = request.respond(response);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn parse_range_cases() {
        assert_eq!(parse_range(None, Some(1000)), (0, None));
        assert_eq!(parse_range(Some("bytes=0-499"), Some(1000)), (0, Some(499)));
        assert_eq!(parse_range(Some("bytes=500-"), Some(1000)), (500, None));
        assert_eq!(
            parse_range(Some("bytes=-200"), Some(1000)),
            (800, Some(999))
        );
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
        };

        let mut buf = vec![0u8; 11];
        use std::io::Read;
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
        };

        let mut buf = [0u8; 10];
        use std::io::Read;
        let res = reader.read(&mut buf);
        assert!(res.is_err());
        assert_eq!(res.unwrap_err().kind(), std::io::ErrorKind::Interrupted);

        let _ = std::fs::remove_file(&file_path);
    }
}
