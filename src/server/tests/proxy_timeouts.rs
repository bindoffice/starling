use super::support::*;
use super::*;

#[tokio::test]
async fn applies_proxy_send_timeout_to_upstream_writes() {
    let (prefix, config) = proxy_timeout_config(18120, "20ms", "1s");
    let (_peer, connection) = timeout_connection(256);
    let outgoing = proxy_request(vec![b'x'; 64 * 1024]);
    let started = Instant::now();
    let result = timed_connection_proxy_exchange(
        connection,
        &outgoing,
        "1.1",
        &config.http.as_ref().unwrap().servers[0].context,
        0,
        false,
        false,
    )
    .await;
    assert!(matches!(result, Err(ProxyFailure::Timeout)));
    assert!(
        started.elapsed() < Duration::from_millis(500),
        "proxy_send_timeout should expire before proxy_read_timeout"
    );
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn applies_proxy_read_timeout_to_upstream_responses() {
    let (prefix, config) = proxy_timeout_config(18121, "1s", "20ms");
    let (_peer, connection) = timeout_connection(4096);
    let outgoing = proxy_request(Vec::new());
    let started = Instant::now();
    let result = timed_connection_proxy_exchange(
        connection,
        &outgoing,
        "1.1",
        &config.http.as_ref().unwrap().servers[0].context,
        0,
        false,
        false,
    )
    .await;
    assert!(matches!(result, Err(ProxyFailure::Timeout)));
    assert!(
        started.elapsed() < Duration::from_millis(500),
        "proxy_read_timeout should expire before proxy_send_timeout"
    );
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn proxy_send_timeout_tracks_partial_write_progress_not_total_duration() {
    let (prefix, config) = proxy_timeout_config(18125, "60ms", "1s");
    let (mut peer, connection) = timeout_connection(32);
    let body = b"abcdefgh";
    let writer = tokio::spawn(async move {
        let mut head = Vec::new();
        while !head.windows(4).any(|window| window == b"\r\n\r\n") {
            let mut byte = [0_u8; 1];
            peer.read_exact(&mut byte).await.unwrap();
            head.extend_from_slice(&byte);
        }
        for _ in 0..body.len() {
            let mut byte = [0_u8; 1];
            peer.read_exact(&mut byte).await.unwrap();
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        peer.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok")
            .await
            .unwrap();
    });
    let outgoing = proxy_request(body.to_vec());
    let exchange = timed_connection_proxy_exchange(
        connection,
        &outgoing,
        "1.1",
        &config.http.as_ref().unwrap().servers[0].context,
        0,
        false,
        false,
    )
    .await
    .unwrap();
    let response = exchange.response;
    assert_eq!(response.status, 200);
    assert_eq!(response.body.content_length(), Some(2));
    writer.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn proxy_read_timeout_tracks_header_progress_not_total_duration() {
    let (prefix, config) = proxy_timeout_config(18122, "1s", "500ms");
    let (mut peer, connection) = timeout_connection(4096);
    let writer = tokio::spawn(async move {
        read_timeout_request_head(&mut peer).await;
        for chunk in [
            b"HTTP/1.1 200 OK\r\n".as_slice(),
            b"Content-Type: text/plain\r\n",
            b"Content-Length: 2\r\n",
            b"Connection: close\r\n",
            b"\r\nok",
        ] {
            peer.write_all(chunk).await.unwrap();
            peer.flush().await.unwrap();
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    });
    let outgoing = proxy_request(Vec::new());
    let exchange = timed_connection_proxy_exchange(
        connection,
        &outgoing,
        "1.1",
        &config.http.as_ref().unwrap().servers[0].context,
        0,
        false,
        false,
    )
    .await
    .unwrap();
    let response = exchange.response;
    assert_eq!(response.status, 200);
    assert_eq!(response.body.content_length(), Some(2));
    writer.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn proxy_read_timeout_tracks_body_progress_not_total_duration() {
    let (prefix, config) = proxy_timeout_config(18124, "1s", "500ms");
    let (mut peer, connection) = timeout_connection(4096);
    let writer = tokio::spawn(async move {
        read_timeout_request_head(&mut peer).await;
        peer.write_all(
            b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\n\
              Content-Length: 5\r\nConnection: close\r\n\r\n",
        )
        .await
        .unwrap();
        peer.flush().await.unwrap();
        for byte in *b"abcde" {
            peer.write_all(&[byte]).await.unwrap();
            peer.flush().await.unwrap();
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    });
    let outgoing = proxy_request(Vec::new());
    let exchange = timed_connection_proxy_exchange(
        connection,
        &outgoing,
        "1.1",
        &config.http.as_ref().unwrap().servers[0].context,
        0,
        false,
        false,
    )
    .await
    .unwrap();
    let response = exchange.response;
    assert_eq!(response.status, 200);
    assert_eq!(response.body.content_length(), Some(5));
    writer.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

fn proxy_timeout_config(
    port: u16,
    send_timeout: &str,
    read_timeout: &str,
) -> (PathBuf, Arc<RuntimeConfig>) {
    test_config(
        &format!(
            r#"
                worker_processes 1;
                events {{ worker_connections 64; }}
                http {{
                    server {{
                        listen 127.0.0.1:{port};
                        proxy_send_timeout {send_timeout};
                        proxy_read_timeout {read_timeout};
                    }}
                }}
            "#
        ),
        &[],
    )
}

fn timeout_connection(capacity: usize) -> (tokio::io::DuplexStream, UpstreamConnection) {
    let (client, peer) = tokio::io::duplex(capacity);
    (
        peer,
        UpstreamConnection::new(Box::new(client), timeout_key()),
    )
}

fn proxy_request(body: Vec<u8>) -> ProxyRequestMessage {
    ProxyRequestMessage {
        method: "POST".into(),
        path: "/".into(),
        headers: vec![("Host".into(), "upstream".into())],
        body,
        body_stream: None,
        body_chunked: false,
        body_limit: 0,
    }
}

fn timeout_key() -> UpstreamPoolKey {
    UpstreamPoolKey {
        generation: 0,
        scheme: "http".into(),
        host: "127.0.0.1".into(),
        port: 80,
        tls_name: None,
        tls_verify: false,
        tls_trusted_certificate: None,
        tls_verify_depth: None,
        tls_crl: None,
        tls_client_certificate: None,
        tls_client_certificate_key: None,
        tls_protocols: None,
        tls_ciphers: None,
        proxy_bind: None,
        socket_keepalive: false,
        socket_rcvbuf: 0,
        socket_sndbuf: 0,
        http_version: "1.1".into(),
    }
}

async fn read_timeout_request_head<R>(stream: &mut R)
where
    R: AsyncRead + Unpin,
{
    let mut request = Vec::new();
    let mut buffer = [0_u8; 1024];
    while !request.windows(4).any(|window| window == b"\r\n\r\n") {
        let read = stream.read(&mut buffer).await.unwrap();
        assert!(read > 0, "upstream request closed before headers");
        request.extend_from_slice(&buffer[..read]);
    }
}
