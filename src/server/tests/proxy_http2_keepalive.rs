use super::support::*;
use super::*;

#[tokio::test]
async fn reuses_named_upstream_http2_connection_between_requests() {
    let upstream = match TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => listener,
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!("skipping HTTP/2 upstream keepalive test: TCP binding is not permitted");
            return;
        }
        Err(error) => panic!("bind HTTP/2 upstream: {error}"),
    };
    let upstream_address = upstream.local_addr().unwrap();
    let backend = tokio::spawn(async move {
        let (stream, _) = upstream.accept().await.unwrap();
        let mut connection = h2::server::handshake(stream).await.unwrap();
        for (path, body) in [("/one", "first"), ("/two", "second")] {
            let (request, mut respond) = connection.accept().await.unwrap().unwrap();
            assert_eq!(request.uri().path(), path);
            let work = async move {
                let response = http::Response::builder()
                    .status(200)
                    .header("Content-Type", "text/plain")
                    .body(())
                    .unwrap();
                let mut stream = respond.send_response(response, false).unwrap();
                stream
                    .send_data(Bytes::copy_from_slice(body.as_bytes()), true)
                    .unwrap();
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
        }
        shutdown_h2(connection).await;
    });
    let (prefix, config) = test_config(
        &format!(
            r#"
                worker_processes 1;
                events {{ worker_connections 64; }}
                http {{
                    upstream backend {{
                        server {upstream_address};
                        keepalive 2;
                    }}
                    server {{
                        listen 127.0.0.1:18524;
                        location / {{
                            proxy_pass http://backend;
                            proxy_http_version 2;
                        }}
                    }}
                }}
            "#
        ),
        &[],
    );
    let state = test_state(&config);
    let first = request_body_with_state(
        &config,
        state.clone(),
        "GET /one HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    let second = request_body_with_state(
        &config,
        state,
        "GET /two HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;

    backend.await.unwrap();
    assert!(first.contains("first"), "{first}");
    assert!(second.contains("second"), "{second}");
    let _ = std::fs::remove_dir_all(prefix);
}

async fn shutdown_h2(
    mut connection: h2::server::Connection<impl AsyncRead + AsyncWrite + Unpin, Bytes>,
) {
    connection.graceful_shutdown();
    let _ = tokio::time::timeout(Duration::from_secs(1), async {
        while connection.accept().await.is_some() {}
    })
    .await;
}
