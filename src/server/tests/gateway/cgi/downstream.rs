use super::*;

#[tokio::test]
async fn fastcgi_downstream_write_failure_cancels_the_streaming_producer() {
    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            http {
                server {
                    listen 127.0.0.1:18129;
                    location / {
                        fastcgi_pass 127.0.0.1:9000;
                        fastcgi_buffering off;
                        fastcgi_read_timeout 30s;
                    }
                }
            }
        "#,
        &[],
    );
    let server = &config.http.as_ref().unwrap().servers[0];
    let context = &server.locations[0].context;
    let request = request();
    let env = env();
    let remote: SocketAddr = "127.0.0.1:45678".parse().unwrap();
    let local: SocketAddr = "127.0.0.1:18129".parse().unwrap();
    let variables = Variables::new(&request, "localhost", server, &local, &remote);
    let (client, mut backend) = tokio::io::duplex(64 * 1024);
    let (release, wait_for_release) = tokio::sync::oneshot::channel();
    let backend_task = tokio::spawn(async move {
        read_fastcgi_request(&mut backend).await;
        write_fastcgi_record(
            &mut backend,
            FCGI_STDOUT,
            b"Status: 200 OK\r\nContent-Type: text/plain\r\n\r\nfirst",
        )
        .await
        .unwrap();
        backend.flush().await.unwrap();
        wait_for_release.await.unwrap();

        let _ = write_fastcgi_record(&mut backend, FCGI_STDOUT, b"second").await;
        let mut buffer = [0_u8; 1];
        let closed = tokio::time::timeout(Duration::from_secs(2), backend.read(&mut buffer))
            .await
            .expect("downstream write failure should close the FastCGI backend");
        assert!(
            matches!(closed, Ok(0) | Err(_)),
            "unexpected FastCGI backend read: {closed:?}"
        );
    });

    let mut response = fastcgi_exchange(
        client,
        &request,
        &env,
        Some(&server.locations[0]),
        context,
        &variables,
    )
    .await
    .unwrap();
    let (mut downstream_client, mut downstream_server) = tokio::io::duplex(64 * 1024);
    let writer = tokio::spawn(async move {
        write_response(
            &mut downstream_server,
            &mut response,
            ResponseWriteOptions {
                request_method: "GET",
                request_version: "HTTP/1.1",
                keep_alive: false,
                keep_alive_header: None,
                server_tokens: ServerTokens::On,
                chunked_transfer_encoding: true,
                rate: ResponseRate::default(),
            },
        )
        .await
    });

    let mut output = Vec::new();
    tokio::time::timeout(Duration::from_secs(1), async {
        while !output.windows(5).any(|window| window == b"first") {
            let mut buffer = [0_u8; 1024];
            let read = downstream_client.read(&mut buffer).await.unwrap();
            assert!(read > 0, "downstream closed before the first chunk");
            output.extend_from_slice(&buffer[..read]);
        }
    })
    .await
    .expect("FastCGI response should stream before downstream disconnect");

    drop(downstream_client);
    release.send(()).unwrap();
    let error = writer.await.unwrap().unwrap_err();
    assert!(
        matches!(
            error.kind(),
            io::ErrorKind::BrokenPipe | io::ErrorKind::ConnectionReset
        ),
        "unexpected downstream write error: {error}"
    );
    backend_task.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

fn request() -> Request {
    Request {
        method: "GET".into(),
        target: "/".into(),
        version: "HTTP/1.1".into(),
        headers: vec![Header {
            name: "Host".into(),
            value: "localhost".into(),
        }],
        body_stream: None,
        body: Vec::new(),
        remote_addr: "127.0.0.1:45678".into(),
        local_addr: "127.0.0.1:18129".into(),
        keep_alive: false,
    }
}

fn env() -> RequestEnv {
    RequestEnv {
        uri: "/".into(),
        args: None,
        unparsed_uri: Some("/".into()),
        valid_location: true,
        internal: false,
        bypass_try_files: false,
        add_uri_to_alias: false,
        limits_checked: false,
        auth_request_checked: false,
        mirror_checked: false,
    }
}

async fn read_fastcgi_request<R: AsyncRead + Unpin>(reader: &mut R) {
    loop {
        let record = read_fastcgi_record(reader).await.unwrap();
        if record.record_type == FCGI_STDIN && record.content.is_empty() {
            break;
        }
    }
}
