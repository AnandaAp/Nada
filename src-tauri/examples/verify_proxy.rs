use nada::bg_engine::{BgEngine, QuickJsHost};
use nada::proxy::StreamProxy;
use nada::streams::resolve_streams;
use std::sync::Arc;
use std::time::Duration;

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
    let head_ct = head
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let head_ct_lc = head_ct.to_ascii_lowercase();
    assert!(
        head_ct_lc.starts_with("audio/") || head_ct_lc.starts_with("video/"),
        "{label}: HEAD Content-Type must be audio/* or video/*, got {head_ct:?}"
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
    let get_ct = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let get_ct_lc = get_ct.to_ascii_lowercase();
    assert!(
        get_ct_lc.starts_with("audio/") || get_ct_lc.starts_with("video/"),
        "{label}: GET Content-Type must be audio/* or video/*, got {get_ct:?}"
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
    let host = Arc::new(QuickJsHost::start().expect("QuickJsHost failed to start"));
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
        println!("[OK] QuickJS BotGuard + base.js initialized");

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
                let audio_fwd_start = if audio_bytes.len() > 2_131_071 {
                    2_000_000
                } else {
                    1_048_576
                };
                verify_seek_slice(
                    &http_client,
                    &audio.url,
                    &audio_bytes,
                    audio_fwd_start,
                    Some((audio_fwd_start + 131_071).min(audio_bytes.len() - 1)),
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
                if is_music {
                    assert!(
                        m_bytes.len() > 1_048_576,
                        "{title} muxed must exceed 1 MiB (1:04 boundary), got {}",
                        m_bytes.len()
                    );
                    let fwd_start = if m_bytes.len() > 4_300_000 {
                        4_194_304
                    } else {
                        1_048_576
                    };
                    verify_seek_slice(
                        &http_client,
                        &m.url,
                        &m_bytes,
                        fwd_start,
                        Some((fwd_start + 65_535).min(m_bytes.len() - 1)),
                        &format!("{title} muxed fwd>1MiB"),
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
    assert!(
        !log_content.contains("mime=text/plain"),
        "Unexpected mime=text/plain served by proxy in proxy.log"
    );

    drop(bg);
    drop(host);
    drop(proxy);
    let _ = std::fs::remove_dir_all(&temp_dir);
    println!("\n=== ALL LIVE VERIFICATION CHECKS PASSED ===");
}
