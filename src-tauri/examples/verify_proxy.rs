use nada::bg_engine::{
    parse_sync_json_result, unwrap_callback_payload, wrap_async_script, BgEngine, CallbackBridge,
    JsHost,
};
use nada::proxy::StreamProxy;
use nada::streams::resolve_streams;
use serde_json::Value;
use std::sync::{mpsc, Arc, Condvar, Mutex};
use std::time::Duration;
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
    Navigate(String),
    Quit,
}

struct WryJsHost {
    proxy: EventLoopProxy<WryCmd>,
    loaded: Arc<(Mutex<bool>, Condvar)>,
    bridge: CallbackBridge,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl WryJsHost {
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
                .with_title("Nada VerifyProxy BgWorker")
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
                        WryCmd::Navigate(url) => {
                            let _ = webview.load_url(&url);
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
            .map_err(|e| format!("WryJsHost init timeout: {e}"))??;

        Ok(Self {
            proxy,
            loaded,
            bridge,
            thread: Some(thread),
        })
    }

    fn wait_ready(&self, timeout: Duration) -> Result<(), String> {
        let started = std::time::Instant::now();
        let deadline = started + timeout;
        let mut last_nav_attempt: Option<std::time::Instant> = None;
        let (lock, cvar) = &*self.loaded;
        let mut guard = lock.lock().unwrap();
        while !*guard {
            let now = std::time::Instant::now();
            if now >= deadline {
                return Err("WryJsHost timed out waiting for robots.txt page load".to_string());
            }
            let step = (deadline - now).min(Duration::from_millis(500));
            let (g, _) = cvar.wait_timeout(guard, step).unwrap();
            guard = g;
            if *guard {
                break;
            }
            drop(guard);

            let (tx, rx) = mpsc::channel();
            if self
                .proxy
                .send_event(WryCmd::EvalCb(
                    "(() => ({ ready: document.readyState === 'complete', host: String(location.hostname || '') }))()".to_string(),
                    tx,
                ))
                .is_ok()
            {
                if let Ok(raw) = rx.recv_timeout(Duration::from_millis(250)) {
                    if let Ok(val) = parse_sync_json_result(&raw) {
                        let ready = val.get("ready").and_then(Value::as_bool).unwrap_or(false);
                        let host = val.get("host").and_then(Value::as_str).unwrap_or("");
                        if ready && host.ends_with("youtube.com") {
                            let mut g2 = lock.lock().unwrap();
                            *g2 = true;
                            cvar.notify_all();
                            return Ok(());
                        }
                        let should_retry_nav = !host.ends_with("youtube.com")
                            && ((ready
                                && last_nav_attempt
                                    .is_none_or(|t| t.elapsed() >= Duration::from_secs(3)))
                                || (!ready
                                    && started.elapsed() >= Duration::from_secs(10)
                                    && last_nav_attempt
                                        .is_none_or(|t| t.elapsed() >= Duration::from_secs(8))));
                        if should_retry_nav {
                            let _ = self.proxy.send_event(WryCmd::Navigate(
                                "https://www.youtube.com/robots.txt".to_string(),
                            ));
                            last_nav_attempt = Some(std::time::Instant::now());
                        }
                    }
                }
            }
            guard = lock.lock().unwrap();
        }
        Ok(())
    }
}

impl Drop for WryJsHost {
    fn drop(&mut self) {
        let _ = self.proxy.send_event(WryCmd::Quit);
        if let Some(h) = self.thread.take() {
            let _ = h.join();
        }
    }
}

impl JsHost for WryJsHost {
    fn eval_async_json(&self, script_body: &str, timeout: Duration) -> Result<Value, String> {
        self.wait_ready(Duration::from_secs(30))?;
        let (req_id, cb_url, rx) = self.bridge.register();
        let wrapped = wrap_async_script(script_body, &cb_url);
        if self.proxy.send_event(WryCmd::Eval(wrapped)).is_err() {
            self.bridge.cancel(&req_id);
            return Err("WryJsHost event loop closed".to_string());
        }
        let raw = match rx.recv_timeout(timeout) {
            Ok(s) => s,
            Err(e) => {
                self.bridge.cancel(&req_id);
                return Err(format!("WryJsHost async callback timeout: {e}"));
            }
        };
        unwrap_callback_payload(&raw)
    }

