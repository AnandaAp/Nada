use std::io::Read;

fn main() {
    let url = std::env::args().nth(1).expect("url arg");
    let url = url.trim();
    let client = reqwest::blocking::Client::builder()
        .user_agent(
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/157.0.0.0 Safari/537.36",
        )
        .build()
        .unwrap();
    for range in ["bytes=0-65535", "bytes=0-1048575", "bytes=0-4194303"] {
        let mut resp = client
            .get(url)
            .header(reqwest::header::RANGE, range)
            .send()
            .unwrap();
        let status = resp.status();
        let mut buf = Vec::new();
        let n = resp.read_to_end(&mut buf).unwrap_or(0);
        println!("reqwest {range} -> {status} read={n}");
    }
}