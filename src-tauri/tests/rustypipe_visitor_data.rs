use std::net::TcpListener;
use std::time::Duration;

use rustypipe::client::RustyPipe;
use rustypipe::error::Error;

#[tokio::test]
async fn visitor_data_transport_failure_returns_error() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind failing proxy");
    let proxy_url = format!("http://{}", listener.local_addr().expect("proxy address"));
    let proxy = reqwest::Proxy::all(&proxy_url).expect("valid proxy");

    let rp = RustyPipe::builder()
        .no_storage()
        .timeout(Duration::from_millis(250))
        .build_with_client(reqwest::ClientBuilder::new().proxy(proxy))
        .expect("build RustyPipe client");

    let result = rp.query().get_visitor_data(true).await;

    assert!(
        matches!(result, Err(Error::Http(_))),
        "expected visitor-data transport error, got {result:?}"
    );
}
