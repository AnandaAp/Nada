use crate::bg_engine::{send_with_transient_retry, BgEngine, VideoContext};
use crate::proxy::{StreamProxy, StreamRegisterHeaders};
use serde_json::Value;

const IOS_USER_AGENT: &str =
    "com.google.ios.youtube/20.03.02 (iPhone16,2; U; CPU iOS 18_2_1 like Mac OS X)";
const MWEB_USER_AGENT: &str =
    "Mozilla/5.0 (Linux; Android 10; K) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.6778.135 Mobile Safari/537.36";

#[derive(Clone, Debug)]
pub struct ResolvedAudio {
    pub itag: u64,
    pub url: String,
    pub mime: String,
    pub bitrate: u32,
}

#[derive(Clone, Debug)]
pub struct ResolvedVideo {
    pub itag: u64,
    pub url: String,
    pub mime: String,
    pub quality: String,
    pub height: u32,
}

#[derive(Clone, Debug)]
pub struct ResolvedStreams {
    pub muxed: Vec<ResolvedVideo>,
    pub video_only: Vec<ResolvedVideo>,
    pub audio: Vec<ResolvedAudio>,
    pub expires_in_seconds: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ClientKind {
    WebRemix,
    Mweb,
    Ios,
}

#[derive(Clone, Debug)]
struct RawCandidate {
    itag: u64,
    is_muxed: bool,
    is_video: bool,
    raw_url: String,
    sp: String,
    s: String,
    mime: String,
    bitrate: u32,
    quality: String,
    height: u32,
    content_length: Option<u64>,
}

pub fn percent_decode_form(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < bytes.len() => {
                if let (Some(h), Some(l)) = (hex_val(bytes[i + 1]), hex_val(bytes[i + 2])) {
                    out.push((h << 4) | l);
                    i += 3;
                    continue;
                }
                out.push(b'%');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

pub fn parse_signature_cipher(cipher: &str) -> Option<(String, String, String)> {
    let mut url: Option<String> = None;
    let mut sp: Option<String> = None;
    let mut s: Option<String> = None;
    for pair in cipher.split('&') {
        if pair.is_empty() {
            continue;
        }
        let Some((k, v)) = pair.split_once('=') else {
            continue;
        };
        let key = percent_decode_form(k);
        let val = percent_decode_form(v);
        match key.as_str() {
            "url" => url = Some(val),
            "sp" => sp = Some(val),
            "s" => s = Some(val),
            _ => {}
        }
    }
    let url = url.filter(|u| !u.is_empty())?;
    let s = s.filter(|v| !v.is_empty())?;
    let sp = sp.filter(|v| !v.is_empty()).unwrap_or_else(|| "sig".to_string());
    Some((url, sp, s))
}

pub fn append_pot_param(url: &str, pot: &str) -> String {
    if pot.is_empty() {
        return url.to_string();
    }
    let encoded_pot: String = pot
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{:02X}", b),
        })
        .collect();
    if let Some((base, query_part)) = url.split_once('?') {
        let mut replaced_empty = false;
        let mut updated_parts = Vec::new();
        for kv in query_part.split('&') {
            if let Some(existing_val) = kv.strip_prefix("pot=") {
                if !existing_val.is_empty() {
                    return url.to_string();
                }
                if !replaced_empty {
                    updated_parts.push(format!("pot={encoded_pot}"));
                    replaced_empty = true;
                }
            } else {
                updated_parts.push(kv.to_string());
            }
        }
        if replaced_empty {
            return format!("{base}?{}", updated_parts.join("&"));
        }
    }
    let sep = if url.contains('?') { '&' } else { '?' };
    format!("{url}{sep}pot={encoded_pot}")
}

fn is_playable_format(fmt: &Value) -> bool {
    if let Some(drm) = fmt.get("drmFamilies").and_then(Value::as_array) {
        if !drm.is_empty() {
            return false;
        }
    }
    if fmt.get("type").and_then(Value::as_str) == Some("FORMAT_STREAM_TYPE_OTF") {
        return false;
    }
    let has_url = fmt
        .get("url")
        .and_then(Value::as_str)
        .is_some_and(|u| !u.is_empty());
    let has_cipher = fmt
        .get("signatureCipher")
        .or_else(|| fmt.get("cipher"))
        .and_then(Value::as_str)
        .is_some_and(|c| !c.is_empty());
    has_url || has_cipher
}

fn extract_raw_candidates(player_json: &Value) -> Result<(Vec<RawCandidate>, u32), String> {
    let status = player_json
        .pointer("/playabilityStatus/status")
        .and_then(Value::as_str)
        .unwrap_or("UNKNOWN");
    if status != "OK" {
        let reason = player_json
            .pointer("/playabilityStatus/reason")
            .and_then(Value::as_str)
            .unwrap_or(status);
        return Err(format!("playabilityStatus={status}: {reason}"));
    }

    let streaming = player_json
        .get("streamingData")
        .ok_or_else(|| "missing streamingData".to_string())?;
    let expires = streaming
        .get("expiresInSeconds")
        .and_then(|v| v.as_u64().or_else(|| v.as_str().and_then(|s| s.parse().ok())))
        .unwrap_or(21540) as u32;

    let mut out = Vec::new();
    for (arr_key, is_muxed) in [("formats", true), ("adaptiveFormats", false)] {
        let Some(arr) = streaming.get(arr_key).and_then(Value::as_array) else {
            continue;
        };
        for fmt in arr {
            if !is_playable_format(fmt) {
                continue;
            }
            let mime = fmt
                .get("mimeType")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            if mime.is_empty() {
                continue;
            }
            let is_video = mime.starts_with("video/");
            let is_audio = mime.starts_with("audio/");
            if !is_video && !is_audio {
                continue;
            }

            let (raw_url, sp, s) = if let Some(u) = fmt.get("url").and_then(Value::as_str) {
                (u.to_string(), String::new(), String::new())
            } else if let Some(cipher) = fmt
                .get("signatureCipher")
                .or_else(|| fmt.get("cipher"))
                .and_then(Value::as_str)
            {
                let Some(parsed) = parse_signature_cipher(cipher) else {
                    continue;
                };
                parsed
            } else {
                continue;
            };

            let itag = fmt.get("itag").and_then(Value::as_u64).unwrap_or(0);
            let bitrate = fmt
                .get("bitrate")
                .and_then(Value::as_u64)
                .or_else(|| fmt.get("averageBitrate").and_then(Value::as_u64))
                .unwrap_or(0) as u32;
            let height = fmt.get("height").and_then(Value::as_u64).unwrap_or(0) as u32;
            let quality = fmt
                .get("qualityLabel")
                .and_then(Value::as_str)
                .map(|q| q.to_string())
                .unwrap_or_else(|| {
                    if height > 0 {
                        format!("{height}p")
                    } else {
                        "unknown".to_string()
                    }
                });
            let content_length = fmt
                .get("contentLength")
                .and_then(|v| v.as_u64().or_else(|| v.as_str().and_then(|s| s.parse().ok())));

            out.push(RawCandidate {
                itag,
                is_muxed,
                is_video,
                raw_url,
                sp,
                s,
                mime,
                bitrate,
                quality,
                height,
                content_length,
            });
        }
    }

    Ok((out, expires))
}

async fn fetch_player_json(
    http: &reqwest::Client,
    kind: ClientKind,
    video_id: &str,
    ctx: &VideoContext,
) -> Result<Value, String> {
    let (endpoint, ua, origin, referer, client_id, client_name, client_ver, use_web_ctx) =
        match kind {
            ClientKind::WebRemix => (
                "https://music.youtube.com/youtubei/v1/player?prettyPrint=false",
                ctx.user_agent.as_str(),
                Some("https://music.youtube.com"),
                Some("https://music.youtube.com/"),
                "67",
                "WEB_REMIX",
                "1.20261118.01.00",
                true,
            ),
            ClientKind::Mweb => (
                "https://m.youtube.com/youtubei/v1/player?prettyPrint=false",
                MWEB_USER_AGENT,
                Some("https://m.youtube.com"),
                Some("https://m.youtube.com/"),
                "2",
                "MWEB",
                "2.20261120.01.00",
                true,
            ),
            ClientKind::Ios => (
                "https://www.youtube.com/youtubei/v1/player?prettyPrint=false",
                IOS_USER_AGENT,
                None,
                None,
                "5",
                "IOS",
                "20.03.02",
                false,
            ),
        };

    let body = if use_web_ctx {
        serde_json::json!({
            "context": {
                "client": {
                    "clientName": client_name,
                    "clientVersion": client_ver,
                    "visitorData": ctx.visitor_data,
                    "hl": "en",
                    "gl": "ID"
                }
            },
            "videoId": video_id,
            "playbackContext": {
                "contentPlaybackContext": {
                    "signatureTimestamp": ctx.sts,
                    "referer": format!("{}{}", origin.unwrap_or("https://www.youtube.com"), format!("/watch?v={video_id}"))
                }
            },
            "serviceIntegrityDimensions": {
                "poToken": ctx.content_pot
            },
            "contentCheckOk": true,
            "racyCheckOk": true
        })
    } else {
        serde_json::json!({
            "context": {
                "client": {
                    "clientName": client_name,
                    "clientVersion": client_ver,
                    "deviceMake": "Apple",
                    "deviceModel": "iPhone16,2",
                    "osName": "iOS",
                    "osVersion": "18.2.1.22C161",
                    "visitorData": ctx.visitor_data,
                    "hl": "en",
                    "gl": "ID"
                }
            },
            "videoId": video_id,
            "contentCheckOk": true,
            "racyCheckOk": true
        })
    };

    let resp = send_with_transient_retry(|| {
        let mut req = http
            .post(endpoint)
            .header(reqwest::header::USER_AGENT, ua)
            .header("X-YouTube-Client-Name", client_id)
            .header("X-YouTube-Client-Version", client_ver)
            .header("X-Goog-Visitor-Id", &ctx.visitor_data)
            .json(&body);

        if let Some(org) = origin {
            req = req.header(reqwest::header::ORIGIN, org);
        }
        if let Some(ref_url) = referer {
            req = req.header(reqwest::header::REFERER, ref_url);
            req = req.header(reqwest::header::COOKIE, "SOCS=CAISAiAD");
        }
        req
    })
    .await
    .map_err(|e| format!("{client_name} player request failed: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("{client_name} player HTTP {}", resp.status()));
    }
    resp.json::<Value>()
        .await
        .map_err(|e| format!("{client_name} player json failed: {e}"))
}

fn register_client_candidates(
    candidates: Vec<RawCandidate>,
    deobf_urls: Vec<String>,
    kind: ClientKind,
    ctx: &VideoContext,
    proxy: &StreamProxy,
    expires_in_seconds: u32,
) -> ResolvedStreams {
    let (headers, attach_pot) = match kind {
        ClientKind::WebRemix => (
            StreamRegisterHeaders {
                user_agent: Some(ctx.user_agent.clone()),
                origin: Some("https://music.youtube.com".to_string()),
                referer: Some("https://music.youtube.com/".to_string()),
            },
            true,
        ),
        ClientKind::Mweb => (
            StreamRegisterHeaders {
                user_agent: Some(MWEB_USER_AGENT.to_string()),
                origin: Some("https://m.youtube.com".to_string()),
                referer: Some("https://m.youtube.com/".to_string()),
            },
            true,
        ),
        ClientKind::Ios => (
            StreamRegisterHeaders {
                user_agent: Some(IOS_USER_AGENT.to_string()),
                origin: None,
                referer: None,
            },
            false,
        ),
    };

    let mut muxed = Vec::new();
    let mut video_only = Vec::new();
    let mut audio = Vec::new();

    for (cand, deobf_url) in candidates.into_iter().zip(deobf_urls.into_iter()) {
        let final_url = if attach_pot {
            append_pot_param(&deobf_url, &ctx.content_pot)
        } else {
            deobf_url
        };
        let proxy_url = proxy.register_with_headers(
            &final_url,
            cand.content_length,
            Some(&cand.mime),
            headers.clone(),
        );
        if cand.is_muxed {
            muxed.push(ResolvedVideo {
                itag: cand.itag,
                url: proxy_url,
                mime: cand.mime,
                quality: cand.quality,
                height: cand.height,
            });
        } else if cand.is_video {
            video_only.push(ResolvedVideo {
                itag: cand.itag,
                url: proxy_url,
                mime: cand.mime,
                quality: cand.quality,
                height: cand.height,
            });
        } else {
            audio.push(ResolvedAudio {
                itag: cand.itag,
                url: proxy_url,
                mime: cand.mime,
                bitrate: cand.bitrate,
            });
        }
    }

    audio.sort_by(|a, b| b.bitrate.cmp(&a.bitrate));
    muxed.sort_by(|a, b| b.height.cmp(&a.height));
    video_only.sort_by(|a, b| b.height.cmp(&a.height));

    ResolvedStreams {
        muxed,
        video_only,
        audio,
        expires_in_seconds,
    }
}

async fn resolve_with_client(
    http: &reqwest::Client,
    bg: &BgEngine,
    proxy: &StreamProxy,
    kind: ClientKind,
    video_id: &str,
    ctx: &VideoContext,
) -> Result<ResolvedStreams, String> {
    let player_json = fetch_player_json(http, kind, video_id, ctx).await?;
    let (candidates, expires) = extract_raw_candidates(&player_json)?;
    if candidates.is_empty() {
        return Err(format!("{kind:?} returned 0 playable formats"));
    }

    let final_urls = if kind == ClientKind::Ios {
        candidates.iter().map(|c| c.raw_url.clone()).collect()
    } else {
        let batch: Vec<(String, String, String)> = candidates
            .iter()
            .map(|c| (c.raw_url.clone(), c.sp.clone(), c.s.clone()))
            .collect();
        bg.deobf_batch(&batch).await?
    };

    Ok(register_client_candidates(
        candidates,
        final_urls,
        kind,
        ctx,
        proxy,
        expires,
    ))
}

pub async fn resolve_streams(
    bg: &BgEngine,
    proxy: &StreamProxy,
    video_id: &str,
) -> Result<ResolvedStreams, String> {
    let http = bg.http_client();

    let ctx = bg.prepare_for_video(video_id).await?;

    let mut last_err;
    match resolve_with_client(http, bg, proxy, ClientKind::WebRemix, video_id, &ctx).await {
        Ok(res) if !res.audio.is_empty() || !res.muxed.is_empty() || !res.video_only.is_empty() => {
            return Ok(res);
        }
        Ok(_) => last_err = "WEB_REMIX returned empty stream sets".to_string(),
        Err(e) => last_err = e,
    }

    let mweb_res = resolve_with_client(http, bg, proxy, ClientKind::Mweb, video_id, &ctx).await;
    match mweb_res {
        Ok(res) if !res.audio.is_empty() => return Ok(res),
        Ok(mut mweb_only) => {
            // MWEB sometimes returns only muxed (itag=18) without adaptive audio;
            // supplement with IOS adaptive audio/video_only for non-PO-enforced videos.
            if let Ok(ios_res) =
                resolve_with_client(http, bg, proxy, ClientKind::Ios, video_id, &ctx).await
            {
                if mweb_only.audio.is_empty() {
                    mweb_only.audio = ios_res.audio;
                }
                if mweb_only.video_only.is_empty() {
                    mweb_only.video_only = ios_res.video_only;
                }
            }
            if !mweb_only.audio.is_empty() || !mweb_only.muxed.is_empty() {
                return Ok(mweb_only);
            }
        }
        Err(e) => {
            last_err = format!("{last_err}; {e}");
        }
    }

    match resolve_with_client(http, bg, proxy, ClientKind::Ios, video_id, &ctx).await {
        Ok(res) => Ok(res),
        Err(e) => Err(format!("All Innertube clients failed ({last_err}; {e})")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bg_engine::JsHost;
    use std::sync::Arc;
    use std::time::Duration;

    struct MockJsHost;

    impl JsHost for MockJsHost {
        fn eval_async_json(&self, script_body: &str, _timeout: Duration) -> Result<Value, String> {
            if script_body.contains("GenerateIT") {
                Ok(serde_json::json!({
                    "ua": "Mozilla/5.0 MockEdge/131.0",
                    "ttl": 43200
                }))
            } else if script_body.contains("__nada_minter") {
                Ok(serde_json::json!({
                    "pot": "mockPotToken1234"
                }))
            } else if script_body.contains("__nada_deobf") {
                Ok(serde_json::json!({ "ready": true }))
            } else {
                Ok(Value::Null)
            }
        }

        fn eval_sync_json(&self, expr: &str, _timeout: Duration) -> Result<Value, String> {
            // Parse items = [[u, sp, s], ...] from expr
            let marker = "const items = ";
            let start = expr.find(marker).ok_or("missing items")? + marker.len();
            let end = expr[start..].find(";\n").ok_or("missing items end")? + start;
            let items: Vec<(String, String, String)> =
                serde_json::from_str(&expr[start..end]).map_err(|e| e.to_string())?;
            let urls: Vec<String> = items
                .into_iter()
                .map(|(u, sp, s)| {
                    if s.is_empty() {
                        format!("{u}&n=deobf")
                    } else {
                        let param = if sp.is_empty() { "sig" } else { &sp };
                        let sep = if u.contains('?') { '&' } else { '?' };
                        format!("{u}{sep}{param}=decoded_{s}&n=deobf")
                    }
                })
                .collect();
            Ok(serde_json::json!({ "urls": urls }))
        }
    }

    #[test]
    fn parse_signature_cipher_decodes_fields() {
        let cipher = "s=ABC%3D123&sp=sig&url=https%3A%2F%2Fexample.com%2Fvideoplayback%3Fitag%3D140";
        let (url, sp, s) = parse_signature_cipher(cipher).unwrap();
        assert_eq!(url, "https://example.com/videoplayback?itag=140");
        assert_eq!(sp, "sig");
        assert_eq!(s, "ABC=123");

        let default_sp = "s=XYZ&url=https%3A%2F%2Fexample.com%2Fv";
        let (_, sp2, s2) = parse_signature_cipher(default_sp).unwrap();
        assert_eq!(sp2, "sig");
        assert_eq!(s2, "XYZ");

        assert!(parse_signature_cipher("url=https%3A%2F%2Fexample.com").is_none());

        // Empty segments (&& / trailing &) and valueless flags must be skipped cleanly
        let messy = "s=SIG4&&flagonly&sp=sig&url=https%3A%2F%2Fexample.com%2Fvp&";
        let (u4, sp4, s4) = parse_signature_cipher(messy).unwrap();
        assert_eq!(u4, "https://example.com/vp");
        assert_eq!(sp4, "sig");
        assert_eq!(s4, "SIG4");
    }

    #[test]
    fn append_pot_param_handles_separators_and_idempotency() {
        assert_eq!(
            append_pot_param("https://example.com/vp?itag=140", "abc_123="),
            "https://example.com/vp?itag=140&pot=abc_123%3D"
        );
        assert_eq!(
            append_pot_param("https://example.com/vp", "abc"),
            "https://example.com/vp?pot=abc"
        );
        assert_eq!(
            append_pot_param("https://example.com/vp?itag=140&pot=existing", "new"),
            "https://example.com/vp?itag=140&pot=existing"
        );
        assert_eq!(
            append_pot_param("https://example.com/vp?itag=140&pot=", "new="),
            "https://example.com/vp?itag=140&pot=new%3D"
        );
        assert_eq!(
            append_pot_param("https://example.com/vp?spot=123", "tok"),
            "https://example.com/vp?spot=123&pot=tok"
        );
    }

    #[test]
    fn extract_candidates_filters_drm_otf_and_sorts_with_mock_host() {
        let player_json = serde_json::json!({
            "playabilityStatus": { "status": "OK" },
            "streamingData": {
                "expiresInSeconds": "21540",
                "formats": [
                    {
                        "itag": 18,
                        "mimeType": "video/mp4; codecs=\"avc1.42001E, mp4a.40.2\"",
                        "bitrate": 500000,
                        "height": 360,
                        "qualityLabel": "360p",
                        "contentLength": "4000000",
                        "signatureCipher": "s=M1&sp=sig&url=https%3A%2F%2Fexample.com%2Fvp%3Fitag%3D18"
                    }
                ],
                "adaptiveFormats": [
                    {
                        "itag": 140,
                        "mimeType": "audio/mp4; codecs=\"mp4a.40.2\"",
                        "bitrate": 128000,
                        "contentLength": "5021414",
                        "signatureCipher": "s=A1&sp=sig&url=https%3A%2F%2Fexample.com%2Fvp%3Fitag%3D140"
                    },
                    {
                        "itag": 251,
                        "mimeType": "audio/webm; codecs=\"opus\"",
                        "bitrate": 160000,
                        "contentLength": "5464606",
                        "url": "https://example.com/vp?itag=251"
                    },
                    {
                        "itag": 999,
                        "mimeType": "audio/mp4",
                        "bitrate": 320000,
                        "drmFamilies": ["WIDEVINE"],
                        "url": "https://example.com/drm"
                    },
                    {
                        "itag": 998,
                        "mimeType": "video/mp4",
                        "height": 1080,
                        "type": "FORMAT_STREAM_TYPE_OTF",
                        "url": "https://example.com/otf"
                    },
                    {
                        "itag": 136,
                        "mimeType": "video/mp4; codecs=\"avc1.4d401f\"",
                        "bitrate": 1200000,
                        "height": 720,
                        "qualityLabel": "720p",
                        "contentLength": "1747971",
                        "signatureCipher": "s=V720&sp=sig&url=https%3A%2F%2Fexample.com%2Fvp%3Fitag%3D136"
                    },
                    {
                        "itag": 137,
                        "mimeType": "video/mp4; codecs=\"avc1.640028\"",
                        "bitrate": 2400000,
                        "height": 1080,
                        "qualityLabel": "1080p",
                        "contentLength": "3000000",
                        "signatureCipher": "s=V1080&sp=sig&url=https%3A%2F%2Fexample.com%2Fvp%3Fitag%3D137"
                    }
                ]
            }
        });

        let (cands, expires) = extract_raw_candidates(&player_json).unwrap();
        assert_eq!(expires, 21540);
        assert_eq!(cands.len(), 5); // DRM and OTF filtered out

        let rt = tokio::runtime::Runtime::new().unwrap();
        let bg = BgEngine::new(Arc::new(MockJsHost)).unwrap();
        let batch: Vec<(String, String, String)> = cands
            .iter()
            .map(|c| (c.raw_url.clone(), c.sp.clone(), c.s.clone()))
            .collect();
        let deobf = rt.block_on(bg.deobf_batch(&batch)).unwrap();

        let dir = std::env::temp_dir().join(format!("nada-streams-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("cache")).unwrap();
        let proxy = StreamProxy::start(dir.join("proxy.log"), dir.join("cache")).unwrap();

        let ctx = VideoContext {
            user_agent: "MockUA".to_string(),
            visitor_data: "MockVD".to_string(),
            content_pot: "mockPot".to_string(),
            sts: 20779,
        };
        let resolved = register_client_candidates(
            cands,
            deobf,
            ClientKind::WebRemix,
            &ctx,
            &proxy,
            expires,
        );

        assert_eq!(resolved.muxed.len(), 1);
        assert_eq!(resolved.muxed[0].height, 360);
        assert_eq!(resolved.video_only.len(), 2);
        assert_eq!(resolved.video_only[0].height, 1080);
        assert_eq!(resolved.video_only[1].height, 720);
        assert_eq!(resolved.audio.len(), 2);
        assert_eq!(resolved.audio[0].bitrate, 160000);
        assert_eq!(resolved.audio[1].bitrate, 128000);
        assert!(resolved.audio[0].url.starts_with("http://127.0.0.1:"));

        drop(proxy);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
