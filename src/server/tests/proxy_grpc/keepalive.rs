use super::support::*;
use super::*;

#[tokio::test]
async fn reuses_named_grpc_upstream_keepalive_connections() {
    let Some(upstream) = bind_grpc_listener("gRPC keepalive test").await else {
        return;
    };
    let upstream_address = upstream.local_addr().unwrap();
    let backend = tokio::spawn(async move {
        let (stream, _) = upstream.accept().await.unwrap();
        let mut connection = h2::server::handshake(stream).await.unwrap();
        for (path, body) in [("/one", "first"), ("/two", "second")] {
            let (request, mut respond) = connection.accept().await.unwrap().unwrap();
            let work = async move {
                assert_eq!(request.uri().path(), path);
                let mut request = request.into_body();
                while request.data().await.is_some() {}
                let response = http::Response::builder()
                    .status(200)
                    .header("Content-Type", "application/grpc")
                    .body(())
                    .unwrap();
                let mut stream = respond.send_response(response, false).unwrap();
                stream
                    .send_data(Bytes::copy_from_slice(body.as_bytes()), false)
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
        }
        while connection.accept().await.is_some() {}
    });
    let (prefix, config) = test_config(
        &format!(
            r#"
                worker_processes 1;
                events {{ worker_connections 64; }}
                http {{
                    upstream grpc_backend {{
                        server {upstream_address};
                        keepalive 1;
                    }}
                    server {{
                        listen 127.0.0.1:18572;
                        location / {{
                            grpc_pass grpc_backend;
                        }}
                    }}
                }}
            "#
        ),
        &[],
    );
    let state = test_state(&config);
    for (path, expected) in [("/one", "first"), ("/two", "second")] {
        let response = request_body_with_state(
            &config,
            state.clone(),
            &format!(
                "POST {path} HTTP/1.1\r\n\
                 Host: localhost\r\n\
                 Content-Type: application/grpc\r\n\
                 Content-Length: 0\r\n\
                 Connection: close\r\n\r\n"
            ),
        )
        .await;
        assert!(response.starts_with("HTTP/1.1 200 OK\r\n"), "{response}");
        assert!(response.contains(expected), "{response}");
    }
    drop(state);
    drop(config);
    backend.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}