    fn eval_sync_json(&self, expr: &str, timeout: Duration) -> Result<Value, String> {
        self.wait_ready(Duration::from_secs(30))?;
        let (tx, rx) = mpsc::channel();
        self.proxy
            .send_event(WryCmd::EvalCb(expr.to_string(), tx))
            .map_err(|_| "WryJsHost event loop closed".to_string())?;
        let raw = rx
            .recv_timeout(timeout)
            .map_err(|e| format!("WryJsHost sync callback timeout: {e}"))?;
        parse_sync_json_result(&raw)
    }
}

async fn fetch_full_and_verify_head(
    client: &reqwest::Client,
    url: &str,
    label: &str,
) -> Vec<u8> {
    let head = client.head(url).send().await.expect("HEAD proxy");
    assert_eq!(
        head.status(),
        reqwest::StatusCode::OK,
        "{label}: expected 200 OK on HEAD, got {}",
        head.status()
    );
    let expected_len: u64 = head
        .headers()
        .get(reqwest::header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(|| panic!("{label}: HEAD missing Content-Length"));
    assert!(expected_len > 0, "{label}: expected non-zero Content-Length");
    assert_eq!(
        head.headers()
            .get(reqwest::header::ACCEPT_RANGES)
            .and_then(|v| v.to_str().ok()),
        Some("bytes"),
        "{label}: HEAD missing Accept-Ranges: bytes"
    );

    let mut resp = client.get(url).send().await.expect("GET proxy");
    assert_eq!(
        resp.status(),
        reqwest::StatusCode::OK,
        "{label}: expected 200 OK on full GET, got {}",
        resp.status()
    );
    let mut data = Vec::with_capacity(expected_len as usize);
    while let Some(chunk) = resp.chunk().await.expect("stream chunk") {
        data.extend_from_slice(&chunk);
    }
    assert_eq!(
        data.len() as u64,
        expected_len,
        "{label}: EOF byte mismatch {} vs {expected_len}",
        data.len()
    );
    println!("[OK] {label}: streamed {}/{} bytes (100% EOF)", data.len(), expected_len);
    data
}

async fn verify_seek_slice(
    client: &reqwest::Client,
    url: &str,
    full: &[u8],
    start: usize,
    end_inclusive: Option<usize>,
    label: &str,
) {
    let total = full.len();
    let end = end_inclusive.unwrap_or(total - 1).min(total - 1);
    let range_hdr = match end_inclusive {
        Some(e) => format!("bytes={start}-{e}"),
        None => format!("bytes={start}-"),
    };
    let mut resp = client
        .get(url)
        .header(reqwest::header::RANGE, &range_hdr)
        .send()
        .await
        .expect("seek GET");
    assert_eq!(
        resp.status(),
        reqwest::StatusCode::PARTIAL_CONTENT,
        "{label} ({range_hdr}): expected 206, got {}",
        resp.status()
    );
    let expected_cr = format!("bytes {start}-{end}/{total}");
    assert_eq!(
        resp.headers()
            .get(reqwest::header::CONTENT_RANGE)
            .and_then(|v| v.to_str().ok()),
        Some(expected_cr.as_str()),
        "{label}: Content-Range mismatch"
    );
    let mut got = Vec::with_capacity(end - start + 1);
    while let Some(chunk) = resp.chunk().await.expect("seek chunk") {
        got.extend_from_slice(&chunk);
    }
    assert_eq!(
        &got[..],
        &full[start..=end],
        "{label} ({range_hdr}): seek payload mismatch"
    );
    println!("[OK] {label} seek {range_hdr}: verified {} bytes", got.len());
}

struct VerifyTempDir {
    dir: std::path::PathBuf,
    log_path: std::path::PathBuf,
}

impl Drop for VerifyTempDir {
    fn drop(&mut self) {
        if std::thread::panicking() {
            let log_content = std::fs::read_to_string(&self.log_path).unwrap_or_default();
            eprintln!("\n--- Proxy Log (on failure/panic) ---\n{log_content}");
        }
        for attempt in 0..10 {
            if !self.dir.exists() || std::fs::remove_dir_all(&self.dir).is_ok() {
                break;
            }
            if attempt < 9 {
                std::thread::sleep(Duration::from_millis(30));
            }
        }
    }
}

fn main() {
    println!("=== Extended Live Stream Resolution & Proxy Verification ===");

    let temp_dir = std::env::temp_dir().join(format!("nada-verify-proxy-{}", std::process::id()));
    let cache_dir = temp_dir.join("cache");
    let log_path = temp_dir.join("proxy.log");
    let _ = std::fs::remove_dir_all(&temp_dir);
    std::fs::create_dir_all(&cache_dir).unwrap();
    let _temp_guard = VerifyTempDir {
        dir: temp_dir.clone(),
        log_path: log_path.clone(),
    };

    let proxy = StreamProxy::start(log_path.clone(), cache_dir).expect("proxy failed to start");
    let host = Arc::new(WryJsHost::start().expect("WryJsHost failed to start"));
    let bg = Arc::new(BgEngine::new(host.clone()).expect("BgEngine init failed"));

    let rt = tokio::runtime::Runtime::new().unwrap();
    let http_client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .read_timeout(Duration::from_secs(30))
        .build()
        .unwrap();

    rt.block_on(async {
        bg.ensure_initialized()
            .await
            .expect("BgEngine live initialization failed");
        println!("[OK] WebView2 BotGuard + base.js initialized");

        let tracks = [
            ("fUdfDKyrbjk", "Mangu", true),
            ("5QMsYmJYUlQ", "Kita Usahakan Lagi", true),
            ("qGXY_kO0K7Y", "Kalah 2", true),
            ("jNQXAC9IVRw", "Me at the zoo (non-music fallback)", false),
        ];

        for (video_id, title, is_music) in tracks {
            println!("\n--- Resolving: {title} ({video_id}) ---");
            let streams = resolve_streams(&bg, &proxy, video_id)
                .await
                .unwrap_or_else(|e| panic!("resolve_streams failed for {title}: {e}"));

            println!(
                "Resolved: audio={}, video_only={}, muxed={}",
                streams.audio.len(),
                streams.video_only.len(),
                streams.muxed.len()
            );
            assert!(
                !streams.audio.is_empty(),
                "{title}: expected at least 1 audio stream"
            );

            // Verify primary audio (prefer 140/251 or highest bitrate)
            let audio = streams
                .audio
                .iter()
                .find(|a| a.itag == 140 || a.itag == 251)
                .unwrap_or(&streams.audio[0]);
            let audio_bytes = fetch_full_and_verify_head(
                &http_client,
                &audio.url,
                &format!("{title} audio itag={}", audio.itag),
            )
            .await;
            if is_music {
                assert!(
                    audio_bytes.len() > 1_048_576,
                    "{title} audio must exceed 1 MiB (1:04 boundary), got {}",
                    audio_bytes.len()
                );
                // Forward seek past 1 MiB, backward seek, and tail seek
                verify_seek_slice(
                    &http_client,
                    &audio.url,
                    &audio_bytes,
                    2_000_000,
                    Some(2_131_071),
                    &format!("{title} audio fwd>1MiB"),
                )
                .await;
                verify_seek_slice(
                    &http_client,
                    &audio.url,
                    &audio_bytes,
                    524_288,
                    Some(655_359),
                    &format!("{title} audio bwd"),
                )
                .await;
                verify_seek_slice(
                    &http_client,
                    &audio.url,
                    &audio_bytes,
                    0,
                    Some(65_535),
                    &format!("{title} audio bwd0"),
                )
                .await;
                let tail_start = audio_bytes.len().saturating_sub(65_536);
                verify_seek_slice(
                    &http_client,
                    &audio.url,
                    &audio_bytes,
                    tail_start,
                    None,
                    &format!("{title} audio tail"),
                )
                .await;
            }

            // Verify video_only (prefer 136 or <=720p) and/or muxed (18)
            if let Some(v) = streams
                .video_only
                .iter()
                .find(|v| v.itag == 136)
                .or_else(|| streams.video_only.iter().find(|v| v.height <= 720))
                .or_else(|| streams.video_only.first())
            {
                let v_bytes = fetch_full_and_verify_head(
                    &http_client,
                    &v.url,
                    &format!("{title} video_only itag={} ({})", v.itag, v.quality),
                )
                .await;
                if v_bytes.len() > 1_100_000 {
                    let fwd_start = if v_bytes.len() > 4_300_000 {
                        4_194_304
                    } else {
                        1_048_576
                    };
                    verify_seek_slice(
                        &http_client,
                        &v.url,
                        &v_bytes,
                        fwd_start,
                        Some((fwd_start + 65_535).min(v_bytes.len() - 1)),
                        &format!("{title} video_only fwd"),
                    )
                    .await;
                }
            }

            if let Some(m) = streams.muxed.first() {
                let m_bytes = fetch_full_and_verify_head(
                    &http_client,
                    &m.url,
                    &format!("{title} muxed itag={} ({})", m.itag, m.quality),
                )
                .await;
                if m_bytes.len() > 4_300_000 {
                    verify_seek_slice(
                        &http_client,
                        &m.url,
                        &m_bytes,
                        4_194_304,
                        Some(4_259_839),
                        &format!("{title} muxed fwd>4MiB"),
                    )
                    .await;
                    verify_seek_slice(
                        &http_client,
                        &m.url,
                        &m_bytes,
                        524_288,
                        Some(655_359),
                        &format!("{title} muxed bwd"),
                    )
                    .await;
                }
            }
        }

        // Replay / re-registration verification on Kalah 2
        println!("\n--- Replay Re-Registration: Kalah 2 (qGXY_kO0K7Y) ---");
        let replay = resolve_streams(&bg, &proxy, "qGXY_kO0K7Y")
            .await
            .expect("replay resolve_streams failed");
        let replay_audio = replay
            .audio
            .iter()
            .find(|a| a.itag == 140 || a.itag == 251)
            .unwrap_or(&replay.audio[0]);
        let _ = fetch_full_and_verify_head(
            &http_client,
            &replay_audio.url,
            "Kalah 2 replay audio",
        )
        .await;
    });

    let log_content = std::fs::read_to_string(&log_path).unwrap_or_default();
    println!("\n--- Proxy Log Summary ---");
    println!("{log_content}");
    assert!(
        !log_content.contains("upstream status 403"),
        "Unexpected 403 Forbidden in proxy.log"
    );
    assert!(
        !log_content.contains("502 error"),
        "Unexpected 502 error in proxy.log"
    );
    assert!(
        !log_content.contains("504 timeout"),
        "Unexpected 504 timeout in proxy.log"
    );

    drop(bg);
    drop(host);
    drop(proxy);
    let _ = std::fs::remove_dir_all(&temp_dir);
    println!("\n=== ALL LIVE VERIFICATION CHECKS PASSED ===");
}
