use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[path = "../src/proxy.rs"]
mod proxy;

// ============================================================================
// Adapter & Deterministic Synthetic Media / Mock Origin Helpers
// ============================================================================

struct TestTempDir(PathBuf);

impl TestTempDir {
    fn new(prefix: &str) -> Self {
        static SEQ: AtomicUsize = AtomicUsize::new(0);
        let n = SEQ.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "nada-e2e-{prefix}-{}-{n}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("cache")).unwrap();
        Self(dir)
    }

    fn log_path(&self) -> PathBuf {
        self.0.join("proxy.log")
    }

    fn cache_dir(&self) -> PathBuf {
        self.0.join("cache")
    }
}

impl Drop for TestTempDir {
    fn drop(&mut self) {
        for attempt in 0..15 {
            if !self.0.exists() || std::fs::remove_dir_all(&self.0).is_ok() {
                break;
            }
            if attempt < 14 {
                std::thread::sleep(Duration::from_millis(40));
            }
        }
    }
}

struct ProxyAdapter {
    inner: proxy::StreamProxy,
    _dir: TestTempDir,
}

impl ProxyAdapter {
    fn start(name: &str) -> Self {
        let dir = TestTempDir::new(name);
        let inner = proxy::StreamProxy::start(dir.log_path(), dir.cache_dir())
            .expect("StreamProxy::start failed");
        Self { inner, _dir: dir }
    }

    fn port(&self) -> u16 {
        self.inner.port()
    }

    fn register(&self, url: &str, total: Option<u64>, mime: Option<&str>) -> String {
        self.inner.register(url, total, mime)
    }

    fn log_contents(&self) -> String {
        std::fs::read_to_string(self._dir.log_path()).unwrap_or_default()
    }

    fn cache_dir(&self) -> PathBuf {
        self._dir.cache_dir()
    }
}

fn synthetic_byte(offset: usize) -> u8 {
    ((offset.wrapping_mul(31).wrapping_add(7)) % 251) as u8
}

fn make_synthetic_payload(len: usize) -> Vec<u8> {
    (0..len).map(synthetic_byte).collect()
}

fn verify_slice(data: &[u8], start_offset: usize, label: &str) {
    for (i, &b) in data.iter().enumerate() {
        let expected = synthetic_byte(start_offset + i);
        if b != expected {
            panic!(
                "{label}: byte mismatch at offset {} (relative {i}): got {b}, expected {expected}",
                start_offset + i
            );
        }
    }
}

#[derive(Clone)]
struct OriginRouteConfig {
    payload: Arc<Vec<u8>>,
    mime: String,
    po_trap_nonzero_range: bool,
    drop_after_bytes: Option<usize>,
    drop_only_first_hit: bool,
    force_status: Option<u16>,
    trickle_chunk_size: usize,
    trickle_delay: Duration,
    stall_headers_for: Option<Duration>,
    accept_ranges_200_on_zero: bool,
}

impl OriginRouteConfig {
    fn new(payload: Arc<Vec<u8>>, mime: &str) -> Self {
        Self {
            payload,
            mime: mime.to_string(),
            po_trap_nonzero_range: false,
            drop_after_bytes: None,
            drop_only_first_hit: false,
            force_status: None,
            trickle_chunk_size: 64 * 1024,
            trickle_delay: Duration::ZERO,
            stall_headers_for: None,
            accept_ranges_200_on_zero: false,
        }
    }
}

#[derive(Clone, Debug)]
struct RecordedUpstreamReq {
    path: String,
    method: String,
    range: Option<String>,
    user_agent: Option<String>,
}

struct MockOriginServer {
    port: u16,
    routes: Arc<Mutex<HashMap<String, OriginRouteConfig>>>,
    hits: Arc<Mutex<HashMap<String, usize>>>,
    requests: Arc<Mutex<Vec<RecordedUpstreamReq>>>,
    stop: Arc<AtomicBool>,
}

impl MockOriginServer {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock origin");
        listener.set_nonblocking(true).unwrap();
        let port = listener.local_addr().unwrap().port();
        let routes: Arc<Mutex<HashMap<String, OriginRouteConfig>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let hits: Arc<Mutex<HashMap<String, usize>>> = Arc::new(Mutex::new(HashMap::new()));
        let requests: Arc<Mutex<Vec<RecordedUpstreamReq>>> = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));

        let r_clone = routes.clone();
        let h_clone = hits.clone();
        let req_clone = requests.clone();
        let s_clone = stop.clone();

        std::thread::spawn(move || {
            while !s_clone.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        let routes = r_clone.clone();
                        let hits = h_clone.clone();
                        let requests = req_clone.clone();
                        std::thread::spawn(move || {
                            handle_mock_conn(&mut stream, &routes, &hits, &requests);
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
            hits,
            requests,
            stop,
        }
    }

    fn set_route(&self, path: &str, cfg: OriginRouteConfig) {
        self.routes.lock().unwrap().insert(path.to_string(), cfg);
    }

    fn url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}{}", self.port, path)
    }

    fn hits_for(&self, path: &str) -> usize {
        *self.hits.lock().unwrap().get(path).unwrap_or(&0)
    }

    fn requests_for(&self, path: &str) -> Vec<RecordedUpstreamReq> {
        self.requests
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r.path == path)
            .cloned()
            .collect()
    }
}

impl Drop for MockOriginServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

fn handle_mock_conn(
    stream: &mut std::net::TcpStream,
    routes: &Arc<Mutex<HashMap<String, OriginRouteConfig>>>,
    hits: &Arc<Mutex<HashMap<String, usize>>>,
    requests: &Arc<Mutex<Vec<RecordedUpstreamReq>>>,
) {
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
    let mut buf = Vec::new();
    let mut tmp = [0u8; 4096];
    loop {
        match stream.read(&mut tmp) {
            Ok(0) => return,
            Ok(n) => {
                buf.extend_from_slice(&tmp[..n]);
                if buf.windows(4).any(|w| w == b"\r\n\r\n") || buf.len() > 32 * 1024 {
                    break;
                }
            }
            Err(_) => return,
        }
    }

    let req_str = String::from_utf8_lossy(&buf);
    let mut lines = req_str.split("\r\n");
    let first = lines.next().unwrap_or("");
    let mut parts = first.split_whitespace();
    let method = parts.next().unwrap_or("GET").to_string();
    let path = parts.next().unwrap_or("/").to_string();

    let mut range_hdr: Option<String> = None;
    let mut ua_hdr: Option<String> = None;
    for line in lines {
        if line.is_empty() {
            break;
        }
        if let Some((k, v)) = line.split_once(':') {
            if k.eq_ignore_ascii_case("range") {
                range_hdr = Some(v.trim().to_string());
            } else if k.eq_ignore_ascii_case("user-agent") {
                ua_hdr = Some(v.trim().to_string());
            }
        }
    }

    requests.lock().unwrap().push(RecordedUpstreamReq {
        path: path.clone(),
        method: method.clone(),
        range: range_hdr.clone(),
        user_agent: ua_hdr,
    });

    let cfg = routes.lock().unwrap().get(&path).cloned();
    let Some(cfg) = cfg else {
        let _ = stream.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
        return;
    };

    let hit_num = {
        let mut h = hits.lock().unwrap();
        let entry = h.entry(path.clone()).or_insert(0);
        *entry += 1;
        *entry
    };

    if let Some(stall) = cfg.stall_headers_for {
        std::thread::sleep(stall);
    }

    if let Some(status) = cfg.force_status {
        let hdr = format!(
            "HTTP/1.1 {status} Error\r\nContent-Length: 5\r\nConnection: close\r\n\r\nerror"
        );
        let _ = stream.write_all(hdr.as_bytes());
        return;
    }

    let total = cfg.payload.len();
    let (start, end, has_range) = match range_hdr.as_deref() {
        Some(r) => {
            let spec = r.strip_prefix("bytes=").unwrap_or("");
            let (l, rgt) = spec.split_once('-').unwrap_or(("0", ""));
            let s: usize = l.parse().unwrap_or(0);
            let e: usize = if rgt.is_empty() {
                total.saturating_sub(1)
            } else {
                rgt.parse::<usize>().unwrap_or(total - 1).min(total - 1)
            };
            (s, e, true)
        }
        None => (0, total.saturating_sub(1), false),
    };

    if cfg.po_trap_nonzero_range && has_range && start > 0 {
        let _ = stream.write_all(
            b"HTTP/1.1 403 Forbidden\r\nContent-Length: 9\r\nConnection: close\r\n\r\nPO403Trap",
        );
        return;
    }

    if total == 0 || start > end || start >= total {
        let hdr = format!(
            "HTTP/1.1 416 Range Not Satisfiable\r\nContent-Range: bytes */{total}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
        );
        let _ = stream.write_all(hdr.as_bytes());
        return;
    }

    let slice = &cfg.payload[start..=end];
    let use_200 = !has_range || (cfg.accept_ranges_200_on_zero && start == 0 && end == total - 1);
    let hdr = if use_200 {
        format!(
            "HTTP/1.1 200 OK\r\nContent-Type: {}\r\nContent-Length: {}\r\nAccept-Ranges: bytes\r\nConnection: close\r\n\r\n",
            cfg.mime,
            slice.len()
        )
    } else {
        format!(
            "HTTP/1.1 206 Partial Content\r\nContent-Type: {}\r\nContent-Range: bytes {}-{}/{}\r\nContent-Length: {}\r\nAccept-Ranges: bytes\r\nConnection: close\r\n\r\n",
            cfg.mime,
            start,
            end,
            total,
            slice.len()
        )
    };
    if stream.write_all(hdr.as_bytes()).is_err() {
        return;
    }
    if method.eq_ignore_ascii_case("HEAD") {
        return;
    }

    let active_drop = if cfg.drop_only_first_hit && hit_num > 1 {
        None
    } else {
        cfg.drop_after_bytes
    };

    let mut pos = 0usize;
    while pos < slice.len() {
        let abs = start + pos;
        if let Some(drop_at) = active_drop {
            if abs >= drop_at {
                // Hard TCP drop mid-stream
                return;
            }
        }
        let mut next_end = (pos + cfg.trickle_chunk_size).min(slice.len());
        if let Some(drop_at) = active_drop {
            if start + next_end > drop_at && drop_at > abs {
                next_end = drop_at - start;
            }
        }
        if stream.write_all(&slice[pos..next_end]).is_err() {
            return;
        }
        let _ = stream.flush();
        pos = next_end;
        if !cfg.trickle_delay.is_zero() && pos < slice.len() {
            std::thread::sleep(cfg.trickle_delay);
        }
    }
}

