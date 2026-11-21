use super::*;

#[derive(Clone, Copy)]
enum RawGateway {
    Uwsgi,
    Scgi,
}

#[tokio::test]
async fn uwsgi_streams_first_chunk_before_upstream_eof() {
    raw_gateway_streams_before_eof(
        RawGateway::Uwsgi,
        r#"
            location / {
                uwsgi_pass 127.0.0.1:9001;
            }
        "#,
        true,
    )
    .await;
}

#[tokio::test]
async fn scgi_streams_first_chunk_before_upstream_eof() {
    raw_gateway_streams_before_eof(
        RawGateway::Scgi,
        r#"
            location / {
                scgi_pass 127.0.0.1:9002;
                scgi_buffering off;
            }
        "#,
        false,
    )
    .await;
}

#[tokio::test]
async fn uwsgi_buffered_mode_streams_after_the_busy_threshold() {
    raw_gateway_streams_after_busy_threshold(
        RawGateway::Uwsgi,
        r#"
            location / {
                uwsgi_pass 127.0.0.1:9001;
                uwsgi_buffers 2 4k;
                uwsgi_busy_buffers_size 4k;
            }
        "#,
    )
    .await;
}

#[tokio::test]
async fn scgi_buffered_mode_streams_after_the_busy_threshold() {
    raw_gateway_streams_after_busy_threshold(
        RawGateway::Scgi,
        r#"
            location / {
                scgi_pass 127.0.0.1:9002;
                scgi_buffers 2 4k;
                scgi_busy_buffers_size 4k;
            }
        "#,
    )
    .await;
}

