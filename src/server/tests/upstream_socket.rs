use super::support::*;
use super::*;

#[tokio::test]
async fn applies_proxy_bind_and_socket_options_to_http_upstreams() {
    let backend = match TcpListener::bind("127.0.0.1:0").await {
        Ok(backend) => backend,
        Err(err) if err.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!("skipping HTTP proxy socket test: sockets not permitted");
            return;
        }
        Err(err) => panic!("bind HTTP backend: {err}"),
    };
    let backend_address = backend.local_addr().unwrap();
    let backend_task = tokio::spawn(async move {
        let (mut stream, peer) = backend.accept().await.unwrap();
        assert_eq!(peer.ip(), "127.0.0.2".parse::<IpAddr>().unwrap());

        let mut reader = BufReader::new(&mut stream);
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).await.unwrap();
            if line == "\r\n" || line.is_empty() {
                break;
            }
        }
        reader
            .get_mut()
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok")
            .await
            .unwrap();
    });
    let source = format!(
        r#"
            worker_processes 1;
            events {{ worker_connections 64; }}
            http {{
                server {{
                    listen 127.0.0.1:18135;
                    location / {{
                        proxy_pass http://{backend_address};
                        proxy_bind 127.0.0.2;
                        proxy_socket_keepalive on;
                        proxy_socket_rcvbuf 32k;
                        proxy_socket_sndbuf 64k;
                    }}
                }}
            }}
        "#
    );
    let (prefix, config) = test_config(&source, &[]);
    let response = request_body(
        &config,
        "GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(
        response.starts_with("HTTP/1.1 200 OK\r\n") && response.ends_with("ok"),
        "unexpected proxy response:\n{response}"
    );
    backend_task.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn applies_fastcgi_bind_and_socket_options_to_upstreams() {
    let backend = match TcpListener::bind("127.0.0.1:0").await {
        Ok(backend) => backend,
        Err(err) if err.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!("skipping FastCGI socket test: sockets not permitted");
            return;
        }
        Err(err) => panic!("bind FastCGI backend: {err}"),
    };
    let backend_address = backend.local_addr().unwrap();
    let backend_task = tokio::spawn(async move {
        let (mut stream, peer) = backend.accept().await.unwrap();
        assert_eq!(peer.ip(), "127.0.0.2".parse::<IpAddr>().unwrap());
        loop {
            let (record_type, content) = read_fastcgi_test_record(&mut stream).await;
            if record_type == FCGI_STDIN && content.is_empty() {
                break;
            }
        }
        write_fastcgi_record(
            &mut stream,
            FCGI_STDOUT,
            b"Status: 200 OK\r\nContent-Type: text/plain\r\n\r\nfastcgi-socket-ok",
        )
        .await
        .unwrap();
        write_fastcgi_record(&mut stream, FCGI_END_REQUEST, &[0; 8])
            .await
            .unwrap();
        stream.flush().await.unwrap();
    });
    let source = format!(
        r#"
            worker_processes 1;
            events {{ worker_connections 64; }}
            http {{
                server {{
                    listen 127.0.0.1:18141;
                    location / {{
                        fastcgi_pass {backend_address};
                        fastcgi_bind 127.0.0.2;
                        fastcgi_socket_keepalive on;
                        fastcgi_socket_rcvbuf 32k;
                        fastcgi_socket_sndbuf 64k;
                    }}
                }}
            }}
        "#
    );
    let (prefix, config) = test_config(&source, &[]);
    let response = request_body(
        &config,
        "GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(
        response.starts_with("HTTP/1.1 200 OK\r\n") && response.ends_with("fastcgi-socket-ok"),
        "unexpected FastCGI response:\n{response}"
    );
    backend_task.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn applies_uwsgi_and_scgi_bind_and_socket_options_to_upstreams() {
    let uwsgi = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let scgi = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let uwsgi_address = uwsgi.local_addr().unwrap();
    let scgi_address = scgi.local_addr().unwrap();
    let uwsgi_task = tokio::spawn(async move {
        let (mut stream, peer) = uwsgi.accept().await.unwrap();
        assert_eq!(peer.ip(), "127.0.0.2".parse::<IpAddr>().unwrap());
        let mut header = [0_u8; 4];
        stream.read_exact(&mut header).await.unwrap();
        let parameter_length = u16::from_le_bytes([header[1], header[2]]) as usize;
        let mut parameters = vec![0_u8; parameter_length];
        stream.read_exact(&mut parameters).await.unwrap();
        stream
            .write_all(b"Status: 200 OK\r\nContent-Type: text/plain\r\n\r\nuwsgi-bind-ok")
            .await
            .unwrap();
        stream.shutdown().await.unwrap();
    });
    let scgi_task = tokio::spawn(async move {
        let (mut stream, peer) = scgi.accept().await.unwrap();
        assert_eq!(peer.ip(), "127.0.0.2".parse::<IpAddr>().unwrap());
        let mut length = String::new();
        loop {
            let byte = stream.read_u8().await.unwrap();
            if byte == b':' {
                break;
            }
            length.push(byte as char);
        }
        let mut netstring = vec![0_u8; length.parse::<usize>().unwrap()];
        stream.read_exact(&mut netstring).await.unwrap();
        stream
            .write_all(b"Status: 200 OK\r\nContent-Type: text/plain\r\n\r\nscgi-bind-ok")
            .await
            .unwrap();
        stream.shutdown().await.unwrap();
    });
    let source = format!(
        r#"
            worker_processes 1;
            events {{ worker_connections 64; }}
            http {{
                uwsgi_bind 127.0.0.2;
                uwsgi_socket_keepalive on;
                uwsgi_socket_rcvbuf 32k;
                uwsgi_socket_sndbuf 64k;
                scgi_bind 127.0.0.2;
                scgi_socket_keepalive on;
                scgi_socket_rcvbuf 16k;
                scgi_socket_sndbuf 128k;
                server {{
                    listen 127.0.0.1:18143;
                    location /u {{
                        uwsgi_pass {uwsgi_address};
                    }}
                    location /s {{
                        scgi_pass {scgi_address};
                    }}
                }}
            }}
        "#
    );
    let (prefix, config) = test_config(&source, &[]);
    let server = &config.http.as_ref().unwrap().servers[0];
    let request = Request {
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
        local_addr: "127.0.0.1:18143".into(),
        keep_alive: false,
    };
    let remote: SocketAddr = "127.0.0.1:45678".parse().unwrap();
    let local: SocketAddr = "127.0.0.1:18143".parse().unwrap();
    let variables = Variables::new(&request, "localhost", server, &local, &remote);

    let response = uwsgi_attempt(
        &request,
        &server.locations[0].context,
        &variables,
        ("127.0.0.1", uwsgi_address.port()),
        false,
    )
    .await
    .unwrap();
    assert_eq!(response.status, 200);
    assert_eq!(response.into_bytes(), b"uwsgi-bind-ok");
    uwsgi_task.await.unwrap();

    let response = scgi_attempt(
        &request,
        &server.locations[1].context,
        &variables,
        ("127.0.0.1", scgi_address.port()),
    )
    .await
    .unwrap();
    assert_eq!(response.status, 200);
    assert_eq!(response.into_bytes(), b"scgi-bind-ok");
    scgi_task.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}
