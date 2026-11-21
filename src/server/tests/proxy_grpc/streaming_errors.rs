use super::support::*;
use super::*;

const FIRST_MESSAGE: &[u8] = b"\0\0\0\0\x05first";

#[tokio::test]
async fn timeout_after_first_committed_message_closes_without_rewriting() {
    run_committed_failure(
        "gRPC upstream timeout after commit",
        18708,
        Duration::from_millis(100),
        h2::Reason::CANCEL,
    )
    .await;
}

#[tokio::test]
async fn reset_after_first_committed_message_closes_without_rewriting() {
    run_committed_failure(
        "gRPC upstream reset after commit",
        18709,
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
    let Some(upstream) = bind_grpc_listener(label).await else {
        return;
    };
    let upstream_address = upstream.local_addr().unwrap();
    let backend = tokio::spawn(async move {
        let (stream, _) = upstream.accept().await.unwrap();
        let mut connection = h2::server::handshake(stream).await.unwrap();
        let (request, mut respond) = connection.accept().await.unwrap().unwrap();
        assert_eq!(request.uri().path(), "/pkg.Service/Stream");

        let response = http::Response::builder()
            .status(200)
            .header("Content-Type", "application/grpc")
            .body(())
            .unwrap();
        let mut body = respond.send_response(response, false).unwrap();
        body.send_data(Bytes::from_static(FIRST_MESSAGE), false)
            .unwrap();

        if failure == h2::Reason::CANCEL {
            // Poll the connection so the first gRPC frame reaches Starling
            // before the upstream stops making read progress.
            let _ = tokio::time::timeout(Duration::from_millis(300), connection.accept()).await;
            drop(body);
        } else {
            // Let the queued DATA frame flush before sending RST_STREAM.
            let _ = tokio::time::timeout(Duration::from_millis(50), connection.accept()).await;
            body.send_reset(failure);
        }
        shutdown_grpc_h2(connection).await;
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
                            grpc_pass grpc://{upstream_address};
                            grpc_read_timeout {timeout_ms}ms;
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
        .write_all(
            b"POST /pkg.Service/Stream HTTP/1.1\r\n\
              Host: localhost\r\n\
              Content-Type: application/grpc\r\n\
              Content-Length: 0\r\n\
              Connection: close\r\n\r\n",
        )
        .await
        .unwrap();

    let mut response = Vec::new();
    tokio::time::timeout(Duration::from_secs(1), async {
        while !response
            .windows(FIRST_MESSAGE.len())
            .any(|window| window == FIRST_MESSAGE)
        {
            let mut buffer = [0_u8; 1024];
            let read = client.read(&mut buffer).await.unwrap();
            assert!(read > 0, "starling closed before streaming gRPC data");
            response.extend_from_slice(&buffer[..read]);
        }
    })
    .await
    .expect("the first gRPC message should reach the client before failure");

    tokio::time::timeout(Duration::from_secs(2), client.read_to_end(&mut response))
        .await
        .expect("failed gRPC upstream should close the committed response")
        .unwrap();
    let text = String::from_utf8_lossy(&response);
    assert!(text.starts_with("HTTP/1.1 200 OK\r\n"), "{text}");
    assert!(text.contains("application/grpc"), "{text}");
    assert!(!text.contains("grpc-status:"), "{text}");
    assert!(!text.contains("502 Bad Gateway"), "{text}");
    assert!(!text.contains("504 Gateway Time-out"), "{text}");
    assert!(!response.ends_with(b"0\r\n\r\n"), "{text}");

    tokio::time::timeout(Duration::from_secs(2), server_task)
        .await
        .expect("HTTP/1 server should stop after the gRPC failure")
        .unwrap();
    backend.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}
