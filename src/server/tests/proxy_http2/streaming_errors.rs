use super::*;

const CONTENT_LENGTH: usize = 100;
const FIRST_CHUNK: &[u8] = b"first";

#[tokio::test]
async fn truncation_after_first_committed_data_closes_without_rewriting() {
    run_committed_failure(
        "HTTP/2 upstream truncation after commit",
        18736,
        Duration::from_secs(5),
        h2::Reason::NO_ERROR,
    )
    .await;
}

#[tokio::test]
async fn read_timeout_after_first_committed_data_closes_without_rewriting() {
    run_committed_failure(
        "HTTP/2 upstream timeout after commit",
        18737,
        Duration::from_millis(100),
        h2::Reason::CANCEL,
    )
    .await;
}

#[tokio::test]
async fn reset_after_first_committed_data_closes_without_rewriting() {
    run_committed_failure(
        "HTTP/2 upstream reset after commit",
        18738,
        Duration::from_secs(5),
        h2::Reason::INTERNAL_ERROR,
    )
    .await;
}

async fn run_committed_failure(
    label: &str,
    port: u16,
    read_timeout: Duration,
    failure: h2::Reason,
) {
    let Some(upstream) = bind_test_listener(label).await else {
        return;
    };
    let upstream_address = upstream.local_addr().unwrap();
    let backend = tokio::spawn(async move {
        let (stream, _) = upstream.accept().await.unwrap();
        let mut connection = h2::server::handshake(stream).await.unwrap();
        let (request, mut respond) = connection.accept().await.unwrap().unwrap();
        assert_eq!(request.uri().path(), "/stream");

        let response = http::Response::builder()
            .status(200)
            .header("Content-Type", "text/plain")
            .header("Content-Length", CONTENT_LENGTH)
            .body(())
            .unwrap();
        let mut body = respond.send_response(response, false).unwrap();
        body.send_data(Bytes::from_static(FIRST_CHUNK), false)
            .unwrap();

        match failure {
            h2::Reason::NO_ERROR => {
                body.send_data(Bytes::from_static(b"short"), true).unwrap();
            }
            h2::Reason::CANCEL => {
                // Poll the connection so the first DATA frame reaches Starling
                // before the upstream enters the stalled-read phase.
                let _ = tokio::time::timeout(Duration::from_millis(300), connection.accept()).await;
                drop(body);
            }
            reason => {
                // Let the queued DATA frame flush before sending RST_STREAM.
                let _ = tokio::time::timeout(Duration::from_millis(50), connection.accept()).await;
                body.send_reset(reason);
            }
        }
        shutdown_h2(connection).await;
    });

    let (prefix, config) = test_config(
        &format!(
            r#"
                worker_processes 1;
                events {{ worker_connections 64; }}
                http {{
                    server {{
                        listen 127.0.0.1:{port};
                        location / {{
                            proxy_pass http://{upstream_address};
                            proxy_http_version 2;
                            proxy_buffering off;
                            proxy_read_timeout {timeout_ms}ms;
                        }}
                    }}
                }}
            "#,
            timeout_ms = read_timeout.as_millis(),
        ),
        &[],
    );
    let group = build_listener_groups(&config)[0].clone();
    let state = test_state(&config);
    let local = group.address.clone();
    let remote: SocketAddr = format!("127.0.0.1:{}", port + 1000).parse().unwrap();
    let (mut client, server) = tokio::io::duplex(64 * 1024);
    let server_task = tokio::spawn(handle_http_connection(
        server,
        ConnectionAddresses::direct(ListenAddress::Inet(remote), local),
        group,
        state,
    ));
    client
        .write_all(b"GET /stream HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();

    let committed = read_committed_prefix(&mut client).await;
    let mut rest = Vec::new();
    tokio::time::timeout(Duration::from_secs(2), client.read_to_end(&mut rest))
        .await
        .expect("failed HTTP/2 upstream should close the committed response")
        .unwrap();
    let response = String::from_utf8([committed, rest].concat()).unwrap();
    assert!(response.starts_with("HTTP/1.1 200 OK\r\n"), "{response}");
    assert!(response.contains("first"), "{response}");
    assert!(!response.contains("short"), "{response}");
    assert!(!response.ends_with("0\r\n\r\n"), "{response}");
    assert!(!response.contains("502 Bad Gateway"), "{response}");
    assert!(!response.contains("504 Gateway Time-out"), "{response}");

    tokio::time::timeout(Duration::from_secs(2), server_task)
        .await
        .expect("HTTP/1 server should stop after the upstream failure")
        .unwrap();
    backend.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

async fn read_committed_prefix(client: &mut tokio::io::DuplexStream) -> Vec<u8> {
    let mut response = Vec::new();
    tokio::time::timeout(Duration::from_secs(1), async {
        while !response
            .windows(FIRST_CHUNK.len())
            .any(|window| window == FIRST_CHUNK)
        {
            let mut buffer = [0_u8; 1024];
            let read = client.read(&mut buffer).await.unwrap();
            assert!(read > 0, "starling closed before committing the response");
            response.extend_from_slice(&buffer[..read]);
        }
    })
    .await
    .expect("the first upstream chunk should reach the client before failure");
    response
}
