use super::super::support::*;
use super::super::*;

const FILE_SIZE: usize = 8 * 1024 * 1024;

#[tokio::test]
async fn serves_an_8_mib_static_file_with_sendfile_and_fallback() {
    for sendfile in ["on", "off"] {
        let Some(listener) = super::bind_test_tcp("HTTP/2 static file").await else {
            return;
        };
        let address = listener.local_addr().unwrap();
        let source = format!(
            r#"
                worker_processes 1;
                events {{ worker_connections 64; }}
                http {{
                    server {{
                        listen {address} http2;
                        root html;
                        sendfile {sendfile};
                        location / {{ }}
                    }}
                }}
            "#
        );
        let line = "0123456789abcdef".repeat(64);
        let body = line.repeat(FILE_SIZE / line.len());
        assert_eq!(body.len(), FILE_SIZE);
        let (prefix, config) = test_config(&source, &[("html/blob.bin", &body)]);
        let group = build_listener_groups(&config)[0].clone();
        let state = test_state(&config);
        let server = tokio::spawn(async move {
            let (stream, remote) = listener.accept().await.unwrap();
            let (stream, control) =
                ClientSocketControl::attach_tcp(stream).expect("attach socket control");
            handle_http_connection_with_reset(
                stream,
                ConnectionAddresses::direct(
                    ListenAddress::Inet(remote),
                    ListenAddress::Inet(address),
                ),
                group,
                state,
                None,
                control,
            )
            .await;
        });

        let stream = TcpStream::connect(address).await.unwrap();
        let (mut sender, connection) = h2::client::handshake(stream).await.unwrap();
        let driver = tokio::spawn(async move {
            let _ = connection.await;
        });
        let request = http::Request::builder()
            .method("GET")
            .uri("http://localhost/blob.bin")
            .body(())
            .unwrap();
        let (response, _) = sender.send_request(request, true).unwrap();
        let response = tokio::time::timeout(Duration::from_secs(5), response)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response
                .headers()
                .get("content-length")
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.parse::<usize>().ok()),
            Some(FILE_SIZE)
        );
        let received = tokio::time::timeout(
            Duration::from_secs(30),
            super::read_h2_response_body(response.into_body()),
        )
        .await
        .expect("HTTP/2 static file body timed out");
        assert_eq!(received.len(), body.len(), "sendfile={sendfile}");
        if received != body.as_bytes() {
            let mismatch = received
                .iter()
                .zip(body.as_bytes())
                .position(|(actual, expected)| actual != expected)
                .unwrap_or(received.len().min(body.len()));
            panic!("HTTP/2 static file body differs at byte {mismatch} (sendfile={sendfile})");
        }

        drop(sender);
        driver.abort();
        let _ = driver.await;
        tokio::time::timeout(Duration::from_secs(5), server)
            .await
            .expect("HTTP/2 static file server did not close")
            .unwrap();
        let _ = std::fs::remove_dir_all(prefix);
    }
}
