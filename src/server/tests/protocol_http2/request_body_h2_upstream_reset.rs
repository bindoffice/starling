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
                        upstream body_retry {{
                            server {first};
                            server {second};
                        }}
                        server {{
                            listen 127.0.0.1:{port} http2;
                            location / {{
                                proxy_pass http://body_retry/;
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
                        upstream body_retry {{
                            server {first};
                            server {second};
                        }}
                        server {{
                            listen 127.0.0.1:{port} http2;
                            location / {{
                                grpc_pass body_retry;
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

    fn frontend_request(self) -> http::Request<()> {
        let mut builder = http::Request::builder()
            .method("POST")
            .uri(format!("http://localhost{}", self.path()))
            .header("content-length", "8");
        if matches!(self, Self::Grpc) {
            builder = builder
                .header("content-type", "application/grpc")
                .header("te", "trailers");
        }
        builder.body(()).unwrap()
    }
}

#[tokio::test]
async fn rst_stream_during_h2_upstream_body_write_cancels_without_retry() {
    run_case(H2Upstream::Proxy, 18669).await;
}

#[tokio::test]
async fn rst_stream_during_grpc_body_write_cancels_without_retry() {
    run_case(H2Upstream::Grpc, 18670).await;
}

async fn run_case(upstream_kind: H2Upstream, port: u16) {
    let Some(first) = bind_test_tcp("HTTP/2 upstream reset backend").await else {
        return;
    };
    let Some(second) = bind_test_tcp("HTTP/2 upstream reset retry backend").await else {
        return;
    };
    let first_address = first.local_addr().unwrap();
    let second_address = second.local_addr().unwrap();
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

        tokio::time::timeout(Duration::from_secs(2), async {
            tokio::select! {
                data = body.data() => {
                    if let Some(Ok(data)) = data {
                        panic!(
                            "{} received unexpected request bytes after reset: {:?}",
                            upstream_kind.label(),
                            data
                        );
                    }
                }
                accepted = connection.accept() => {
                    if accepted.is_some() {
                        panic!(
                            "{} opened another stream after the frontend reset",
                            upstream_kind.label()
                        );
                    }
                }
            }
        })
        .await
        .expect("frontend reset should terminate the streaming upstream body");
        shutdown_h2(connection).await;
    });
    let second_backend = tokio::spawn(async move {
        if tokio::time::timeout(Duration::from_millis(500), second.accept())
            .await
            .is_ok()
        {
            panic!(
                "{} started a retry after the HTTP/2 client reset",
                upstream_kind.label()
            );
        }
    });

    let source = upstream_kind.config(first_address, second_address, port);
    let (prefix, config) = test_config(&source, &[]);
    let group = build_listener_groups(&config)[0].clone();
    let state = test_state(&config);
    let local = group.address.clone();
    let remote: SocketAddr = "127.0.0.1:45688".parse().unwrap();
    let (client_io, server_io) = tokio::io::duplex(64 * 1024);
    let server_task = tokio::spawn(handle_http_connection(
        server_io,
        ConnectionAddresses::direct(ListenAddress::Inet(remote), local),
        group,
        state,
    ));

    let (mut sender, driver) = h2::client::handshake(client_io).await.unwrap();
    let driver = tokio::spawn(driver);
    let (response, mut request_stream) = sender
        .send_request(upstream_kind.frontend_request(), false)
        .unwrap();
    request_stream
        .send_data(Bytes::from_static(b"part"), false)
        .unwrap();
    prefix_seen_rx.await.unwrap();
    request_stream.send_reset(h2::Reason::CANCEL);

    assert!(
        tokio::time::timeout(Duration::from_secs(2), response)
            .await
            .expect("reset response future should complete")
            .is_err()
    );

    drop(sender);
    driver.abort();
    let _ = tokio::time::timeout(Duration::from_secs(2), server_task).await;
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
