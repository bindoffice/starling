use super::*;

#[tokio::test]
async fn fastcgi_forwards_request_body_before_the_client_finishes() {
    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            http {
                server {
                    listen 127.0.0.1:18130;
                    location / {
                        fastcgi_pass 127.0.0.1:9000;
                        fastcgi_request_buffering off;
                        fastcgi_buffering off;
                    }
                }
            }
        "#,
        &[],
    );
    let server = &config.http.as_ref().unwrap().servers[0];
    let context = server.locations[0].context.clone();
    let (sender, request) = streaming_request("/", "127.0.0.1:18130", 8);
    let env = test_env();
    let variables = Variables::new(&request, "localhost", server, &remote(), &local(18130));
    let (client, mut backend) = tokio::io::duplex(64 * 1024);
    let (first_seen_tx, first_seen_rx) = tokio::sync::oneshot::channel();
    let backend_task = tokio::spawn(async move {
        let first = next_fastcgi_stdin(&mut backend).await;
        assert_eq!(first, b"part");
        first_seen_tx.send(()).unwrap();
        while !next_fastcgi_stdin(&mut backend).await.is_empty() {}
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
    first_seen_rx.await.unwrap();
    sender.send(Bytes::from_static(b"more")).await;
    drop(sender);

    let response = exchange.await.unwrap().unwrap();
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
async fn uwsgi_forwards_request_body_before_the_client_finishes() {
    raw_gateway_forwards_request_body(
        RawGateway::Uwsgi,
        r#"
            location / {
                uwsgi_pass 127.0.0.1:9001;
                uwsgi_request_buffering off;
                uwsgi_buffering off;
            }
        "#,
        18131,
    )
    .await;
}

#[tokio::test]
async fn scgi_forwards_request_body_before_the_client_finishes() {
    raw_gateway_forwards_request_body(
        RawGateway::Scgi,
        r#"
            location / {
                scgi_pass 127.0.0.1:9002;
                scgi_request_buffering off;
                scgi_buffering off;
            }
        "#,
        18132,
    )
    .await;
}

#[tokio::test]
async fn uwsgi_chunked_body_is_buffered_before_forwarding() {
    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            http {
                server {
                    listen 127.0.0.1:18156;
                    location / {
                        uwsgi_pass 127.0.0.1:9001;
                        uwsgi_request_buffering off;
                    }
                }
            }
        "#,
        &[],
    );
    let server = &config.http.as_ref().unwrap().servers[0];
    let context = server.locations[0].context.clone();
    let (sender, mut request) = streaming_chunked_request("/", "127.0.0.1:18156");
    sender.send(Bytes::from_static(b"part")).await;
    sender.send(Bytes::from_static(b"more")).await;
    drop(sender);

    let mut variables = Variables::new(&request, "localhost", server, &remote(), &local(18156));
    prepare_request_body(&mut request, &context, &mut variables)
        .await
        .unwrap();
    assert_eq!(request.body, b"partmore");
    assert!(request.body_stream.is_none());

    let (client, mut backend) = tokio::io::duplex(64 * 1024);
    let backend_task = tokio::spawn(async move {
        read_uwsgi_header(&mut backend).await;
        let mut body = [0_u8; 8];
        backend.read_exact(&mut body).await.unwrap();
        assert_eq!(&body, b"partmore");
        backend
            .write_all(b"Status: 200 OK\r\nContent-Type: text/plain\r\n\r\nok")
            .await
            .unwrap();
        backend.shutdown().await.unwrap();
    });
    let response = uwsgi_exchange(client, &request, &context, &variables)
        .await
        .unwrap();
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
async fn fastcgi_replays_buffered_file_body_from_the_start() {
    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            http {
                client_body_buffer_size 4;
                server {
                    listen 127.0.0.1:18157;
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
    let context = server.locations[0].context.clone();
    let (sender, mut request) = streaming_chunked_request("/", "127.0.0.1:18157");
    sender.send(Bytes::from_static(b"part")).await;
    sender.send(Bytes::from_static(b"more")).await;
    drop(sender);

    let mut variables = Variables::new(&request, "localhost", server, &remote(), &local(18157));
    prepare_request_body(&mut request, &context, &mut variables)
        .await
        .unwrap();
    assert!(request.body.is_empty());
    let file = request.body_file().expect("buffered request body file");
    assert_eq!(file.len(), 8);
    assert!(request.body_stream.as_ref().unwrap().is_file_backed());

    let (client, mut backend) = tokio::io::duplex(64 * 1024);
    let backend_task = tokio::spawn(async move {
        let body = next_fastcgi_stdin(&mut backend).await;
        assert_eq!(body, b"partmore");
        while !next_fastcgi_stdin(&mut backend).await.is_empty() {}
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
    let response =
        match fastcgi_exchange(client, &request, &test_env(), None, &context, &variables).await {
            Ok(response) => response,
            Err(error) => {
                let backend_error = backend_task.await;
                panic!("FastCGI file body exchange failed: {error:?}; backend: {backend_error:?}");
            }
        };
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
async fn raw_gateways_replay_buffered_file_body_with_content_length() {
    raw_gateway_replays_buffered_file_body(
        RawGateway::Uwsgi,
        r#"
            location / {
                uwsgi_pass 127.0.0.1:9001;
                uwsgi_buffering off;
            }
        "#,
        18158,
    )
    .await;
    raw_gateway_replays_buffered_file_body(
        RawGateway::Scgi,
        r#"
            location / {
                scgi_pass 127.0.0.1:9002;
                scgi_buffering off;
            }
        "#,
        18159,
    )
    .await;
}

#[tokio::test]
async fn fastcgi_maps_streaming_request_body_overflow_to_413() {
    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            http {
                client_max_body_size 5;
                server {
                    listen 127.0.0.1:18133;
                    location / {
                        fastcgi_pass 127.0.0.1:9000;
                        fastcgi_request_buffering off;
                    }
                }
            }
        "#,
        &[],
    );
    let server = &config.http.as_ref().unwrap().servers[0];
    let context = server.locations[0].context.clone();
    let (sender, request) = streaming_request("/", "127.0.0.1:18133", 8);
    let env = test_env();
    let variables = Variables::new(&request, "localhost", server, &remote(), &local(18133));
    let (client, mut backend) = tokio::io::duplex(64 * 1024);
    let backend_task = tokio::spawn(async move {
        let mut discarded = Vec::new();
        let _ = backend.read_to_end(&mut discarded).await;
    });
    let exchange = tokio::spawn(async move {
        fastcgi_exchange(client, &request, &env, None, &context, &variables).await
    });

    sender.send(Bytes::from_static(b"part")).await;
    sender.send(Bytes::from_static(b"more")).await;
    drop(sender);
    let failure = match exchange.await.unwrap() {
        Ok(_) => panic!("streaming FastCGI request body should exceed client_max_body_size"),
        Err(failure) => failure,
    };
    assert!(matches!(failure, ProxyFailure::RequestBody(413)));
    backend_task.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

#[derive(Clone, Copy)]
enum RawGateway {
    Uwsgi,
    Scgi,
}

async fn raw_gateway_forwards_request_body(gateway: RawGateway, location: &str, port: u16) {
    let source = format!(
        r#"
            worker_processes 1;
            events {{ worker_connections 64; }}
            http {{
                server {{
                    listen 127.0.0.1:{port};
                    {location}
                }}
            }}
        "#
    );
    let (prefix, config) = test_config(&source, &[]);
    let server = &config.http.as_ref().unwrap().servers[0];
    let context = server.locations[0].context.clone();
    let (sender, request) = streaming_request("/", &format!("127.0.0.1:{port}"), 8);
    let variables = Variables::new(&request, "localhost", server, &remote(), &local(port));
    let (client, mut backend) = tokio::io::duplex(64 * 1024);
    let (first_seen_tx, first_seen_rx) = tokio::sync::oneshot::channel();
    let backend_task = tokio::spawn(async move {
        match gateway {
            RawGateway::Uwsgi => read_uwsgi_header(&mut backend).await,
            RawGateway::Scgi => read_scgi_header(&mut backend).await,
        }
        let mut first = [0_u8; 4];
        backend.read_exact(&mut first).await.unwrap();
        assert_eq!(&first, b"part");
        first_seen_tx.send(()).unwrap();
        let mut rest = [0_u8; 4];
        backend.read_exact(&mut rest).await.unwrap();
        assert_eq!(&rest, b"more");
        backend
            .write_all(b"Status: 200 OK\r\nContent-Type: text/plain\r\n\r\nok")
            .await
            .unwrap();
        backend.shutdown().await.unwrap();
    });
    let exchange_context = context.clone();
    let exchange_variables = variables.clone();
    let exchange = tokio::spawn(async move {
        match gateway {
            RawGateway::Uwsgi => {
                uwsgi_exchange(client, &request, &exchange_context, &exchange_variables).await
            }
            RawGateway::Scgi => {
                scgi_exchange(client, &request, &exchange_context, &exchange_variables).await
            }
        }
    });

    sender.send(Bytes::from_static(b"part")).await;
    first_seen_rx.await.unwrap();
    sender.send(Bytes::from_static(b"more")).await;
    drop(sender);

    let response = exchange.await.unwrap().unwrap();
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

async fn raw_gateway_replays_buffered_file_body(gateway: RawGateway, location: &str, port: u16) {
    let source = format!(
        r#"
            worker_processes 1;
            events {{ worker_connections 64; }}
            http {{
                client_body_buffer_size 4;
                server {{
                    listen 127.0.0.1:{port};
                    {location}
                }}
            }}
        "#
    );
    let (prefix, config) = test_config(&source, &[]);
    let server = &config.http.as_ref().unwrap().servers[0];
    let context = server.locations[0].context.clone();
    let (sender, mut request) = streaming_chunked_request("/", &format!("127.0.0.1:{port}"));
    sender.send(Bytes::from_static(b"part")).await;
    sender.send(Bytes::from_static(b"more")).await;
    drop(sender);

    let mut variables = Variables::new(&request, "localhost", server, &remote(), &local(port));
    prepare_request_body(&mut request, &context, &mut variables)
        .await
        .unwrap();
    let file = request.body_file().expect("buffered request body file");
    let body_path = file.path().to_path_buf();
    assert_eq!(file.len(), 8);

    let (client, mut backend) = tokio::io::duplex(64 * 1024);
    let backend_task = tokio::spawn(async move {
        let parameters = match gateway {
            RawGateway::Uwsgi => read_uwsgi_parameters(&mut backend).await,
            RawGateway::Scgi => read_scgi_parameters(&mut backend).await,
        };
        assert_eq!(
            parameters.get("CONTENT_LENGTH").map(String::as_str),
            Some("8")
        );
        let mut body = [0_u8; 8];
        backend.read_exact(&mut body).await.unwrap();
        assert_eq!(&body, b"partmore");
        backend
            .write_all(b"Status: 200 OK\r\nContent-Type: text/plain\r\n\r\nok")
            .await
            .unwrap();
        backend.shutdown().await.unwrap();
    });
    let response = match gateway {
        RawGateway::Uwsgi => uwsgi_exchange(client, &request, &context, &variables).await,
        RawGateway::Scgi => scgi_exchange(client, &request, &context, &variables).await,
    }
    .unwrap();
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

    drop(file);
    drop(variables);
    drop(request);
    assert!(
        !body_path.exists(),
        "buffered request body should be removed"
    );
    let _ = std::fs::remove_dir_all(prefix);
}

async fn read_uwsgi_parameters<R: AsyncRead + Unpin>(reader: &mut R) -> HashMap<String, String> {
    let mut header = [0_u8; 4];
    reader.read_exact(&mut header).await.unwrap();
    let length = u16::from_le_bytes([header[1], header[2]]) as usize;
    let mut parameters = vec![0_u8; length];
    reader.read_exact(&mut parameters).await.unwrap();
    parse_null_separated_params(&parameters)
}

async fn read_scgi_parameters<R: AsyncRead + Unpin>(reader: &mut R) -> HashMap<String, String> {
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
    assert_eq!(parameters.last(), Some(&b','));
    parse_null_separated_params(&parameters[..parameters.len() - 1])
}
