use super::support::*;
use super::*;

#[tokio::test]
async fn streams_grpc_data_and_trailers_before_upstream_eof() {
    let Some(upstream) = bind_grpc_listener("gRPC streaming test").await else {
        return;
    };
    let upstream_address = upstream.local_addr().unwrap();
    let (release, wait_for_release) = tokio::sync::oneshot::channel();
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
        let mut stream = respond.send_response(response, false).unwrap();
        stream
            .send_data(Bytes::from_static(b"\0\0\0\0\x05first"), false)
            .unwrap();
        let work = async move {
            wait_for_release.await.unwrap();
            stream
                .send_data(Bytes::from_static(b"\0\0\0\0\x06second"), false)
                .unwrap();
            let mut trailers = http::HeaderMap::new();
            trailers.insert("grpc-status", http::HeaderValue::from_static("0"));
            stream.send_trailers(trailers).unwrap();
        };
        tokio::pin!(work);
        loop {
            tokio::select! {
                () = &mut work => break,
                accepted = connection.accept() => {
                    if accepted.is_none() {
                        work.await;
                        break;
                    }
                }
            }
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
                        listen 127.0.0.1:18715;
                        location / {{
                            grpc_pass grpc://{upstream_address};
                            grpc_read_timeout 5s;
                        }}
                    }}
                }}
            "#
        ),
        &[],
    );
    let group = build_listener_groups(&config)[0].clone();
    let state = test_state(&config);
    let local = group.address.clone();
    let remote: SocketAddr = "127.0.0.1:45715".parse().unwrap();
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
    let first = tokio::time::timeout(Duration::from_secs(1), async {
        while !response
            .windows(10)
            .any(|window| window == b"\0\0\0\0\x05first")
        {
            let mut buffer = [0_u8; 1024];
            let read = client.read(&mut buffer).await.unwrap();
            assert!(read > 0, "starling closed before streaming gRPC data");
            response.extend_from_slice(&buffer[..read]);
        }
    })
    .await;
    if first.is_err() {
        panic!(
            "the first gRPC message should reach the client before upstream EOF: {:?}",
            String::from_utf8_lossy(&response)
        );
    }

    release.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(2), client.read_to_end(&mut response))
        .await
        .expect("streaming gRPC response should complete")
        .unwrap();
    let response = String::from_utf8_lossy(&response);
    assert!(response.starts_with("HTTP/1.1 200 OK\r\n"), "{response}");
    assert!(
        response.find("\0\0\0\0\x05first").unwrap() < response.find("\0\0\0\0\x06second").unwrap(),
        "gRPC messages should retain order"
    );
    assert!(response.contains("grpc-status: 0\r\n"), "{response}");

    tokio::time::timeout(Duration::from_secs(2), server_task)
        .await
        .expect("HTTP server should close after the response")
        .unwrap();
    backend.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}
