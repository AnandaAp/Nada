use rustypipe::client::{ClientType, RustyPipe};

const CHROME_UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/157.0.0.0 Safari/537.36";

async fn probe_range(http: &reqwest::Client, url: &str, ua: &str, range: &str, query: bool) {
    let request = if query {
        http.get(url).query(&[("range", range)])
    } else {
        http.get(url).header(reqwest::header::RANGE, format!("bytes={range}"))
    };
    match request.header(reqwest::header::USER_AGENT, ua).send().await {
        Ok(response) => {
            let status = response.status();
            let len = response.content_length();
            let cr = response
                .headers()
                .get(reqwest::header::CONTENT_RANGE)
                .and_then(|h| h.to_str().ok())
                .unwrap_or("-")
                .to_string();
            println!("    range={range} query={query} -> {status} len={len:?} cr={cr}");
        }
        Err(error) => println!("    range={range} query={query} -> error {error}"),
    }
}

#[tokio::main]
async fn main() {
    let storage = std::env::temp_dir().join("nada-stream-probe");
    std::fs::create_dir_all(&storage).unwrap();
    let rp = RustyPipe::builder()
        .storage_dir(storage)
        .build()
        .unwrap();
    let q = rp.query().unauthenticated();
    let query = std::env::args().nth(1).unwrap_or_else(|| "guyon waton".into());
    let results = q.music_search_tracks(&query).await.unwrap();
    let track = &results.items.items[0];
    println!("track {} {}", track.id, track.name);
    let http = reqwest::Client::new();
    let fresh_dir = std::env::temp_dir().join(format!("nada-fresh-{}", std::process::id()));
    std::fs::create_dir_all(&fresh_dir).unwrap();
    let rp2 = RustyPipe::builder().storage_dir(&fresh_dir).build().unwrap();
    let player2 = rp2
        .query()
        .unauthenticated()
        .player_from_client(&track.id, ClientType::Ios)
        .await
        .unwrap();
    let stream2 = player2
        .audio_streams
        .iter()
        .filter(|s| s.drm_track_type.is_none())
        .max_by_key(|s| s.bitrate)
        .unwrap();
    println!("fresh-instance probe:");
    probe_range(&http, &stream2.url, CHROME_UA, "0-1023", false).await;
    probe_range(&http, &stream2.url, CHROME_UA, "1048576-1114111", false).await;
    for round in 0..2 {
        let player = q.player_from_client(&track.id, ClientType::Ios).await.unwrap();
        let stream = player
            .audio_streams
            .iter()
            .filter(|s| s.drm_track_type.is_none())
            .max_by_key(|s| s.bitrate)
            .unwrap();
        let path = std::env::temp_dir()
            .join("nada-js")
            .join(format!("ios-url{round}.txt"));
        std::fs::write(&path, &stream.url).unwrap();
        println!("round {round}: itag={} url saved", stream.itag);
        if round == 1 {
            probe_range(&http, &stream.url, CHROME_UA, "1048576-2097151", false).await;
            probe_range(&http, &stream.url, CHROME_UA, "3145728-3211263", false).await;
        }
    }
    for client in [ClientType::Tv, ClientType::Android, ClientType::Desktop] {
        match q.raw(client, "player", &serde_json::json!({"videoId": track.id})).await {
            Ok(raw) => {
                let value: serde_json::Value = serde_json::from_str(&raw).unwrap();
                let status = value["playabilityStatus"]["status"].as_str().unwrap_or("?");
                let formats = value["streamingData"]["adaptiveFormats"]
                    .as_array()
                    .map(|a| a.len())
                    .unwrap_or(0);
                let direct = value["streamingData"]["adaptiveFormats"]
                    .as_array()
                    .map(|a| a.iter().filter(|f| f.get("url").is_some()).count())
                    .unwrap_or(0);
                println!("raw {client:?}: status={status} formats={formats} direct={direct}");
                if let Some(arr) = value["streamingData"]["adaptiveFormats"].as_array() {
                    for f in arr.iter().filter(|f| f["mimeType"].as_str().is_some_and(|m| m.starts_with("audio/"))).take(2) {
                        println!("  audio itag={} url?={} cipher?={}", f["itag"], f.get("url").is_some(), f.get("signatureCipher").is_some());
                        if let Some(u) = f["url"].as_str() {
                            std::fs::write(std::env::temp_dir().join("nada-js").join(format!("raw-{client:?}.txt")), u).unwrap();
                            probe_range(&http, u, CHROME_UA, "0-1023", false).await;
                            probe_range(&http, u, CHROME_UA, "1048576-2097151", false).await;
                        }
                    }
                }
            }
            Err(error) => println!("raw {client:?}: error {error}"),
        }
    }
    for client in [
        ClientType::Ios,
        ClientType::Tv,
        ClientType::Android,
        ClientType::Desktop,
    ] {
        let player = match q.player_from_client(&track.id, client).await {
            Ok(player) => player,
            Err(error) => {
                println!("{client:?}: player error {error}");
                continue;
            }
        };
        let Some(stream) = player
            .audio_streams
            .iter()
            .filter(|s| s.drm_track_type.is_none())
            .max_by_key(|s| s.bitrate)
        else {
            println!("{client:?}: no audio streams");
            continue;
        };
        let host = reqwest::Url::parse(&stream.url)
            .ok()
            .and_then(|u| u.host_str().map(str::to_string))
            .unwrap_or_default();
        println!(
            "{client:?}: itag={} bitrate={} size={} host={host}",
            stream.itag, stream.bitrate, stream.size
        );
        if client == ClientType::Ios {
            std::fs::write(
                std::env::temp_dir().join("nada-js").join("ios-url.txt"),
                &stream.url,
            )
            .unwrap();
        }
        let client_ua = q.user_agent(client).to_string();
        for (label, ua) in [("chrome", CHROME_UA.to_string()), ("client", client_ua)] {
            println!("  ua={label}");
            probe_range(&http, &stream.url, &ua, "0-1023", false).await;
            probe_range(&http, &stream.url, &ua, "1048576-2097151", false).await;
            probe_range(&http, &stream.url, &ua, "1048576-2097151", true).await;
            probe_stream(&http, &stream.url, &ua, None).await;
            probe_stream(&http, &stream.url, &ua, Some("bytes=0-")).await;
            probe_stream(&http, &stream.url, &ua, Some("bytes=1048576-")).await;
        }
    }
}

async fn probe_stream(http: &reqwest::Client, url: &str, ua: &str, range: Option<&str>) {
    let mut request = http.get(url).header(reqwest::header::USER_AGENT, ua);
    if let Some(range) = range {
        request = request.header(reqwest::header::RANGE, range);
    }
    match request.send().await {
        Ok(mut response) => {
            let status = response.status();
            let mut read = 0usize;
            let mut err = None;
            while read < 3 * 1024 * 1024 {
                match response.chunk().await {
                    Ok(Some(chunk)) => read += chunk.len(),
                    Ok(None) => break,
                    Err(e) => {
                        err = Some(e.to_string());
                        break;
                    }
                }
            }
            println!(
                "    stream range={range:?} -> {status} read={read} err={err:?}"
            );
        }
        Err(error) => println!("    stream range={range:?} -> error {error}"),
    }
}