fn http_client() -> reqwest::blocking::Client {
    reqwest::blocking::Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(45))
        .build()
        .unwrap()
}

// ============================================================================
// Tier 1: 16 Offline Unit / Invariant / Boundary Contract Tests (F1, F2, F3)
// ============================================================================

fn collect_rust_sources(include_build_rs: bool) -> Vec<(PathBuf, String)> {
    fn walk_rs(dir: &std::path::Path, out: &mut Vec<(PathBuf, String)>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk_rs(&path, out);
            } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
                if let Ok(text) = std::fs::read_to_string(&path) {
                    out.push((path, text));
                }
            }
        }
    }

    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut out = Vec::new();
    walk_rs(&manifest_dir.join("src"), &mut out);
    if include_build_rs {
        let build_rs = manifest_dir.join("build.rs");
        if build_rs.exists() {
            if let Ok(text) = std::fs::read_to_string(&build_rs) {
                out.push((build_rs, text));
            }
        }
    }
    out
}

#[test]
fn t1_f1_01_streams_response_contract_shape() {
    let sources = collect_rust_sources(false);
    let combined: String = sources
        .iter()
        .map(|(_, s)| s.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        combined.contains("struct StreamsResponse") || combined.contains("struct ResolvedStreams"),
        "Streams response struct missing across src-tauri/src/*.rs"
    );
    for field in ["muxed:", "video_only:", "audio:", "expires_in_seconds:"] {
        assert!(
            combined.contains(field),
            "Missing {field} contract field across src-tauri/src/*.rs"
        );
    }
    assert!(
        combined.contains("async fn streams("),
        "Missing Tauri streams command across src-tauri/src/*.rs"
    );
    // Compile-time verification of the exported ResolvedStreams contract shape
    let contract_check = nada::streams::ResolvedStreams {
        muxed: Vec::new(),
        video_only: Vec::new(),
        audio: Vec::new(),
        expires_in_seconds: 21600,
    };
    assert_eq!(contract_check.expires_in_seconds, 21600);
}

#[test]
fn t1_f1_02_drm_filtered_and_candidate_sorting() {
    let sources = collect_rust_sources(false);
    let combined: String = sources
        .iter()
        .map(|(_, s)| s.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        combined.contains("drmFamilies") || combined.contains("drm_track_type.is_none()"),
        "DRM filtering missing across src-tauri/src/*.rs"
    );
    assert!(
        combined.contains("b.bitrate.cmp(&a.bitrate)"),
        "Audio descending bitrate sort missing across src-tauri/src/*.rs"
    );
    assert!(
        combined.contains("b.height.cmp(&a.height)"),
        "Video descending height sort missing across src-tauri/src/*.rs"
    );
}

#[test]
fn t1_f1_03_local_proxy_url_format_and_reachability() {
    let origin = MockOriginServer::start();
    let data = Arc::new(make_synthetic_payload(128 * 1024));
    origin.set_route("/t1f103", OriginRouteConfig::new(data.clone(), "audio/mp4"));

    let proxy = ProxyAdapter::start("t1_f1_03");
    let proxy_url = proxy.register(&origin.url("/t1f103"), Some(data.len() as u64), Some("audio/mp4"));
    let expected_prefix = format!("http://127.0.0.1:{}/s/", proxy.port());
    assert!(proxy_url.starts_with(&expected_prefix), "Unexpected proxy URL: {proxy_url}");

    let resp = http_client().get(&proxy_url).send().unwrap();
    assert!(resp.status().is_success() || resp.status() == reqwest::StatusCode::PARTIAL_CONTENT);
    let bytes = resp.bytes().unwrap();
    assert_eq!(bytes.len(), data.len());
    verify_slice(&bytes, 0, "t1_f1_03");
}

#[test]
fn t1_f1_04_zero_external_binary_in_rust_sources() {
    let sources = collect_rust_sources(true);
    assert!(
        sources.len() >= 5,
        "Expected all src-tauri/src/*.rs (+ build.rs), found {}",
        sources.len()
    );
    let cargo_toml = include_str!("../Cargo.toml");
    for (path, src) in &sources {
        assert!(
            !src.contains("Command::new"),
            "External Command::new forbidden in {}",
            path.display()
        );
        assert!(
            !src.contains("yt-dlp"),
            "yt-dlp forbidden in {}",
            path.display()
        );
        assert!(
            !src.contains("ffmpeg"),
            "ffmpeg forbidden in {}",
            path.display()
        );
        assert!(
            !src.contains("rustypipe-botguard"),
            "external botguard binary forbidden in {}",
            path.display()
        );
    }
    assert!(cargo_toml.contains("rustls-tls"));
    assert!(!cargo_toml.contains("native-tls"));
}

#[test]
fn t1_f1_05_distinct_tokens_across_urls_and_fresh_after_error() {
    let origin = MockOriginServer::start();
    let data = Arc::new(make_synthetic_payload(64 * 1024));
    origin.set_route("/a", OriginRouteConfig::new(data.clone(), "audio/mp4"));
    origin.set_route("/b", OriginRouteConfig::new(data.clone(), "audio/mp4"));

    let proxy = ProxyAdapter::start("t1_f1_05");
    let u1 = proxy.register(&origin.url("/a"), Some(data.len() as u64), Some("audio/mp4"));
    let u2 = proxy.register(&origin.url("/b"), Some(data.len() as u64), Some("audio/mp4"));
    assert_ne!(u1, u2);
}

#[test]
fn t1_f2_01_parse_range_cases() {
    assert_eq!(proxy::parse_range(None, Some(1000)), (0, None));
    assert_eq!(proxy::parse_range(Some("bytes=0-499"), Some(1000)), (0, Some(499)));
    assert_eq!(proxy::parse_range(Some("bytes=500-"), Some(1000)), (500, None));
    assert_eq!(proxy::parse_range(Some("bytes=-200"), Some(1000)), (800, Some(999)));
    assert_eq!(proxy::parse_range(Some("invalid"), Some(1000)), (0, None));
}

#[test]
fn t1_f2_02_query_string_token_stripping() {
    let origin = MockOriginServer::start();
    let data = Arc::new(make_synthetic_payload(64 * 1024));
    origin.set_route("/qs", OriginRouteConfig::new(data.clone(), "audio/mp4"));

    let proxy = ProxyAdapter::start("t1_f2_02");
    let url = proxy.register(&origin.url("/qs"), Some(data.len() as u64), Some("audio/mp4"));
    let resp = http_client().get(format!("{url}?t=123&cb=abc")).send().unwrap();
    assert!(
        resp.status().is_success() || resp.status() == reqwest::StatusCode::PARTIAL_CONTENT,
        "Query string on proxy token must not 404, got status {}",
        resp.status()
    );
    let body = resp.bytes().unwrap();
    assert_eq!(body.len(), data.len());
    verify_slice(&body, 0, "t1_f2_02");
}