async fn raw_gateway_streams_before_eof(
    gateway: RawGateway,
    location: &str,
    use_accel_header: bool,
) {
    let source = format!(
        r#"
            worker_processes 1;
            events {{ worker_connections 64; }}
            http {{
                server {{
                    listen 127.0.0.1:18124;
                    {location}
                }}
            }}
        "#
    );
    let (prefix, config) = test_config(&source, &[]);
    let server = &config.http.as_ref().unwrap().servers[0];
    let context = &server.locations[0].context;
    let request = test_request("/");
    let remote: SocketAddr = "127.0.0.1:45678".parse().unwrap();
    let local: SocketAddr = "127.0.0.1:18124".parse().unwrap();
    let variables = Variables::new(&request, "localhost", server, &local, &remote);
    let (client, mut backend) = tokio::io::duplex(64 * 1024);
    let (continue_tx, continue_rx) = tokio::sync::oneshot::channel();
    let backend_task = tokio::spawn(async move {
        match gateway {
            RawGateway::Uwsgi => read_test_uwsgi_packet(&mut backend).await,
            RawGateway::Scgi => read_test_scgi_packet(&mut backend).await,
        }
        let extra_header = if use_accel_header {
            "X-Accel-Buffering: no\r\n"
        } else {
            ""
        };
        backend
            .write_all(
                format!("Status: 200 OK\r\nContent-Type: text/plain\r\n{extra_header}\r\nfirst")
                    .as_bytes(),
            )
            .await
            .unwrap();
        backend.flush().await.unwrap();
        continue_rx.await.unwrap();
        backend.write_all(b"second").await.unwrap();
        backend.shutdown().await.unwrap();
    });

    let response = match gateway {
        RawGateway::Uwsgi => uwsgi_exchange(client, &request, context, &variables).await,
        RawGateway::Scgi => scgi_exchange(client, &request, context, &variables).await,
    }
    .unwrap();
    assert_eq!(response.status, 200);
    assert_eq!(response.header("Content-Length"), None);
    let ResponseBody::Stream(mut stream) = response.body else {
        panic!("gateway response should be streamed");
    };
    assert_eq!(next_stream_data(&mut stream).await, b"first");
    continue_tx.send(()).unwrap();
    assert_eq!(next_stream_data(&mut stream).await, b"second");
    assert!(stream.next().await.unwrap().is_none());
    backend_task.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

async fn raw_gateway_streams_after_busy_threshold(gateway: RawGateway, location: &str) {
    let source = format!(
        r#"
            worker_processes 1;
            events {{ worker_connections 64; }}
            http {{
                server {{
                    listen 127.0.0.1:18127;
                    {location}
                }}
            }}
        "#
    );
    let (prefix, config) = test_config(&source, &[]);
    let server = &config.http.as_ref().unwrap().servers[0];
    let context = &server.locations[0].context;
    let request = test_request("/");
    let remote: SocketAddr = "127.0.0.1:45678".parse().unwrap();
    let local: SocketAddr = "127.0.0.1:18127".parse().unwrap();
    let variables = Variables::new(&request, "localhost", server, &local, &remote);
    let (client, mut backend) = tokio::io::duplex(64 * 1024);
    let (continue_tx, continue_rx) = tokio::sync::oneshot::channel();
    let first = "a".repeat(8 * 1024);
    let expected_first = first.clone();
    let backend_task = tokio::spawn(async move {
        match gateway {
            RawGateway::Uwsgi => read_test_uwsgi_packet(&mut backend).await,
            RawGateway::Scgi => read_test_scgi_packet(&mut backend).await,
        }
        backend
            .write_all(
                format!("Status: 200 OK\r\nContent-Type: text/plain\r\n\r\n{first}").as_bytes(),
            )
            .await
            .unwrap();
        backend.flush().await.unwrap();
        continue_rx.await.unwrap();
        backend.write_all(b"second").await.unwrap();
        backend.shutdown().await.unwrap();
    });

    let response = match gateway {
        RawGateway::Uwsgi => uwsgi_exchange(client, &request, context, &variables).await,
        RawGateway::Scgi => scgi_exchange(client, &request, context, &variables).await,
    }
    .unwrap();
    let ResponseBody::Stream(mut stream) = response.body else {
        panic!("buffered gateway response should switch to streaming after the busy threshold");
    };
    assert_eq!(
        next_stream_data_amount(&mut stream, expected_first.len()).await,
        expected_first.as_bytes()
    );
    continue_tx.send(()).unwrap();
    assert_eq!(next_stream_data(&mut stream).await, b"second");
    assert!(stream.next().await.unwrap().is_none());
    backend_task.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn fastcgi_streams_stdout_before_end_request() {
    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            http {
                server {
                    listen 127.0.0.1:18125;
                    location / {
                        fastcgi_pass 127.0.0.1:9000;
                        fastcgi_buffering off;
                    }
                }
            }
        "#,
        &[],
    );
    let server = &config.http.as_ref().unwrap().servers[0];
    let context = &server.locations[0].context;
    let request = test_request("/");
    let env = test_env();
    let remote: SocketAddr = "127.0.0.1:45678".parse().unwrap();
    let local: SocketAddr = "127.0.0.1:18125".parse().unwrap();
    let variables = Variables::new(&request, "localhost", server, &local, &remote);
    let (client, mut backend) = tokio::io::duplex(64 * 1024);
    let (continue_tx, continue_rx) = tokio::sync::oneshot::channel();
    let backend_task = tokio::spawn(async move {
        read_fastcgi_test_request(&mut backend).await;
        write_fastcgi_record(
            &mut backend,
            FCGI_STDOUT,
            b"Status: 200 OK\r\nContent-Type: text/plain\r\n\r\nfirst",
        )
        .await
        .unwrap();
        backend.flush().await.unwrap();
        continue_rx.await.unwrap();
        write_fastcgi_record(&mut backend, FCGI_STDOUT, b"second")
            .await
            .unwrap();
        write_fastcgi_record(&mut backend, FCGI_END_REQUEST, &[0; 8])
            .await
            .unwrap();
        backend.flush().await.unwrap();
    });

    let response = fastcgi_exchange(
        client,
        &request,
        &env,
        Some(&server.locations[0]),
        context,
        &variables,
    )
    .await
    .unwrap();
    let ResponseBody::Stream(mut stream) = response.body else {
        panic!("FastCGI response should be streamed");
    };
    assert_eq!(next_stream_data(&mut stream).await, b"first");
    continue_tx.send(()).unwrap();
    assert_eq!(next_stream_data(&mut stream).await, b"second");
    assert!(stream.next().await.unwrap().is_none());
    backend_task.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn fastcgi_buffered_mode_streams_after_the_busy_threshold() {
    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            http {
                server {
                    listen 127.0.0.1:18128;
                    location / {
                        fastcgi_pass 127.0.0.1:9000;
                        fastcgi_buffers 2 4k;
                        fastcgi_busy_buffers_size 4k;
                    }
                }
            }
        "#,
        &[],
    );
    let server = &config.http.as_ref().unwrap().servers[0];
    let context = &server.locations[0].context;
    let request = test_request("/");
    let env = test_env();
    let remote: SocketAddr = "127.0.0.1:45678".parse().unwrap();
    let local: SocketAddr = "127.0.0.1:18128".parse().unwrap();
    let variables = Variables::new(&request, "localhost", server, &local, &remote);
    let (client, mut backend) = tokio::io::duplex(64 * 1024);
    let (continue_tx, continue_rx) = tokio::sync::oneshot::channel();
    let first = "a".repeat(8 * 1024);
    let expected_first = first.clone();
    let backend_task = tokio::spawn(async move {
        read_fastcgi_test_request(&mut backend).await;
        write_fastcgi_record(
            &mut backend,
            FCGI_STDOUT,
            format!("Status: 200 OK\r\nContent-Type: text/plain\r\n\r\n{first}").as_bytes(),
        )
        .await
        .unwrap();
        backend.flush().await.unwrap();
        continue_rx.await.unwrap();
        write_fastcgi_record(&mut backend, FCGI_STDOUT, b"second")
            .await
            .unwrap();
        write_fastcgi_record(&mut backend, FCGI_END_REQUEST, &[0; 8])
            .await
            .unwrap();
        backend.flush().await.unwrap();
    });

    let response = fastcgi_exchange(
        client,
        &request,
        &env,
        Some(&server.locations[0]),
        context,
        &variables,
    )
    .await
    .unwrap();
    let ResponseBody::Stream(mut stream) = response.body else {
        panic!("buffered FastCGI response should switch to streaming after the busy threshold");
    };
    assert_eq!(
        next_stream_data_amount(&mut stream, expected_first.len()).await,
        expected_first.as_bytes()
    );
    continue_tx.send(()).unwrap();
    assert_eq!(next_stream_data(&mut stream).await, b"second");
    assert!(stream.next().await.unwrap().is_none());
    backend_task.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn fastcgi_stream_terminates_on_caught_stderr() {
    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            http {
                server {
                    listen 127.0.0.1:18126;
                    location / {
                        fastcgi_pass 127.0.0.1:9000;
                        fastcgi_buffering off;
                        fastcgi_catch_stderr "PHP Fatal error";
                    }
                }
            }
        "#,
        &[],
    );
    let server = &config.http.as_ref().unwrap().servers[0];
    let context = &server.locations[0].context;
    let request = test_request("/");
    let env = test_env();
    let remote: SocketAddr = "127.0.0.1:45678".parse().unwrap();
    let local: SocketAddr = "127.0.0.1:18126".parse().unwrap();
    let variables = Variables::new(&request, "localhost", server, &local, &remote);
    let (client, mut backend) = tokio::io::duplex(64 * 1024);
    let (continue_tx, continue_rx) = tokio::sync::oneshot::channel();
    let backend_task = tokio::spawn(async move {
        read_fastcgi_test_request(&mut backend).await;
        write_fastcgi_record(
            &mut backend,
            FCGI_STDOUT,
            b"Status: 200 OK\r\nContent-Type: text/plain\r\n\r\nfirst",
        )
        .await
        .unwrap();
        backend.flush().await.unwrap();
        continue_rx.await.unwrap();
        write_fastcgi_record(&mut backend, FCGI_STDERR, b"PHP Fatal error: failed")
            .await
            .unwrap();
        backend.flush().await.unwrap();
    });

    let response = fastcgi_exchange(
        client,
        &request,
        &env,
        Some(&server.locations[0]),
        context,
        &variables,
    )
    .await
    .unwrap();
    let ResponseBody::Stream(mut stream) = response.body else {
        panic!("FastCGI response should be streamed");
    };
    assert_eq!(next_stream_data(&mut stream).await, b"first");
    continue_tx.send(()).unwrap();
    assert!(stream.next().await.is_err());
    backend_task.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

fn test_request(uri: &str) -> Request {
    Request {
        method: "GET".into(),
        target: uri.into(),
        version: "HTTP/1.1".into(),
        headers: vec![Header {
            name: "Host".into(),
            value: "localhost".into(),
        }],
        body_stream: None,
        body: Vec::new(),
        remote_addr: "127.0.0.1:45678".into(),
        local_addr: "127.0.0.1:18124".into(),
        keep_alive: false,
    }
}

fn test_env() -> RequestEnv {
    RequestEnv {
        uri: "/".into(),
        args: None,
        unparsed_uri: Some("/".into()),
        internal: false,
        bypass_try_files: false,
        limits_checked: false,
        auth_request_checked: false,
        mirror_checked: false,
    }
}

async fn next_stream_data(stream: &mut ResponseStream) -> Vec<u8> {
    match tokio::time::timeout(Duration::from_secs(1), stream.next())
        .await
        .expect("streamed body timed out")
        .unwrap()
    {
        Some(ResponseStreamItem::Data(data)) => data.to_vec(),
        Some(ResponseStreamItem::Trailers(_)) => panic!("unexpected gateway trailers"),
        None => panic!("gateway stream ended before data"),
    }
}

async fn next_stream_data_amount(stream: &mut ResponseStream, amount: usize) -> Vec<u8> {
    let mut body = Vec::with_capacity(amount);
    while body.len() < amount {
        let data = next_stream_data(stream).await;
        assert!(
            data.len() <= 4 * 1024,
            "a DATA item exceeded the configured busy window"
        );
        body.extend_from_slice(&data);
    }
    body
}

async fn read_test_uwsgi_packet<R: AsyncRead + Unpin>(reader: &mut R) {
    let mut header = [0_u8; 4];
    reader.read_exact(&mut header).await.unwrap();
    let length = u16::from_le_bytes([header[1], header[2]]) as usize;
    let mut parameters = vec![0_u8; length];
    reader.read_exact(&mut parameters).await.unwrap();
}

async fn read_test_scgi_packet<R: AsyncRead + Unpin>(reader: &mut R) {
    let mut length = Vec::new();
    let mut byte = [0_u8; 1];
    loop {
        reader.read_exact(&mut byte).await.unwrap();
        if byte[0] == b':' {
            break;
        }
        length.push(byte[0]);
    }
    let length = std::str::from_utf8(&length).unwrap().parse().unwrap();
    let mut parameters = vec![0_u8; length];
    reader.read_exact(&mut parameters).await.unwrap();
}

async fn read_fastcgi_test_request<R: AsyncRead + Unpin>(reader: &mut R) {
    loop {
        let record = read_fastcgi_record(reader).await.unwrap();
        if record.record_type == FCGI_STDIN && record.content.is_empty() {
            break;
        }
    }
}
