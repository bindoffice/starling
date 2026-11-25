use super::support::*;
use super::*;

#[test]
fn packs_binary_remote_addr_for_limit_keys() {
    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            http {
                server {
                    listen 127.0.0.1:18575;
                    location / { return 204; }
                }
            }
        "#,
        &[],
    );
    let server = &config.http.as_ref().unwrap().servers[0];
    let request = Request {
        method: "GET".into(),
        target: "/".into(),
        version: "HTTP/1.1".into(),
        headers: (vec![Header {
            name: "Host".into(),
            value: "localhost".into(),
        }])
        .into(),
        body_stream: None,
        body: Vec::new(),
        remote_addr: "127.0.0.1:45678".into(),
        local_addr: "127.0.0.1:18575".into(),
        keep_alive: false,
    };
    let remote: SocketAddr = "127.0.0.1:45678".parse().unwrap();
    let local: SocketAddr = "127.0.0.1:18575".parse().unwrap();
    let mut variables = Variables::new(&request, "localhost", server, &local, &remote);

    assert_eq!(
        eval_variables_bytes("$binary_remote_addr", &variables),
        [127, 0, 0, 1]
    );
    variables.remote_addr = "2001:db8::1".into();
    let mut expected = b"prefix:".to_vec();
    expected.extend_from_slice(&[0x20, 0x01, 0x0d, 0xb8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1]);
    assert_eq!(
        eval_variables_bytes("prefix:$binary_remote_addr", &variables),
        expected
    );
    let _ = std::fs::remove_dir_all(prefix);
}

#[test]
fn shares_request_headers_until_a_request_copy_is_mutated() {
    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            http {
                server {
                    listen 127.0.0.1:18574;
                    location / { return 204; }
                }
            }
        "#,
        &[],
    );
    let server = &config.http.as_ref().unwrap().servers[0];
    let request = Request {
        method: "GET".into(),
        target: "/".into(),
        version: "HTTP/1.1".into(),
        headers: (vec![Header {
            name: "Host".into(),
            value: "localhost".into(),
        }])
        .into(),
        body_stream: None,
        body: Vec::new(),
        remote_addr: "127.0.0.1:45678".into(),
        local_addr: "127.0.0.1:18574".into(),
        keep_alive: false,
    };
    let remote: SocketAddr = "127.0.0.1:45678".parse().unwrap();
    let local: SocketAddr = "127.0.0.1:18574".parse().unwrap();
    let variables = Variables::new(&request, "localhost", server, &local, &remote);
    assert!(Arc::ptr_eq(&request.headers, &variables.headers));

    let mut copied = request.clone();
    assert!(Arc::ptr_eq(&request.headers, &copied.headers));
    Arc::make_mut(&mut copied.headers)[0].value = "changed.test".into();
    assert_eq!(request.headers[0].value, "localhost");
    assert_eq!(copied.headers[0].value, "changed.test");
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn preserves_the_original_remote_port_for_real_ip_requests() {
    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            http {
                set_real_ip_from 127.0.0.0/8;
                real_ip_header X-Forwarded-For;
                server {
                    listen 127.0.0.1:18576;
                    location / {
                        return 200 "$remote_addr|$realip_remote_addr|$realip_remote_port\n";
                    }
                }
            }
        "#,
        &[],
    );
    let response = request_body(
        &config,
        "GET / HTTP/1.1\r\n\
         Host: localhost\r\n\
         X-Forwarded-For: 203.0.113.10\r\n\
         Connection: close\r\n\r\n",
    )
    .await;
    assert!(
        response.ends_with("203.0.113.10|127.0.0.1|45678\n"),
        "{response}"
    );
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn exposes_proxy_target_host_and_port_to_proxy_headers() {
    let upstream = match TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => listener,
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!("skipping proxy variable test: binding TCP sockets is not permitted");
            return;
        }
        Err(error) => panic!("bind upstream: {error}"),
    };
    let upstream_address = upstream.local_addr().unwrap();
    let backend = tokio::spawn(async move {
        let (stream, _) = upstream.accept().await.unwrap();
        let mut reader = BufReader::new(stream);
        let mut request_line = String::new();
        reader.read_line(&mut request_line).await.unwrap();
        assert_eq!(request_line, "GET /variables HTTP/1.1\r\n");
        let mut host = None;
        let mut port = None;
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).await.unwrap();
            if line == "\r\n" || line.is_empty() {
                break;
            }
            let lower = line.to_ascii_lowercase();
            if let Some(value) = lower.strip_prefix("x-proxy-host: ") {
                host = Some(value.trim().to_string());
            } else if let Some(value) = lower.strip_prefix("x-proxy-port: ") {
                port = Some(value.trim().to_string());
            }
        }
        assert_eq!(host, Some(upstream_address.to_string()));
        assert_eq!(port, Some(upstream_address.port().to_string()));
        reader
            .get_mut()
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Length: 8\r\nConnection: close\r\n\r\nproxied\n",
            )
            .await
            .unwrap();
    });
    let (prefix, config) = test_config(
        &format!(
            r#"
                worker_processes 1;
                events {{ worker_connections 64; }}
                http {{
                    server {{
                        listen 127.0.0.1:18577;
                        location / {{
                            proxy_pass http://{upstream_address};
                            proxy_http_version 1.1;
                            proxy_set_header Connection "";
                            proxy_set_header X-Proxy-Host $proxy_host;
                            proxy_set_header X-Proxy-Port $proxy_port;
                        }}
                    }}
                }}
            "#
        ),
        &[],
    );
    let response = request_body(
        &config,
        "GET /variables HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    backend.await.unwrap();
    assert!(response.starts_with("HTTP/1.1 200 OK\r\n"), "{response}");
    assert!(response.ends_with("proxied\n"), "{response}");
    let _ = std::fs::remove_dir_all(prefix);
}