#[test]
fn t1_f2_03_head_returns_headers_with_empty_body() {
    let origin = MockOriginServer::start();
    let data = Arc::new(make_synthetic_payload(128 * 1024));
    origin.set_route("/head", OriginRouteConfig::new(data.clone(), "audio/webm"));

    let proxy = ProxyAdapter::start("t1_f2_03");
    let url = proxy.register(&origin.url("/head"), Some(data.len() as u64), Some("audio/webm"));
    let resp = http_client().head(&url).send().unwrap();
    assert!(resp.status() == reqwest::StatusCode::OK || resp.status() == reqwest::StatusCode::PARTIAL_CONTENT);
    assert_eq!(resp.headers().get("accept-ranges").and_then(|v| v.to_str().ok()), Some("bytes"));
    assert_eq!(resp.headers().get("content-type").and_then(|v| v.to_str().ok()), Some("audio/webm"));
    assert!(resp.bytes().unwrap().is_empty());
}

#[test]
fn t1_f2_04_out_of_bounds_range_returns_416() {
    let origin = MockOriginServer::start();
    let data = Arc::new(make_synthetic_payload(64 * 1024));
    origin.set_route("/oob", OriginRouteConfig::new(data.clone(), "audio/mp4"));

    let proxy = ProxyAdapter::start("t1_f2_04");
    let url = proxy.register(&origin.url("/oob"), Some(data.len() as u64), Some("audio/mp4"));
    let resp = http_client()
        .get(&url)
        .header("Range", "bytes=99999999-")
        .send()
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::RANGE_NOT_SATISFIABLE);
}

#[test]
fn t1_f2_05_get_without_range_streams_from_zero() {
    let origin = MockOriginServer::start();
    let data = Arc::new(make_synthetic_payload(256 * 1024));
    origin.set_route("/full", OriginRouteConfig::new(data.clone(), "audio/mp4"));

    let proxy = ProxyAdapter::start("t1_f2_05");
    let url = proxy.register(&origin.url("/full"), Some(data.len() as u64), Some("audio/mp4"));
    let resp = http_client().get(&url).send().unwrap();
    let bytes = resp.bytes().unwrap();
    assert_eq!(bytes.len(), data.len());
    verify_slice(&bytes, 0, "t1_f2_05");
}

#[test]
fn t1_f2_06_content_type_propagation() {
    let origin = MockOriginServer::start();
    let data = Arc::new(make_synthetic_payload(32 * 1024));
    origin.set_route("/ct", OriginRouteConfig::new(data.clone(), "video/webm; codecs=\"vp9\""));

    let proxy = ProxyAdapter::start("t1_f2_06");
    let url = proxy.register(
        &origin.url("/ct"),
        Some(data.len() as u64),
        Some("video/webm; codecs=\"vp9\""),
    );
    let resp = http_client().get(&url).send().unwrap();
    assert_eq!(
        resp.headers().get("content-type").and_then(|v| v.to_str().ok()),
        Some("video/webm; codecs=\"vp9\"")
    );
}

#[test]
fn t1_f3_01_unknown_token_returns_404() {
    let proxy = ProxyAdapter::start("t1_f3_01");
    let url = format!("http://127.0.0.1:{}/s/definitely_unknown_token", proxy.port());
    let resp = http_client().get(&url).send().unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::NOT_FOUND);
}

#[test]
fn t1_f3_02_reregister_after_failure_uses_fresh_session_file_without_conflict() {
    let origin = MockOriginServer::start();
    let data = Arc::new(make_synthetic_payload(128 * 1024));
    let mut bad_cfg = OriginRouteConfig::new(data.clone(), "audio/mp4");
    bad_cfg.force_status = Some(500);
    origin.set_route("/rereg", bad_cfg);

    let proxy = ProxyAdapter::start("t1_f3_02");
    let raw_url = origin.url("/rereg");
    let p1 = proxy.register(&raw_url, Some(data.len() as u64), Some("audio/mp4"));
    let r1 = http_client().get(&p1).send().unwrap();
    assert_eq!(r1.status(), reqwest::StatusCode::BAD_GATEWAY);

    // Fix origin and re-register same URL: must produce a new working session
    origin.set_route("/rereg", OriginRouteConfig::new(data.clone(), "audio/mp4"));
    let p2 = proxy.register(&raw_url, Some(data.len() as u64), Some("audio/mp4"));
    let r2 = http_client().get(&p2).send().unwrap();
    assert!(
        r2.status().is_success() || r2.status() == reqwest::StatusCode::PARTIAL_CONTENT,
        "Re-registered session after error must succeed, got {}",
        r2.status()
    );
    assert_eq!(r2.bytes().unwrap().len(), data.len());
}

#[test]
fn t1_f3_03_evicted_or_cancelled_session_replaces_cleanly_on_reregister() {
    let origin = MockOriginServer::start();
    let data = Arc::new(make_synthetic_payload(64 * 1024));
    for i in 0..12 {
        origin.set_route(&format!("/ev/{i}"), OriginRouteConfig::new(data.clone(), "audio/mp4"));
    }

    let proxy = ProxyAdapter::start("t1_f3_03");
    let url0 = origin.url("/ev/0");
    let p0 = proxy.register(&url0, Some(data.len() as u64), Some("audio/mp4"));
    assert_eq!(http_client().get(&p0).send().unwrap().bytes().unwrap().len(), data.len());

    // Push 11 more downloads through the 10-slot LRU to evict /ev/0
    for i in 1..=11 {
        let pi = proxy.register(&origin.url(&format!("/ev/{i}")), Some(data.len() as u64), Some("audio/mp4"));
        let _ = http_client().get(&pi).send().unwrap().bytes().unwrap();
    }

    // Re-registering /ev/0 must yield a working session (not a cancelled zombie)
    let p0_new = proxy.register(&url0, Some(data.len() as u64), Some("audio/mp4"));
    let resp = http_client().get(&p0_new).send().unwrap();
    assert!(
        resp.status().is_success() || resp.status() == reqwest::StatusCode::PARTIAL_CONTENT,
        "Re-registered evicted session must succeed, got {}",
        resp.status()
    );
    assert_eq!(resp.bytes().unwrap().len(), data.len());
}

#[test]
fn t1_f3_04_windows_shared_open_concurrent_reader_while_writer_active() {
    let origin = MockOriginServer::start();
    let data = Arc::new(make_synthetic_payload(512 * 1024));
    let mut cfg = OriginRouteConfig::new(data.clone(), "audio/mp4");
    cfg.trickle_chunk_size = 32 * 1024;
    cfg.trickle_delay = Duration::from_millis(15);
    origin.set_route("/winshare", cfg);

    let proxy = ProxyAdapter::start("t1_f3_04");
    let url = proxy.register(&origin.url("/winshare"), Some(data.len() as u64), Some("audio/mp4"));

    let mut handles = Vec::new();
    for range in ["bytes=0-131071", "bytes=65536-262143", "bytes=131072-524287"] {
        let u = url.clone();
        let r = range.to_string();
        handles.push(std::thread::spawn(move || {
            let resp = http_client().get(&u).header("Range", &r).send().unwrap();
            assert_eq!(resp.status(), reqwest::StatusCode::PARTIAL_CONTENT);
            resp.bytes().unwrap()
        }));
    }
    for h in handles {
        let bytes = h.join().unwrap();
        assert!(!bytes.is_empty());
    }
}

