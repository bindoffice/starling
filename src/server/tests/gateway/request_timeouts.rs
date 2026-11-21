use super::*;

#[tokio::test]
async fn fastcgi_read_timeout_does_not_cover_a_slow_client_body() {
    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            http {
                server {
                    listen 127.0.0.1:18134;
                    location / {
                        fastcgi_pass 127.0.0.1:9000;
                        fastcgi_request_buffering off;
                        fastcgi_buffering off;
                        fastcgi_send_timeout 20ms;
                        fastcgi_read_timeout 20ms;
                    }
                }
            }
        "#,
        &[],
    );
    let server = &config.http.as_ref().unwrap().servers[0];
    let context = server.locations[0].context.clone();
    let (sender, request) = streaming_request("/", "127.0.0.1:18134", 8);
    let env = test_env();
    let variables = Variables::new(&request, "localhost", server, &remote(), &local(18134));
    let (client, mut backend) = tokio::io::duplex(64 * 1024);
    let backend_task = tokio::spawn(async move {
        let mut body = Vec::new();
        loop {
            let chunk = next_fastcgi_stdin(&mut backend).await;
            if chunk.is_empty() {
                break;
            }
            body.extend_from_slice(&chunk);
        }
        assert_eq!(body, b"partmore");
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

    tokio::time::sleep(Duration::from_millis(50)).await;
    sender.send(Bytes::from_static(b"part")).await;
    sender.send(Bytes::from_static(b"more")).await;
    drop(sender);

    let response = exchange.await.unwrap().unwrap();
    assert_eq!(response.status, 200);
    backend_task.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn fastcgi_read_timeout_still_applies_while_waiting_for_upstream() {
    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            http {
                server {
                    listen 127.0.0.1:18135;
                    location / {
                        fastcgi_pass 127.0.0.1:9000;
                        fastcgi_request_buffering off;
                        fastcgi_buffering off;
                        fastcgi_read_timeout 20ms;
                    }
                }
            }
        "#,
        &[],
    );
    let server = &config.http.as_ref().unwrap().servers[0];
    let context = server.locations[0].context.clone();
    let (sender, request) = streaming_request("/", "127.0.0.1:18135", 4);
    let env = test_env();
    let variables = Variables::new(&request, "localhost", server, &remote(), &local(18135));
    let (client, mut backend) = tokio::io::duplex(64 * 1024);
    let backend_task = tokio::spawn(async move {
        assert_eq!(next_fastcgi_stdin(&mut backend).await, b"part");
        tokio::time::sleep(Duration::from_millis(60)).await;
    });
    let exchange = tokio::spawn(async move {
        fastcgi_exchange(client, &request, &env, None, &context, &variables).await
    });

    sender.send(Bytes::from_static(b"part")).await;
    drop(sender);

    let failure = match exchange.await.unwrap() {
        Ok(_) => panic!("FastCGI response should time out"),
        Err(failure) => failure,
    };
    assert!(matches!(failure, ProxyFailure::Timeout));
    backend_task.abort();
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn fastcgi_read_timeout_tracks_inactivity_within_a_record() {
    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            http {
                server {
                    listen 127.0.0.1:18138;
                    location / {
                        fastcgi_pass 127.0.0.1:9000;
                        fastcgi_buffering off;
                        fastcgi_read_timeout 20ms;
                    }
                }
            }
        "#,
        &[],
    );
    let server = &config.http.as_ref().unwrap().servers[0];
    let context = server.locations[0].context.clone();
    let request = plain_request("/", "127.0.0.1:18138");
    let variables = Variables::new(&request, "localhost", server, &remote(), &local(18138));
    let (client, mut backend) = tokio::io::duplex(64 * 1024);
    let backend_task = tokio::spawn(async move {
        loop {
            let record = read_fastcgi_record(&mut backend).await.unwrap();
            if record.record_type == FCGI_STDIN && record.content.is_empty() {
                break;
            }
        }

        let content = b"Status: 200 OK\r\nContent-Type: text/plain\r\n\r\nok";
        let mut header = vec![
            1,
            FCGI_STDOUT,
            0,
            1,
            (content.len() >> 8) as u8,
            content.len() as u8,
            0,
            0,
        ];
        header.extend_from_slice(content);
        for byte in header {
            backend.write_all(&[byte]).await.unwrap();
            backend.flush().await.unwrap();
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        write_fastcgi_record(&mut backend, FCGI_END_REQUEST, &[0; 8])
            .await
            .unwrap();
        backend.flush().await.unwrap();
    });

    let result = tokio::time::timeout(
        Duration::from_secs(1),
        fastcgi_exchange(client, &request, &test_env(), None, &context, &variables),
    )
    .await
    .expect("FastCGI trickle should not hang");
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
async fn fastcgi_read_timeout_expires_between_stdout_records() {
    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            http {
                server {
                    listen 127.0.0.1:18162;
                    location / {
                        fastcgi_pass 127.0.0.1:9000;
                        fastcgi_buffering off;
                        fastcgi_read_timeout 20ms;
                    }
                }
            }
        "#,
        &[],
    );
    let server = &config.http.as_ref().unwrap().servers[0];
    let context = server.locations[0].context.clone();
    let request = plain_request("/", "127.0.0.1:18162");
    let variables = Variables::new(&request, "localhost", server, &remote(), &local(18162));
    let (client, mut backend) = tokio::io::duplex(64 * 1024);
    let backend_task = tokio::spawn(async move {
        loop {
            let record = read_fastcgi_record(&mut backend).await.unwrap();
            if record.record_type == FCGI_STDIN && record.content.is_empty() {
                break;
            }
        }
        write_fastcgi_record(
            &mut backend,
            FCGI_STDOUT,
            b"Status: 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 2\r\n\r\n",
        )
        .await
        .unwrap();
        backend.flush().await.unwrap();
        tokio::time::sleep(Duration::from_millis(250)).await;
    });

    let response = tokio::time::timeout(
        Duration::from_secs(1),
        fastcgi_exchange(client, &request, &test_env(), None, &context, &variables),
    )
    .await
    .expect("FastCGI response head should arrive before the record gap")
    .unwrap();
    assert_eq!(response.status, 200);
    let ResponseBody::Stream(mut stream) = response.body else {
        panic!("FastCGI response body should be streamed");
    };
    let failure = tokio::time::timeout(Duration::from_secs(1), stream.next())
        .await
        .expect("FastCGI record gap timeout should not hang")
        .expect_err("a gap longer than read timeout should fail");
    assert_eq!(failure.kind(), io::ErrorKind::TimedOut);
    backend_task.abort();
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn fastcgi_send_timeout_applies_to_a_stalled_upstream() {
    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            http {
                server {
                    listen 127.0.0.1:18136;
                    location / {
                        fastcgi_pass 127.0.0.1:9000;
                        fastcgi_send_timeout 20ms;
                        fastcgi_read_timeout 1s;
                    }
                }
            }
        "#,
        &[],
    );
    let server = &config.http.as_ref().unwrap().servers[0];
    let context = server.locations[0].context.clone();
    let request = plain_request("/", "127.0.0.1:18136");
    let variables = Variables::new(&request, "localhost", server, &remote(), &local(18136));
    let (client, backend) = tokio::io::duplex(1);
    let backend_task = tokio::spawn(async move {
        let _backend = backend;
        tokio::time::sleep(Duration::from_secs(1)).await;
    });

    let result = tokio::time::timeout(
        Duration::from_secs(1),
        fastcgi_exchange(client, &request, &test_env(), None, &context, &variables),
    )
    .await
    .expect("FastCGI send timeout should not hang");
    assert!(matches!(result, Err(ProxyFailure::Timeout)));
    backend_task.abort();
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn scgi_chunked_body_without_content_length_is_not_streamed() {
    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            http {
                server {
                    listen 127.0.0.1:18137;
                    location / {
                        scgi_pass 127.0.0.1:9002;
                        scgi_request_buffering off;
                    }
                }
            }
        "#,
        &[],
    );
    let server = &config.http.as_ref().unwrap().servers[0];
    let context = server.locations[0].context.clone();
    let (sender, mut request) = streaming_chunked_request("/", "127.0.0.1:18137");
    assert!(!request_body_stream_enabled(&request, &context));
    sender.send(Bytes::from_static(b"part")).await;
    sender.send(Bytes::from_static(b"more")).await;
    drop(sender);

    let mut variables = Variables::new(&request, "localhost", server, &remote(), &local(18137));
    prepare_request_body(&mut request, &context, &mut variables)
        .await
        .unwrap();
    assert_eq!(request.body, b"partmore");
    let packet = build_scgi_header(&request, &context, &variables, false).unwrap();
    assert!(
        packet
            .windows(b"CONTENT_LENGTH\0".len())
            .any(|window| window == b"CONTENT_LENGTH\0")
    );
    assert!(packet.windows(2).any(|window| window == b"8\0"));
    let _ = std::fs::remove_dir_all(prefix);
}
