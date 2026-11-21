use super::support::*;
use super::*;

#[derive(Clone, Copy)]
enum H2Upstream {
    Proxy,
    Grpc,
}

impl H2Upstream {
    fn label(self) -> &'static str {
        match self {
            Self::Proxy => "HTTP/2 proxy",
            Self::Grpc => "gRPC",
        }
    }

    fn path(self) -> &'static str {
        match self {
            Self::Proxy => "/upload",
            Self::Grpc => "/pkg.Service/Cancel",
        }
    }

    fn config(self, first: SocketAddr, second: SocketAddr, port: u16) -> String {
        match self {
            Self::Proxy => format!(
                r#"
                    worker_processes 1;
                    events {{ worker_connections 64; }}
                    http {{
                        upstream h2_retry {{
                            server {first};
                            server {second};
                        }}
                        server {{
                            listen 127.0.0.1:{port};
                            location / {{
                                proxy_pass http://h2_retry/;
                                proxy_http_version 2;
                                proxy_request_buffering off;
                                proxy_next_upstream error timeout non_idempotent;
                                proxy_next_upstream_tries 2;
                                proxy_read_timeout 5s;
                            }}
                        }}
                    }}
                "#
            ),
            Self::Grpc => format!(
                r#"
                    worker_processes 1;
                    events {{ worker_connections 64; }}
                    http {{
                        upstream grpc_retry {{
                            server {first};
                            server {second};
                        }}
                        server {{
                            listen 127.0.0.1:{port};
                            location / {{
                                grpc_pass grpc_retry;
                                grpc_request_buffering off;
                                grpc_next_upstream error timeout non_idempotent;
                                grpc_next_upstream_tries 2;
                                grpc_read_timeout 5s;
                            }}
                        }}
                    }}
                "#
            ),
        }
    }

    fn request(self) -> Vec<u8> {
        match self {
            Self::Proxy => b"POST /upload HTTP/1.1\r\nHost: localhost\r\nContent-Length: 8\r\n\
                  Connection: close\r\n\r\npart"
                .to_vec(),
            Self::Grpc => b"POST /pkg.Service/Cancel HTTP/1.1\r\nHost: localhost\r\n\
                  Content-Type: application/grpc\r\nTE: trailers\r\nContent-Length: 8\r\n\
                  Connection: close\r\n\r\npart"
                .to_vec(),
        }
    }
}

#[tokio::test]
async fn http2_upstream_body_write_cancels_on_client_half_close() {
    run_case(H2Upstream::Proxy, 18755).await;
}

#[tokio::test]
async fn grpc_upstream_body_write_cancels_on_client_half_close() {
    run_case(H2Upstream::Grpc, 18756).await;
}

async fn run_case(upstream_kind: H2Upstream, port: u16) {
    let Some((first, frontend_listener)) = bind_listeners().await else {
        return;
    };
    let second = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let first_address = first.local_addr().unwrap();
    let second_address = second.local_addr().unwrap();
    let frontend = frontend_listener.local_addr().unwrap();
    let (prefix_seen, prefix_seen_rx) = tokio::sync::oneshot::channel();
    let first_backend = tokio::spawn(async move {
        let (stream, _) = first.accept().await.unwrap();
        let mut connection = h2::server::handshake(stream).await.unwrap();
        let (request, _respond) = connection.accept().await.unwrap().unwrap();
        assert_eq!(request.uri().path(), upstream_kind.path());
        let mut body = request.into_body();
        let first = next_data(&mut connection, &mut body).await;
        assert_eq!(&first[..], b"part");
        prefix_seen.send(()).unwrap();

        let next = body.data();
        tokio::pin!(next);
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                tokio::select! {
                    data = &mut next => {
                        match data {
                            Some(Ok(data)) => {
                                panic!(
                                    "{} received unexpected request bytes after half-close: {:?}",
                                    upstream_kind.label(),
                                    data
                                );
                            }
                            Some(Err(_)) | None => break,
                        }
                    }
                    accepted = connection.accept() => {
                        if accepted.is_none() {
                            break;
                        }
                    }
                }
            }
        })
        .await
        .expect("half-closed client should close the upstream HTTP/2 connection");
        shutdown_h2(connection).await;
    });
    let second_backend = tokio::spawn(async move {
        if tokio::time::timeout(Duration::from_millis(500), second.accept())
            .await
            .is_ok()
        {
            panic!(
                "{} started a retry after the client half-closed",
                upstream_kind.label()
            );
        }
    });

    let (prefix, config) = test_config(
        &upstream_kind.config(first_address, second_address, port),
        &[],
    );
    let server_task = spawn_frontend(frontend_listener, &config);
    let mut client = TcpStream::connect(frontend).await.unwrap();
    client.write_all(&upstream_kind.request()).await.unwrap();
    tokio::time::timeout(Duration::from_secs(1), prefix_seen_rx)
        .await
        .expect("upstream should receive the first request-body chunk")
        .unwrap();
    client.shutdown().await.unwrap();

    let mut response = Vec::new();
    tokio::time::timeout(Duration::from_secs(2), client.read_to_end(&mut response))
        .await
        .expect("half-closed client should close the downstream connection")
        .unwrap();
    assert!(
        response.is_empty(),
        "{} half-close must not synthesize a response: {}",
        upstream_kind.label(),
        String::from_utf8_lossy(&response)
    );

    tokio::time::timeout(Duration::from_secs(2), server_task)
        .await
        .expect("connection handler should stop after the half-close")
        .unwrap();
    first_backend.await.unwrap();
    second_backend.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

async fn next_data(
    connection: &mut h2::server::Connection<impl AsyncRead + AsyncWrite + Unpin, Bytes>,
    body: &mut h2::RecvStream,
) -> Bytes {
    let next = body.data();
    tokio::pin!(next);
    loop {
        tokio::select! {
            data = &mut next => return data.unwrap().unwrap(),
            accepted = connection.accept() => {
                assert!(accepted.is_some(), "upstream connection closed before request data");
            }
        }
    }
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