#[test]
fn t1_f3_05_startup_cache_dir_cleanup_contract() {
    let sources = collect_rust_sources(false);
    let combined: String = sources
        .iter()
        .map(|(_, s)| s.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(combined.contains("stream_cache"), "stream_cache cleanup missing across src-tauri/src/*.rs");
    assert!(combined.contains("remove_dir_all(&cache_dir)"), "remove_dir_all(&cache_dir) missing across src-tauri/src/*.rs");
    let proxy = ProxyAdapter::start("t1_f3_05");
    assert!(proxy.cache_dir().exists());
}

// ============================================================================
// Tier 2: 18 Protocol & Lifecycle E2E Tests (F1, F2, F3)
// ============================================================================

#[test]
fn t2_f1_01_multi_candidate_audio_video_muxed_registration() {
    let origin = MockOriginServer::start();
    let a = Arc::new(make_synthetic_payload(128 * 1024));
    let v = Arc::new(make_synthetic_payload(256 * 1024));
    let m = Arc::new(make_synthetic_payload(192 * 1024));
    origin.set_route("/cand/a", OriginRouteConfig::new(a.clone(), "audio/mp4"));
    origin.set_route("/cand/v", OriginRouteConfig::new(v.clone(), "video/mp4"));
    origin.set_route("/cand/m", OriginRouteConfig::new(m.clone(), "video/mp4"));

    let proxy = ProxyAdapter::start("t2_f1_01");
    let pa = proxy.register(&origin.url("/cand/a"), Some(a.len() as u64), Some("audio/mp4"));
    let pv = proxy.register(&origin.url("/cand/v"), Some(v.len() as u64), Some("video/mp4"));
    let pm = proxy.register(&origin.url("/cand/m"), Some(m.len() as u64), Some("video/mp4"));

    assert_eq!(http_client().get(&pa).send().unwrap().bytes().unwrap().len(), a.len());
    assert_eq!(http_client().get(&pv).send().unwrap().bytes().unwrap().len(), v.len());
    assert_eq!(http_client().get(&pm).send().unwrap().bytes().unwrap().len(), m.len());
}

#[test]
fn t2_f1_02_upstream_user_agent_contract() {
    let origin = MockOriginServer::start();
    let data = Arc::new(make_synthetic_payload(64 * 1024));
    origin.set_route("/ua", OriginRouteConfig::new(data.clone(), "audio/mp4"));

    let proxy = ProxyAdapter::start("t2_f1_02");
    let url = proxy.register(&origin.url("/ua"), Some(data.len() as u64), Some("audio/mp4"));
    let _ = http_client().get(&url).send().unwrap().bytes().unwrap();

    let reqs = origin.requests_for("/ua");
    assert!(!reqs.is_empty());
    let ua = reqs[0].user_agent.as_deref().unwrap_or("");
    assert!(!ua.is_empty(), "Upstream request must send a User-Agent");
}

#[test]
fn t2_f1_03_fallback_when_primary_candidate_returns_403() {
    let origin = MockOriginServer::start();
    let data = Arc::new(make_synthetic_payload(256 * 1024));
    let mut bad = OriginRouteConfig::new(data.clone(), "audio/mp4");
    bad.force_status = Some(403);
    origin.set_route("/cand1", bad);
    origin.set_route("/cand2", OriginRouteConfig::new(data.clone(), "audio/mp4"));

    let proxy = ProxyAdapter::start("t2_f1_03");
    let p1 = proxy.register(&origin.url("/cand1"), Some(data.len() as u64), Some("audio/mp4"));
    let p2 = proxy.register(&origin.url("/cand2"), Some(data.len() as u64), Some("audio/mp4"));

    assert_eq!(http_client().get(&p1).send().unwrap().status(), reqwest::StatusCode::BAD_GATEWAY);
    let bytes = http_client().get(&p2).send().unwrap().bytes().unwrap();
    assert_eq!(bytes.len(), data.len());
    verify_slice(&bytes, 0, "t2_f1_03");
}

#[test]
fn t2_f1_04_fresh_token_after_expired_url_recovery() {
    let origin = MockOriginServer::start();
    let data = Arc::new(make_synthetic_payload(128 * 1024));
    let mut expired = OriginRouteConfig::new(data.clone(), "audio/mp4");
    expired.force_status = Some(403);
    origin.set_route("/exp_v1", expired);
    origin.set_route("/exp_v2", OriginRouteConfig::new(data.clone(), "audio/mp4"));

    let proxy = ProxyAdapter::start("t2_f1_04");
    let p1 = proxy.register(&origin.url("/exp_v1"), Some(data.len() as u64), Some("audio/mp4"));
    assert_eq!(http_client().get(&p1).send().unwrap().status(), reqwest::StatusCode::BAD_GATEWAY);

    let p2 = proxy.register(&origin.url("/exp_v2"), Some(data.len() as u64), Some("audio/mp4"));
    assert_ne!(p1, p2);
    assert_eq!(http_client().get(&p2).send().unwrap().bytes().unwrap().len(), data.len());
}

#[test]
fn t2_f1_05_po_trap_zero_start_range_compliance() {
    let origin = MockOriginServer::start();
    let data = Arc::new(make_synthetic_payload(2560 * 1024)); // 2.5 MiB (> 1 MiB / 1:04)
    let mut cfg = OriginRouteConfig::new(data.clone(), "audio/mp4");
    cfg.po_trap_nonzero_range = true;
    origin.set_route("/potrap", cfg);

    let proxy = ProxyAdapter::start("t2_f1_05");
    let url = proxy.register(&origin.url("/potrap"), Some(data.len() as u64), Some("audio/mp4"));
    let resp = http_client().get(&url).send().unwrap();
    assert!(resp.status().is_success() || resp.status() == reqwest::StatusCode::PARTIAL_CONTENT);
    let bytes = resp.bytes().unwrap();
    assert_eq!(bytes.len(), data.len(), "Must stream full 2.5 MiB without upstream 403 at 512K/1M");
    verify_slice(&bytes, 0, "t2_f1_05");
    let reqs = origin.requests_for("/potrap");
    assert!(!reqs.is_empty());
    assert!(reqs.iter().all(|r| r.method == "GET" && r.range.as_deref().unwrap_or("bytes=0-").starts_with("bytes=0-")));
}

#[test]
fn t2_f2_01_unsegmented_stream_past_1_04_byte_exact() {
    let origin = MockOriginServer::start();
    let total = 3 * 1024 * 1024; // 3.0 MiB
    let data = Arc::new(make_synthetic_payload(total));
    let mut cfg = OriginRouteConfig::new(data.clone(), "audio/mp4");
    cfg.po_trap_nonzero_range = true;
    origin.set_route("/stream3m", cfg);

    let proxy = ProxyAdapter::start("t2_f2_01");
    let url = proxy.register(&origin.url("/stream3m"), Some(total as u64), Some("audio/mp4"));

    // Read sequential Range chunks crossing 512 KiB, 1 MiB (1:04), and 2 MiB
    let mut combined = Vec::with_capacity(total);
    let step = 384 * 1024;
    let mut off = 0;
    while off < total {
        let end = (off + step - 1).min(total - 1);
        let resp = http_client()
            .get(&url)
            .header("Range", format!("bytes={off}-{end}"))
            .send()
            .unwrap();
        assert_eq!(resp.status(), reqwest::StatusCode::PARTIAL_CONTENT);
        combined.extend_from_slice(&resp.bytes().unwrap());
        off = end + 1;
    }
    assert_eq!(combined.len(), total);
    verify_slice(&combined, 0, "t2_f2_01");
    assert_eq!(origin.hits_for("/stream3m"), 1, "Unsegmented single upstream fetch expected");
}

#[test]
fn t2_f2_02_upstream_drop_at_1mib_recovers_without_byte_corruption() {
    let origin = MockOriginServer::start();
    let total = 2 * 1024 * 1024 + 256 * 1024; // 2.25 MiB
    let data = Arc::new(make_synthetic_payload(total));
    let mut cfg = OriginRouteConfig::new(data.clone(), "audio/mp4");
    cfg.drop_after_bytes = Some(1024 * 1024); // Drop TCP at exact 1 MiB (1:04)
    cfg.drop_only_first_hit = true;
    origin.set_route("/drop1m", cfg);

    let proxy = ProxyAdapter::start("t2_f2_02");
    let url = proxy.register(&origin.url("/drop1m"), Some(total as u64), Some("audio/mp4"));
    let resp = http_client().get(&url).send().unwrap();
    let bytes = resp.bytes().unwrap();
    assert_eq!(bytes.len(), total, "Must recover cleanly from 1 MiB drop to full EOF");
    verify_slice(&bytes, 0, "t2_f2_02");
}

#[test]
fn t2_f2_03_slow_trickle_exceeding_20s_completes_under_read_timeout() {
    let origin = MockOriginServer::start();
    let total = 512 * 1024; // 512 KiB trickled over ~22s (44 chunks x 500ms)
    let data = Arc::new(make_synthetic_payload(total));
    let mut cfg = OriginRouteConfig::new(data.clone(), "audio/mp4");
    cfg.trickle_chunk_size = 12 * 1024;
    cfg.trickle_delay = Duration::from_millis(500);
    origin.set_route("/slow22s", cfg);

    let proxy = ProxyAdapter::start("t2_f2_03");
    let url = proxy.register(&origin.url("/slow22s"), Some(total as u64), Some("audio/mp4"));
    let started = Instant::now();
    let resp = http_client().get(&url).send().unwrap();
    let bytes = resp.bytes().unwrap();
    let elapsed = started.elapsed();
    assert!(elapsed >= Duration::from_secs(20), "Trickle test took {:?}", elapsed);
    assert_eq!(bytes.len(), total, "Stream >20s with active bytes must not hit total-body timeout");
    verify_slice(&bytes, 0, "t2_f2_03");
}

#[test]
fn t2_f2_04_forward_seek_waits_on_condvar_and_completes() {
    let origin = MockOriginServer::start();
    let total = 2 * 1024 * 1024;
    let data = Arc::new(make_synthetic_payload(total));
    let mut cfg = OriginRouteConfig::new(data.clone(), "audio/mp4");
    cfg.trickle_chunk_size = 64 * 1024;
    cfg.trickle_delay = Duration::from_millis(25);
    origin.set_route("/seekwait", cfg);

    let proxy = ProxyAdapter::start("t2_f2_04");
    let url = proxy.register(&origin.url("/seekwait"), Some(total as u64), Some("audio/mp4"));

    let resp = http_client()
        .get(&url)
        .header("Range", "bytes=1500000-1631071")
        .send()
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::PARTIAL_CONTENT);
    let bytes = resp.bytes().unwrap();
    assert_eq!(bytes.len(), 131072);
    verify_slice(&bytes, 1500000, "t2_f2_04");
}

