use super::*;

#[derive(Clone, Copy, Debug)]
enum RawGateway {
    Uwsgi,
    Scgi,
}

impl RawGateway {
    fn port(self) -> u16 {
        match self {
            Self::Uwsgi => 18157,
            Self::Scgi => 18158,
        }
    }

    fn directive(self, value: &str) -> String {
        match self {
            Self::Uwsgi => format!(
                "uwsgi_pass 127.0.0.1:{}; uwsgi_request_buffering off; \
                 uwsgi_buffering off; uwsgi_send_timeout {value}; uwsgi_read_timeout 1s;",
                self.port()
            ),
            Self::Scgi => format!(
                "scgi_pass 127.0.0.1:{}; scgi_request_buffering off; \
                 scgi_buffering off; scgi_send_timeout {value}; scgi_read_timeout 1s;",
                self.port()
            ),
        }
    }
}

#[tokio::test]
async fn uwsgi_send_timeout_expires_after_a_streaming_body_write_stalls() {
    send_timeout_expires_after_body_stall(RawGateway::Uwsgi).await;
}

#[tokio::test]
async fn scgi_send_timeout_expires_after_a_streaming_body_write_stalls() {
    send_timeout_expires_after_body_stall(RawGateway::Scgi).await;
}

#[tokio::test]
async fn uwsgi_send_timeout_tracks_partial_write_progress() {
    send_timeout_tracks_partial_progress(RawGateway::Uwsgi).await;
}

#[tokio::test]
async fn scgi_send_timeout_tracks_partial_write_progress() {
    send_timeout_tracks_partial_progress(RawGateway::Scgi).await;
}

