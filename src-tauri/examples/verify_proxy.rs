use rustypipe::client::{ClientType, RustyPipe};
use std::time::Duration;

#[path = "../src/proxy.rs"]
mod proxy;

fn main() {
    println!("=== Testing Decoupled Stream Caching Proxy Live ===");

    let temp_dir = std::env::temp_dir().join(format!("nada-verify-proxy-{}", std::process::id()));
    let cache_dir = temp_dir.join("cache");
    let log_path = temp_dir.join("proxy.log");
    let _ = std::fs::remove_dir_all(&temp_dir);
    std::fs::create_dir_all(&cache_dir).unwrap();

    let proxy = proxy::StreamProxy::start(log_path.clone(), cache_dir).expect("proxy failed to start");

    let rt = tokio::runtime::Runtime::new().unwrap();
    let http_client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(45))
        .build()
        .unwrap();

    for (video_id, title) in [("fUdfDKyrbjk", "Mangu"), ("5QMsYmJYUlQ", "Kita Usahakan Lagi")] {
        println!("\nFetching player for: {} ({})", title, video_id);
        let stream = rt.block_on(async {
            let rp = RustyPipe::new();
            let q = rp.query();
            let player = q.player_from_client(video_id, ClientType::Ios).await.unwrap();
            let s = player.audio_streams.into_iter().find(|s| s.itag == 140).unwrap();
            (s.url, s.size, s.mime)
        });
        let (stream_url, stream_size, stream_mime) = stream;
        println!("Stream 140: size={} mime={}", stream_size, stream_mime);

        let proxy_url = proxy.register(&stream_url, Some(stream_size), Some(&stream_mime));
        println!("Registered -> {proxy_url}");

        let resp = http_client.get(&proxy_url).send().expect("send request to proxy");
        println!("Proxy response status: {}", resp.status());

        use std::io::Read;
        let mut reader = resp;
        let mut buf = [0u8; 64 * 1024];
        let mut total_read = 0u64;
        while let Ok(n) = reader.read(&mut buf) {
            if n == 0 { break; }
            total_read += n as u64;
        }
        println!("[SUCCESS] Total read from proxy: {}/{}", total_read, stream_size);
        assert_eq!(total_read, stream_size);
    }

    if let Ok(log_content) = std::fs::read_to_string(&log_path) {
        println!("\n--- Proxy Log ---");
        println!("{}", log_content);
    }

    let _ = std::fs::remove_dir_all(&temp_dir);
}