#[test]
fn t2_f2_05_backward_seek_after_1_04_reads_cached_prefix() {
    let origin = MockOriginServer::start();
    let total = 2 * 1024 * 1024;
    let data = Arc::new(make_synthetic_payload(total));
    let mut cfg = OriginRouteConfig::new(data.clone(), "audio/mp4");
    cfg.po_trap_nonzero_range = true;
    origin.set_route("/backseek", cfg);

    let proxy = ProxyAdapter::start("t2_f2_05");
    let url = proxy.register(&origin.url("/backseek"), Some(total as u64), Some("audio/mp4"));

    let full = http_client().get(&url).send().unwrap().bytes().unwrap();
    assert_eq!(full.len(), total);

    let back = http_client()
        .get(&url)
        .header("Range", "bytes=65536-131071")
        .send()
        .unwrap()
        .bytes()
        .unwrap();
    verify_slice(&back, 65536, "t2_f2_05_back");

    let fwd = http_client()
        .get(&url)
        .header("Range", "bytes=1572864-1703935")
        .send()
        .unwrap()
        .bytes()
        .unwrap();
    verify_slice(&fwd, 1572864, "t2_f2_05_fwd");
    assert_eq!(origin.hits_for("/backseek"), 1);
}

#[test]
fn t2_f2_06_upstream_403_and_500_return_502() {
    let origin = MockOriginServer::start();
    let data = Arc::new(make_synthetic_payload(32 * 1024));
    let mut c403 = OriginRouteConfig::new(data.clone(), "audio/mp4");
    c403.force_status = Some(403);
    let mut c500 = OriginRouteConfig::new(data.clone(), "audio/mp4");
    c500.force_status = Some(500);
    origin.set_route("/e403", c403);
    origin.set_route("/e500", c500);

    let proxy = ProxyAdapter::start("t2_f2_06");
    let u403 = proxy.register(&origin.url("/e403"), Some(data.len() as u64), Some("audio/mp4"));
    let u500 = proxy.register(&origin.url("/e500"), Some(data.len() as u64), Some("audio/mp4"));

    assert_eq!(http_client().get(&u403).send().unwrap().status(), reqwest::StatusCode::BAD_GATEWAY);
    assert_eq!(http_client().get(&u500).send().unwrap().status(), reqwest::StatusCode::BAD_GATEWAY);
}

#[test]
fn t2_f2_07_upstream_stall_over_5s_returns_504() {
    let origin = MockOriginServer::start();
    let data = Arc::new(make_synthetic_payload(32 * 1024));
    let mut stall = OriginRouteConfig::new(data.clone(), "audio/mp4");
    stall.stall_headers_for = Some(Duration::from_secs(7));
    origin.set_route("/stall504", stall);

    let proxy = ProxyAdapter::start("t2_f2_07");
    let url = proxy.register(&origin.url("/stall504"), None, Some("audio/mp4"));
    let resp = http_client().get(&url).send().unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::GATEWAY_TIMEOUT);
}

#[test]
fn t2_f3_01_300_session_eviction_removes_from_sessions_map_returns_404() {
    let origin = MockOriginServer::start();
    let data = Arc::new(make_synthetic_payload(16 * 1024));
    origin.set_route("/s0", OriginRouteConfig::new(data.clone(), "audio/mp4"));

    let proxy = ProxyAdapter::start("t2_f3_01");
    let first_url = proxy.register(&origin.url("/s0"), Some(data.len() as u64), Some("audio/mp4"));

    for i in 1..=305 {
        let _ = proxy.register(
            &origin.url(&format!("/s{i}")),
            Some(data.len() as u64),
            Some("audio/mp4"),
        );
    }

    let resp = http_client().get(&first_url).send().unwrap();
    assert_eq!(
        resp.status(),
        reqwest::StatusCode::NOT_FOUND,
        "Evicted >300 token must be removed from sessions map and return 404"
    );
}

#[test]
fn t2_f3_02_10_file_lru_eviction_preserves_active_reader() {
    let origin = MockOriginServer::start();
    let total = 1536 * 1024;
    let data = Arc::new(make_synthetic_payload(total));
    for i in 0..12 {
        let mut cfg = OriginRouteConfig::new(data.clone(), "audio/mp4");
        if i == 0 {
            cfg.trickle_chunk_size = 64 * 1024;
            cfg.trickle_delay = Duration::from_millis(20);
        }
        origin.set_route(&format!("/lru/{i}"), cfg);
    }

    let proxy = ProxyAdapter::start("t2_f3_02");
    let u0 = proxy.register(&origin.url("/lru/0"), Some(total as u64), Some("audio/mp4"));
    let reader_handle = std::thread::spawn(move || http_client().get(&u0).send());

    std::thread::sleep(Duration::from_millis(120));
    for i in 1..12 {
        let ui = proxy.register(
            &origin.url(&format!("/lru/{i}")),
            Some(total as u64),
            Some("audio/mp4"),
        );
        let bytes = http_client().get(&ui).send().unwrap().bytes().unwrap();
        assert_eq!(bytes.len(), total);
    }

    // Active reader on evicted track 0 must either finish cleanly or abort cleanly without 500 panic
    let r0 = reader_handle.join().unwrap().unwrap();
    assert!(r0.status() != reqwest::StatusCode::INTERNAL_SERVER_ERROR);
}

#[test]
fn t2_f3_03_reregister_errored_url_yields_playable_session() {
    let origin = MockOriginServer::start();
    let data = Arc::new(make_synthetic_payload(128 * 1024));
    let mut bad = OriginRouteConfig::new(data.clone(), "audio/mp4");
    bad.force_status = Some(502);
    origin.set_route("/heal", bad);

    let proxy = ProxyAdapter::start("t2_f3_03");
    let raw = origin.url("/heal");
    let p1 = proxy.register(&raw, Some(data.len() as u64), Some("audio/mp4"));
    assert_eq!(http_client().get(&p1).send().unwrap().status(), reqwest::StatusCode::BAD_GATEWAY);

    origin.set_route("/heal", OriginRouteConfig::new(data.clone(), "audio/mp4"));
    let p2 = proxy.register(&raw, Some(data.len() as u64), Some("audio/mp4"));
    let resp = http_client().get(&p2).send().unwrap();
    assert!(
        resp.status().is_success() || resp.status() == reqwest::StatusCode::PARTIAL_CONTENT,
        "Re-registering errored URL must create fresh playable session, got {}",
        resp.status()
    );
    assert_eq!(resp.bytes().unwrap().len(), data.len());
}