#[tokio::test]
async fn fastcgi_send_timeout_tracks_partial_record_progress() {
    let port = 18159;
    let (prefix, config) = test_config(
        &format!(
            r#"
                worker_processes 1;
                events {{ worker_connections 64; }}
                http {{
                    server {{
                        listen 127.0.0.1:{port};
                        location / {{
                            fastcgi_pass 127.0.0.1:9000;
                            fastcgi_request_buffering off;
                            fastcgi_buffering off;
                            fastcgi_send_timeout 30ms;
                            fastcgi_read_timeout 1s;
                        }}
                    }}
                }}
            "#
        ),
        &[],
    );
    let server = &config.http.as_ref().unwrap().servers[0];
    let context = server.locations[0].context.clone();
    let (sender, request) = streaming_request("/", &format!("127.0.0.1:{port}"), 4);
    let variables = Variables::new(&request, "localhost", server, &remote(), &local(port));
    let env = test_env();
    let (client, mut backend) = tokio::io::duplex(1);
    let backend_task = tokio::spawn(async move {
        let mut body = Vec::new();
        loop {
            let record = read_fastcgi_record_slow(&mut backend).await;
            if record.record_type == FCGI_STDIN {
                if record.content.is_empty() {
                    break;
                }
                body.extend_from_slice(&record.content);
            }
        }
        assert_eq!(body, b"part");
        write_fastcgi_record(
            &mut backend,
            FCGI_STDOUT,
            b"Status: 200 OK\r\nContent-Type: text/plain\r\n\r\nok",
        )
        .await
        .unwrap();
        write_fastcgi_record(&mut backend, FCGI_END_REQUEST, &[0; 8])
            .await
            .unwrap();
        backend.flush().await.unwrap();
    });
    let exchange_context = context.clone();
    let exchange_variables = variables.clone();
    let exchange = tokio::spawn(async move {
        fastcgi_exchange(
            client,
            &request,
            &env,
            None,
            &exchange_context,
            &exchange_variables,
        )
        .await
    });

    sender.send(Bytes::from_static(b"part")).await;
    drop(sender);
    let result = tokio::time::timeout(Duration::from_secs(3), exchange)
        .await
        .expect("partially progressing FastCGI records should not time out")
        .unwrap();
    let response = result.unwrap();
    assert_eq!(response.status, 200);
    assert_eq!(
        response
            .body
            .into_memory(MAX_BODY_BYTES as u64)
            .await
            .unwrap(),
        b"ok"
    );
    backend_task.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn uwsgi_send_timeout_does_not_cover_client_body_gaps() {
    send_timeout_does_not_cover_client_body_gaps(RawGateway::Uwsgi).await;
}

#[tokio::test]
async fn scgi_send_timeout_does_not_cover_client_body_gaps() {
    send_timeout_does_not_cover_client_body_gaps(RawGateway::Scgi).await;
}

async fn send_timeout_expires_after_body_stall(gateway: RawGateway) {
    let port = gateway.port();
    let source = format!(
        r#"
            worker_processes 1;
            events {{ worker_connections 64; }}
            http {{
                server {{
                    listen 127.0.0.1:{port};
                    location / {{
                        {}
                    }}
                }}
            }}
        "#,
        gateway.directive("20ms")
    );
    let (prefix, config) = test_config(&source, &[]);
    let server = &config.http.as_ref().unwrap().servers[0];
    let context = server.locations[0].context.clone();
    let body_len = 64 * 1024;
    let (sender, request) = streaming_request("/", &format!("127.0.0.1:{port}"), body_len);
    let variables = Variables::new(&request, "localhost", server, &remote(), &local(port));
    let (client, mut backend) = tokio::io::duplex(1);
    let backend_task = tokio::spawn(async move {
        match gateway {
            RawGateway::Uwsgi => read_uwsgi_header(&mut backend).await,
            RawGateway::Scgi => read_scgi_header(&mut backend).await,
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    });
    let exchange_context = context.clone();
    let exchange_variables = variables.clone();
    let exchange = tokio::spawn(async move {
        exchange(
            gateway,
            client,
            &request,
            &exchange_context,
            &exchange_variables,
        )
        .await
    });

    sender.send(Bytes::from(vec![b'x'; body_len])).await;
    let result = tokio::time::timeout(Duration::from_secs(1), exchange)
        .await
        .expect("streaming body send timeout should not hang")
        .unwrap();
    assert!(
        matches!(result, Err(ProxyFailure::Timeout)),
        "expected upstream send timeout"
    );
    backend_task.abort();
    let _ = std::fs::remove_dir_all(prefix);
}

async fn send_timeout_does_not_cover_client_body_gaps(gateway: RawGateway) {
    let port = gateway.port();
    let source = format!(
        r#"
            worker_processes 1;
            events {{ worker_connections 64; }}
            http {{
                server {{
                    listen 127.0.0.1:{port};
                    location / {{
                        {}
                        {}
                    }}
                }}
            }}
        "#,
        gateway.directive("20ms"),
        gateway.parameter()
    );
    let (prefix, config) = test_config(&source, &[]);
    let server = &config.http.as_ref().unwrap().servers[0];
    let context = server.locations[0].context.clone();
    let (sender, request) = streaming_request("/", &format!("127.0.0.1:{port}"), 8);
    let variables = Variables::new(&request, "localhost", server, &remote(), &local(port));
    let (header_seen_tx, header_seen_rx) = tokio::sync::oneshot::channel();
    let (first_body_seen_tx, first_body_seen_rx) = tokio::sync::oneshot::channel();
    let (client, mut backend) = tokio::io::duplex(64 * 1024);
    let backend_task = tokio::spawn(async move {
        match gateway {
            RawGateway::Uwsgi => read_uwsgi_header(&mut backend).await,
            RawGateway::Scgi => read_scgi_header(&mut backend).await,
        }
        header_seen_tx.send(()).unwrap();
        let mut first = [0_u8; 4];
        backend.read_exact(&mut first).await.unwrap();
        assert_eq!(&first, b"part");
        first_body_seen_tx.send(()).unwrap();
        let mut second = [0_u8; 4];
        backend.read_exact(&mut second).await.unwrap();
        assert_eq!(&second, b"more");
        backend
            .write_all(b"Status: 200 OK\r\nContent-Type: text/plain\r\n\r\nok")
            .await
            .unwrap();
        backend.shutdown().await.unwrap();
    });
    let exchange =
        tokio::spawn(
            async move { exchange(gateway, client, &request, &context, &variables).await },
        );

    tokio::time::timeout(Duration::from_secs(1), header_seen_rx)
        .await
        .expect("the gateway header should reach upstream")
        .unwrap();
    tokio::time::sleep(Duration::from_millis(80)).await;
    sender.send(Bytes::from_static(b"part")).await;
    tokio::time::timeout(Duration::from_secs(1), first_body_seen_rx)
        .await
        .expect("the first body chunk should reach upstream")
        .unwrap();
    tokio::time::sleep(Duration::from_millis(80)).await;
    sender.send(Bytes::from_static(b"more")).await;
    drop(sender);

    let response = tokio::time::timeout(Duration::from_secs(2), exchange)
        .await
        .expect("waiting for the next client chunk should not consume send timeout")
        .unwrap()
        .unwrap();
    assert_eq!(response.status, 200);
    backend_task.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

async fn send_timeout_tracks_partial_progress(gateway: RawGateway) {
    let port = gateway.port();
    let source = format!(
        r#"
            worker_processes 1;
            events {{ worker_connections 64; }}
            http {{
                server {{
                    listen 127.0.0.1:{port};
                    location / {{
                        {}
                        {}
                    }}
                }}
            }}
        "#,
        gateway.directive("30ms"),
        gateway.parameter()
    );
    let (prefix, config) = test_config(&source, &[]);
    let server = &config.http.as_ref().unwrap().servers[0];
    let context = server.locations[0].context.clone();
    let (sender, request) = streaming_request("/", &format!("127.0.0.1:{port}"), 4);
    let variables = Variables::new(&request, "localhost", server, &remote(), &local(port));
    let (client, mut backend) = tokio::io::duplex(1);
    let backend_task = tokio::spawn(async move {
        match gateway {
            RawGateway::Uwsgi => read_uwsgi_header_slow(&mut backend).await,
            RawGateway::Scgi => read_scgi_header_slow(&mut backend).await,
        }
        let body = read_slow(&mut backend, 4).await;
        assert_eq!(&body, b"part");
        backend
            .write_all(b"Status: 200 OK\r\nContent-Type: text/plain\r\n\r\nok")
            .await
            .unwrap();
        backend.shutdown().await.unwrap();
    });
    let exchange_context = context.clone();
    let exchange_variables = variables.clone();
    let exchange = tokio::spawn(async move {
        exchange(
            gateway,
            client,
            &request,
            &exchange_context,
            &exchange_variables,
        )
        .await
    });

    sender.send(Bytes::from_static(b"part")).await;
    drop(sender);
    let result = tokio::time::timeout(Duration::from_secs(2), exchange)
        .await
        .expect("partially progressing gateway writes should not time out")
        .unwrap();
    let response = result.unwrap();
    assert_eq!(response.status, 200);
    assert_eq!(
        response
            .body
            .into_memory(MAX_BODY_BYTES as u64)
            .await
            .unwrap(),
        b"ok"
    );
    backend_task.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

impl RawGateway {
    fn parameter(self) -> &'static str {
        match self {
            Self::Uwsgi => "uwsgi_param REQUEST_METHOD $request_method;",
            Self::Scgi => "scgi_param REQUEST_METHOD $request_method;",
        }
    }
}

async fn exchange<S>(
    gateway: RawGateway,
    stream: S,
    request: &Request,
    context: &EffectiveConfig,
    variables: &Variables,
) -> Result<Response, ProxyFailure>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    match gateway {
        RawGateway::Uwsgi => uwsgi_exchange(stream, request, context, variables).await,
        RawGateway::Scgi => scgi_exchange(stream, request, context, variables).await,
    }
}

async fn read_uwsgi_header_slow<R: AsyncRead + Unpin>(reader: &mut R) {
    let header = read_slow(reader, 4).await;
    let length = u16::from_le_bytes([header[1], header[2]]) as usize;
    let _ = read_slow(reader, length).await;
}

async fn read_scgi_header_slow<R: AsyncRead + Unpin>(reader: &mut R) {
    let mut length = Vec::new();
    loop {
        let byte = read_slow(reader, 1).await;
        if byte[0] == b':' {
            break;
        }
        length.push(byte[0]);
    }
    let length = std::str::from_utf8(&length).unwrap().parse().unwrap();
    let _ = read_slow(reader, length).await;
}

async fn read_fastcgi_record_slow<R: AsyncRead + Unpin>(reader: &mut R) -> FastCgiRecord {
    let header = read_slow(reader, 8).await;
    assert_eq!(header[0], 1);
    let content_length = u16::from_be_bytes([header[4], header[5]]) as usize;
    let padding = header[6] as usize;
    let content = read_slow(reader, content_length).await;
    let _ = read_slow(reader, padding).await;
    FastCgiRecord {
        record_type: header[1],
        content,
    }
}

async fn read_slow<R: AsyncRead + Unpin>(reader: &mut R, length: usize) -> Vec<u8> {
    let mut result = vec![0_u8; length];
    for byte in &mut result {
        reader.read_exact(std::slice::from_mut(byte)).await.unwrap();
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    result
}