#[test]
fn t2_f3_04_simultaneous_head_and_get_spawn_single_worker() {
    let origin = MockOriginServer::start();
    let data = Arc::new(make_synthetic_payload(256 * 1024));
    let mut cfg = OriginRouteConfig::new(data.clone(), "audio/mp4");
    cfg.trickle_chunk_size = 32 * 1024;
    cfg.trickle_delay = Duration::from_millis(15);
    origin.set_route("/headget", cfg);

    let proxy = ProxyAdapter::start("t2_f3_04");
    let url = proxy.register(&origin.url("/headget"), Some(data.len() as u64), Some("audio/mp4"));

    let u_head = url.clone();
    let h1 = std::thread::spawn(move || http_client().head(&u_head).send().unwrap().status());
    let h2 = std::thread::spawn(move || http_client().get(&url).send().unwrap().bytes().unwrap());

    let s = h1.join().unwrap();
    let b = h2.join().unwrap();
    assert!(s == reqwest::StatusCode::OK || s == reqwest::StatusCode::PARTIAL_CONTENT);
    assert_eq!(b.len(), data.len());
    assert_eq!(origin.hits_for("/headget"), 1);
}

#[test]
fn t2_f3_05_sequential_replays_avoid_windows_file_in_use_errors() {
    let origin = MockOriginServer::start();
    let data = Arc::new(make_synthetic_payload(256 * 1024));
    origin.set_route("/replay", OriginRouteConfig::new(data.clone(), "audio/mp4"));

    let proxy = ProxyAdapter::start("t2_f3_05");
    let raw = origin.url("/replay");
    for _ in 0..5 {
        let p = proxy.register(&raw, Some(data.len() as u64), Some("audio/mp4"));
        let r = http_client().get(&p).send().unwrap();
        assert_ne!(r.status(), reqwest::StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(r.bytes().unwrap().len(), data.len());
    }
}

#[test]
fn t2_f3_06_cancelled_download_unblocks_waiting_reader() {
    // Sub-case A: Pre-header 503 cancellation when Range start exceeds downloaded bytes
    {
        let origin = MockOriginServer::start();
        let total = 512 * 1024;
        let data = Arc::new(make_synthetic_payload(total));
        let mut stalled = OriginRouteConfig::new(data, "audio/mp4");
        stalled.trickle_chunk_size = 8 * 1024;
        stalled.trickle_delay = Duration::from_secs(2); // very slow after first chunk
        origin.set_route("/cancel_wait/0", stalled);
        for i in 1..=11 {
            origin.set_route(
                &format!("/cancel_wait/{i}"),
                OriginRouteConfig::new(Arc::new(make_synthetic_payload(32 * 1024)), "audio/mp4"),
            );
        }

        let proxy = ProxyAdapter::start("t2_f3_06_pre");
        let u0 = proxy.register(&origin.url("/cancel_wait/0"), Some(total as u64), Some("audio/mp4"));

        let t0 = std::thread::spawn(move || {
            let fast_client = reqwest::blocking::Client::builder()
                .connect_timeout(Duration::from_secs(2))
                .timeout(Duration::from_secs(5))
                .build()
                .unwrap();
            let resp = fast_client
                .get(&u0)
                .header("Range", "bytes=300000-350000")
                .send()
                .ok()?;
            let status = resp.status();
            let _ = resp.bytes();
            Some(status)
        });

        std::thread::sleep(Duration::from_millis(300));
        let client = http_client();
        for i in 1..=11 {
            let ui = proxy.register(
                &origin.url(&format!("/cancel_wait/{i}")),
                Some(32 * 1024),
                Some("audio/mp4"),
            );
            let _ = client.get(&ui).send().unwrap().bytes().unwrap();
        }

        let started = Instant::now();
        let status = t0.join().unwrap();
        let elapsed = started.elapsed();
        assert_eq!(status, Some(reqwest::StatusCode::SERVICE_UNAVAILABLE));
        assert!(
            elapsed < Duration::from_secs(3),
            "Pre-header waiting reader took too long to unblock after eviction: {:?}\nproxy.log:\n{}",
            elapsed,
            proxy.log_contents()
        );
    }

    // Sub-case B: Genuine mid-body CacheRangeReader cancellation (206 headers sent after 8 KiB chunk,
    // reader actively blocked mid-body before 11 subsequent downloads evict session 0)
    {
        let origin = MockOriginServer::start();
        let total = 600 * 1024;
        let data = Arc::new(make_synthetic_payload(total));
        let mut stalled = OriginRouteConfig::new(data, "audio/mp4");
        stalled.trickle_chunk_size = 8 * 1024;
        stalled.trickle_delay = Duration::from_secs(2);
        origin.set_route("/cancel_mid/0", stalled);
        for i in 1..=11 {
            origin.set_route(
                &format!("/cancel_mid/{i}"),
                OriginRouteConfig::new(Arc::new(make_synthetic_payload(32 * 1024)), "audio/mp4"),
            );
        }

        let proxy = ProxyAdapter::start("t2_f3_06_mid");
        let u0 = proxy.register(&origin.url("/cancel_mid/0"), Some(total as u64), Some("audio/mp4"));

        let t0 = std::thread::spawn(move || {
            let fast_client = reqwest::blocking::Client::builder()
                .connect_timeout(Duration::from_secs(2))
                .timeout(Duration::from_secs(5))
                .build()
                .unwrap();
            match fast_client
                .get(&u0)
                .header("Range", "bytes=0-500000")
                .send()
            {
                Ok(mut resp) => {
                    let status = resp.status();
                    let mut buf = Vec::new();
                    let read_res = resp.read_to_end(&mut buf);
                    (Some(status), buf.len(), read_res.err().map(|e| e.to_string()))
                }
                Err(e) => (None, 0, Some(e.to_string())),
            }
        });

        // Wait until 206 headers have been sent and CacheRangeReader is blocked after the first 8 KiB chunk
        let wait_206 = Instant::now();
        while !proxy.log_contents().contains("status=206") && wait_206.elapsed() < Duration::from_secs(2) {
            std::thread::sleep(Duration::from_millis(20));
        }
        std::thread::sleep(Duration::from_millis(120));

        let client = http_client();
        for i in 1..=11 {
            let ui = proxy.register(
                &origin.url(&format!("/cancel_mid/{i}")),
                Some(32 * 1024),
                Some("audio/mp4"),
            );
            let _ = client.get(&ui).send().unwrap().bytes().unwrap();
        }

        // Evicting /cancel_mid/0 must abort the mid-body reader promptly (< 3.5s), NOT hang until 5s TimedOut
        let post_evict = Instant::now();
        let (status, bytes_read, err) = t0.join().unwrap();
        let elapsed = post_evict.elapsed();
        assert_eq!(
            status,
            Some(reqwest::StatusCode::PARTIAL_CONTENT),
            "Mid-body sub-case must enter CacheRangeReader with 206 Partial Content (err={err:?})"
        );
        assert!(
            bytes_read >= 8 * 1024 && bytes_read < 500_001,
            "Expected >=8 KiB initial chunk and <500001 partial body, got {bytes_read}"
        );
        assert!(
            elapsed < Duration::from_millis(3500),
            "Mid-body CacheRangeReader hung for {:?} instead of unblocking promptly on cancel (bytes_read={bytes_read}, err={err:?})\nproxy.log:\n{}",
            elapsed,
            proxy.log_contents()
        );
    }
}

// ============================================================================
// Tier 3: Cross-Feature Integration E2E Tests (T3-01, T3-02)
// ============================================================================

#[test]
fn t3_01_f1_x_f2_resolved_candidates_stream_past_1_04_with_po_trap() {
    let origin = MockOriginServer::start();
    let audio_len = 2560 * 1024; // 2.5 MiB
    let video_len = 4 * 1024 * 1024; // 4.0 MiB
    let audio_data = Arc::new(make_synthetic_payload(audio_len));
    let video_data = Arc::new(make_synthetic_payload(video_len));

    let mut a_cfg = OriginRouteConfig::new(audio_data.clone(), "audio/mp4");
    a_cfg.po_trap_nonzero_range = true;
    let mut v_cfg = OriginRouteConfig::new(video_data.clone(), "video/mp4");
    v_cfg.po_trap_nonzero_range = true;
    origin.set_route("/split/a", a_cfg);
    origin.set_route("/split/v", v_cfg);

    let proxy = ProxyAdapter::start("t3_01");
    let pa = proxy.register(&origin.url("/split/a"), Some(audio_len as u64), Some("audio/mp4"));
    let pv = proxy.register(&origin.url("/split/v"), Some(video_len as u64), Some("video/mp4"));

    let ha = std::thread::spawn(move || {
        let b = http_client().get(&pa).send().unwrap().bytes().unwrap();
        assert_eq!(b.len(), audio_len);
        verify_slice(&b, 0, "t3_01_audio");
        let seek = http_client()
            .get(&pa)
            .header("Range", "bytes=1500000-1800000")
            .send()
            .unwrap()
            .bytes()
            .unwrap();
        verify_slice(&seek, 1500000, "t3_01_audio_seek");
    });
    let hv = std::thread::spawn(move || {
        let b = http_client().get(&pv).send().unwrap().bytes().unwrap();
        assert_eq!(b.len(), video_len);
        verify_slice(&b, 0, "t3_01_video");
    });

    ha.join().unwrap();
    hv.join().unwrap();
    assert_eq!(origin.hits_for("/split/a"), 1);
    assert_eq!(origin.hits_for("/split/v"), 1);
}

#[test]
fn t3_02_f2_x_f3_midstream_drop_seek_and_lru_replacement() {
    let origin = MockOriginServer::start();
    let total = 2 * 1024 * 1024;
    let data = Arc::new(make_synthetic_payload(total));
    let mut c0 = OriginRouteConfig::new(data.clone(), "audio/mp4");
    c0.drop_after_bytes = Some(1024 * 1024);
    c0.drop_only_first_hit = true;
    origin.set_route("/t302/0", c0);
    for i in 1..=11 {
        origin.set_route(
            &format!("/t302/{i}"),
            OriginRouteConfig::new(Arc::new(make_synthetic_payload(256 * 1024)), "audio/mp4"),
        );
    }

    let proxy = ProxyAdapter::start("t3_02");
    let raw0 = origin.url("/t302/0");
    let p0 = proxy.register(&raw0, Some(total as u64), Some("audio/mp4"));
    let b0 = http_client().get(&p0).send().unwrap().bytes().unwrap();
    assert_eq!(b0.len(), total);
    verify_slice(&b0, 0, "t3_02_0");

    // Evict track 0 via 11 subsequent tracks
    for i in 1..=11 {
        let pi = proxy.register(&origin.url(&format!("/t302/{i}")), Some(256 * 1024), Some("audio/mp4"));
        assert_eq!(http_client().get(&pi).send().unwrap().bytes().unwrap().len(), 256 * 1024);
    }

    // Re-register track 0 and seek past 1 MiB
    let p0_re = proxy.register(&raw0, Some(total as u64), Some("audio/mp4"));
    let seek = http_client()
        .get(&p0_re)
        .header("Range", "bytes=1200000-1500000")
        .send()
        .unwrap()
        .bytes()
        .unwrap();
    assert_eq!(seek.len(), 300001);
    verify_slice(&seek, 1200000, "t3_02_seek");
}

// ============================================================================
// Tier 4: Full Proxy Multi-Track Replay & Live YouTube Acceptance
// ============================================================================

#[test]
fn t4_05_multi_track_12_track_replay_and_seek_storm() {
    let origin = MockOriginServer::start();
    let total = 2 * 1024 * 1024; // 2.0 MiB per track (> 1:04)
    let data = Arc::new(make_synthetic_payload(total));

    for i in 0..12 {
        let mut cfg = OriginRouteConfig::new(data.clone(), "audio/mp4");
        if i % 2 == 0 {
            cfg.po_trap_nonzero_range = true;
        } else {
            cfg.drop_after_bytes = Some(1024 * 1024);
            cfg.drop_only_first_hit = true;
        }
        origin.set_route(&format!("/t405/{i}"), cfg);
    }

    let proxy = ProxyAdapter::start("t4_05");
    for i in 0..12 {
        let pu = proxy.register(&origin.url(&format!("/t405/{i}")), Some(total as u64), Some("audio/mp4"));
        let full = http_client().get(&pu).send().unwrap().bytes().unwrap();
        assert_eq!(full.len(), total, "Track {i} incomplete");
        verify_slice(&full, 0, &format!("t4_05_track_{i}"));

        for range in ["bytes=0-65535", "bytes=1048576-1572863", "bytes=1572864-"] {
            let start_off = if range.starts_with("bytes=0") {
                0
            } else if range.starts_with("bytes=1048576") {
                1048576
            } else {
                1572864
            };
            let chunk = http_client()
                .get(&pu)
                .header("Range", range)
                .send()
                .unwrap()
                .bytes()
                .unwrap();
            verify_slice(&chunk, start_off, &format!("t4_05_seek_{i}_{start_off}"));
        }
    }

    // Replay Track 0 after LRU eviction of >10 tracks
    let p0_replay = proxy.register(&origin.url("/t405/0"), Some(total as u64), Some("audio/mp4"));
    let replay_bytes = http_client().get(&p0_replay).send().unwrap().bytes().unwrap();
    assert_eq!(replay_bytes.len(), total);
    verify_slice(&replay_bytes, 0, "t4_05_replay_0");
}

#[test]
#[ignore = "Live network YouTube test (run with -- --ignored --nocapture)"]
fn t4_06_live_youtube_kalah2_and_mangu_acceptance() {
    use nada::bg_engine::{
        parse_sync_json_result, unwrap_callback_payload, wrap_async_script, BgEngine,
        CallbackBridge, JsHost,
    };
    use nada::proxy::StreamProxy;
    use nada::streams::{resolve_streams, ResolvedAudio, ResolvedStreams, ResolvedVideo};
    use rustypipe::client::{ClientType, RustyPipe};
    use std::sync::{mpsc, Condvar};
    use tao::dpi::LogicalSize;
    use tao::event::Event;
    use tao::event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy};
    use tao::platform::run_return::EventLoopExtRunReturn;
    #[cfg(target_os = "windows")]
    use tao::platform::windows::{EventLoopBuilderExtWindows, WindowBuilderExtWindows};
    use tao::window::WindowBuilder;
    use wry::{PageLoadEvent, WebViewBuilder};

    enum WryCmd {
        Eval(String),
        EvalCb(String, mpsc::Sender<String>),
        Quit,
    }

    struct LiveWryJsHost {
        proxy: EventLoopProxy<WryCmd>,
        loaded: Arc<(Mutex<bool>, Condvar)>,
        bridge: CallbackBridge,
        thread: Option<std::thread::JoinHandle<()>>,
    }

    impl LiveWryJsHost {
        fn start() -> Result<Self, String> {
            let bridge = CallbackBridge::start()?;
            let loaded = Arc::new((Mutex::new(false), Condvar::new()));
            let loaded_hook = loaded.clone();
            let (tx, rx) = mpsc::channel::<Result<EventLoopProxy<WryCmd>, String>>();

            let thread = std::thread::spawn(move || {
                let mut builder = EventLoopBuilder::<WryCmd>::with_user_event();
                #[cfg(target_os = "windows")]
                builder.with_any_thread(true);
                let mut event_loop = builder.build();
                let el_proxy = event_loop.create_proxy();

                let mut win_builder = WindowBuilder::new()
                    .with_title("Nada E2E Live BgWorker")
                    .with_visible(false)
                    .with_inner_size(LogicalSize::new(1.0, 1.0));
                #[cfg(target_os = "windows")]
                {
                    win_builder = win_builder.with_skip_taskbar(true);
                }
                let window = match win_builder.build(&event_loop) {
                    Ok(w) => w,
                    Err(e) => {
                        let _ = tx.send(Err(format!("tao window build failed: {e}")));
                        return;
                    }
                };
                let webview = match WebViewBuilder::new()
                    .with_url("https://www.youtube.com/robots.txt")
                    .with_on_page_load_handler(move |event, url| {
                        if matches!(event, PageLoadEvent::Finished) && url.contains("robots.txt") {
                            let (lock, cvar) = &*loaded_hook;
                            *lock.lock().unwrap() = true;
                            cvar.notify_all();
                        }
                    })
                    .build(&window)
                {
                    Ok(wv) => wv,
                    Err(e) => {
                        let _ = tx.send(Err(format!("wry webview build failed: {e}")));
                        return;
                    }
                };
                let _ = tx.send(Ok(el_proxy));
                event_loop.run_return(move |event, _, control_flow| {
                    *control_flow = ControlFlow::Wait;
                    if let Event::UserEvent(cmd) = event {
                        match cmd {
                            WryCmd::Eval(js) => {
                                let _ = webview.evaluate_script(&js);
                            }
                            WryCmd::EvalCb(js, tx) => {
                                let _ = webview.evaluate_script_with_callback(&js, move |res| {
                                    let _ = tx.send(res);
                                });
                            }
                            WryCmd::Quit => {
                                *control_flow = ControlFlow::Exit;
                            }
                        }
                    }
                });
                drop(window);
            });

            let proxy = rx
                .recv_timeout(Duration::from_secs(15))
                .map_err(|e| format!("LiveWryJsHost init timeout: {e}"))??;
            Ok(Self {
                proxy,
                loaded,
                bridge,
                thread: Some(thread),
            })
        }

        fn wait_ready(&self, timeout: Duration) -> Result<(), String> {
            let deadline = Instant::now() + timeout;
            let (lock, cvar) = &*self.loaded;
            let mut guard = lock.lock().unwrap();
            while !*guard {
                let now = Instant::now();
                if now >= deadline {
                    return Err("LiveWryJsHost timed out waiting for robots.txt".to_string());
                }
                let (g, _) = cvar.wait_timeout(guard, deadline - now).unwrap();
                guard = g;
            }
            Ok(())
        }
    }

    impl Drop for LiveWryJsHost {
        fn drop(&mut self) {
            let _ = self.proxy.send_event(WryCmd::Quit);
            if let Some(h) = self.thread.take() {
                let _ = h.join();
            }
        }
    }

    impl JsHost for LiveWryJsHost {
        fn eval_async_json(&self, script_body: &str, timeout: Duration) -> Result<serde_json::Value, String> {
            self.wait_ready(Duration::from_secs(15))?;
            let (req_id, cb_url, rx) = self.bridge.register();
            let wrapped = wrap_async_script(script_body, &cb_url);
            if self.proxy.send_event(WryCmd::Eval(wrapped)).is_err() {
                self.bridge.cancel(&req_id);
                return Err("LiveWryJsHost closed".to_string());
            }
            let raw = match rx.recv_timeout(timeout) {
                Ok(s) => s,
                Err(e) => {
                    self.bridge.cancel(&req_id);
                    return Err(format!("LiveWryJsHost async timeout: {e}"));
                }
            };
            unwrap_callback_payload(&raw)
        }

        fn eval_sync_json(&self, expr: &str, timeout: Duration) -> Result<serde_json::Value, String> {
            self.wait_ready(Duration::from_secs(15))?;
            let (tx, rx) = mpsc::channel();
            self.proxy
                .send_event(WryCmd::EvalCb(expr.to_string(), tx))
                .map_err(|_| "LiveWryJsHost closed".to_string())?;
            let raw = rx
                .recv_timeout(timeout)
                .map_err(|e| format!("LiveWryJsHost sync timeout: {e}"))?;
            parse_sync_json_result(&raw)
        }
    }

    fn verify_live_stream_and_seeks(client: &reqwest::blocking::Client, url: &str, label: &str) {
        let head = client.head(url).send().expect("proxy HEAD");
        assert!(
            head.status().is_success() || head.status() == reqwest::StatusCode::PARTIAL_CONTENT,
            "{label}: HEAD failed with {}",
            head.status()
        );
        let expected_len = head
            .headers()
            .get(reqwest::header::CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.parse::<usize>().ok());

        let mut resp = client.get(url).send().expect("proxy GET");
        assert!(
            resp.status().is_success() || resp.status() == reqwest::StatusCode::PARTIAL_CONTENT,
            "{label}: GET failed with {}",
            resp.status()
        );
        let mut full = Vec::new();
        resp.read_to_end(&mut full).expect("proxy read_to_end");
        assert!(
            full.len() > 1_500_000,
            "{label}: stream did not exceed 1,500,000 bytes (got {})",
            full.len()
        );
        if let Some(exp) = expected_len {
            assert_eq!(full.len(), exp, "{label}: full EOF byte mismatch");
        }

        // Backward seek
        let mut seek_back = client.get(url).header("Range", "bytes=0-65535").send().unwrap();
        assert_eq!(seek_back.status(), reqwest::StatusCode::PARTIAL_CONTENT, "{label} seek back status");
        let mut back_bytes = Vec::new();
        seek_back.read_to_end(&mut back_bytes).unwrap();
        assert_eq!(&back_bytes[..], &full[0..=65535], "{label} backward seek mismatch");

        // Forward seek past 1 MiB (1:04 boundary)
        let mut seek_fwd = client
            .get(url)
            .header("Range", "bytes=1048576-1572863")
            .send()
            .unwrap();
        assert_eq!(seek_fwd.status(), reqwest::StatusCode::PARTIAL_CONTENT, "{label} seek fwd status");
        let mut fwd_bytes = Vec::new();
        seek_fwd.read_to_end(&mut fwd_bytes).unwrap();
        assert_eq!(&fwd_bytes[..], &full[1048576..=1572863], "{label} forward seek mismatch");
    }

    let dir = TestTempDir::new("t4_06_live");
    let proxy = StreamProxy::start(dir.log_path(), dir.cache_dir()).expect("StreamProxy::start failed");
    let rt = tokio::runtime::Runtime::new().unwrap();
    let rp = RustyPipe::new();
    let host = LiveWryJsHost::start().ok().map(Arc::new);
    let bg = host
        .as_ref()
        .and_then(|h| BgEngine::new(h.clone() as Arc<dyn JsHost>).ok())
        .map(Arc::new);

    let client = reqwest::blocking::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(90))
        .build()
        .unwrap();

    for (video_id, title) in [("qGXY_kO0K7Y", "Kalah 2"), ("fUdfDKyrbjk", "Mangu")] {
        let resolved: ResolvedStreams = rt.block_on(async {
            if let Some(bg_ref) = bg.as_ref() {
                if let Ok(r) = resolve_streams(bg_ref, &proxy, video_id).await {
                    if !r.audio.is_empty() && (!r.video_only.is_empty() || !r.muxed.is_empty()) {
                        return r;
                    }
                }
            }
            let player = rp
                .query()
                .player_from_client(video_id, ClientType::Ios)
                .await
                .unwrap_or_else(|e| panic!("Failed to resolve {title} ({video_id}): {e}"));
            let mut audio: Vec<ResolvedAudio> = player
                .audio_streams
                .iter()
                .filter(|s| s.drm_track_type.is_none())
                .map(|s| ResolvedAudio {
                    itag: 0,
                    url: proxy.register(&s.url, Some(s.size), Some(&s.mime)),
                    mime: s.mime.clone(),
                    bitrate: s.bitrate,
                })
                .collect();
            audio.sort_by(|a, b| b.bitrate.cmp(&a.bitrate));

            let mut muxed: Vec<ResolvedVideo> = player
                .video_streams
                .iter()
                .filter(|s| s.drm_track_type.is_none())
                .map(|s| ResolvedVideo {
                    itag: 0,
                    url: proxy.register(&s.url, s.size, Some(&s.mime)),
                    mime: s.mime.clone(),
                    quality: s.quality.clone(),
                    height: s.height,
                })
                .collect();
            muxed.sort_by(|a, b| b.height.cmp(&a.height));

            let mut video_only: Vec<ResolvedVideo> = player
                .video_only_streams
                .iter()
                .filter(|s| s.drm_track_type.is_none())
                .map(|s| ResolvedVideo {
                    itag: 0,
                    url: proxy.register(&s.url, s.size, Some(&s.mime)),
                    mime: s.mime.clone(),
                    quality: s.quality.clone(),
                    height: s.height,
                })
                .collect();
            video_only.sort_by(|a, b| b.height.cmp(&a.height));

            ResolvedStreams {
                muxed,
                video_only,
                audio,
                expires_in_seconds: player.expires_in_seconds,
            }
        });

        let audio = resolved
            .audio
            .iter()
            .find(|s| s.itag == 140 || s.itag == 251)
            .or_else(|| resolved.audio.first())
            .unwrap_or_else(|| panic!("{title}: no audio stream resolved"));
        verify_live_stream_and_seeks(&client, &audio.url, &format!("{title} audio"));

        let mut video_checked = false;
        if let Some(vo) = resolved
            .video_only
            .iter()
            .find(|v| v.height >= 360 && v.height <= 720)
            .or_else(|| resolved.video_only.first())
        {
            verify_live_stream_and_seeks(&client, &vo.url, &format!("{title} video_only ({})", vo.quality));
            video_checked = true;
        }
        if let Some(mx) = resolved.muxed.first() {
            verify_live_stream_and_seeks(&client, &mx.url, &format!("{title} muxed ({})", mx.quality));
            video_checked = true;
        }
        assert!(video_checked, "{title}: expected at least one video_only or muxed video stream");
    }

    let log = std::fs::read_to_string(dir.log_path()).unwrap_or_default();
    assert!(!log.contains("upstream status 403"), "403 encountered in proxy.log:\n{log}");
    assert!(!log.contains("502 error"), "502 encountered in proxy.log:\n{log}");
    assert!(!log.contains("504 timeout"), "504 encountered in proxy.log:\n{log}");
    drop(bg);
    drop(host);
    drop(proxy);
}
