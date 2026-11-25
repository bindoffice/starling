use super::*;

#[test]
fn builds_nginx_compatible_proxy_paths() {
    let (prefix, config) = test_config(
        r#"
                worker_processes 1;
                events { worker_connections 64; }
                http {
                    server {
                        listen 127.0.0.1:18087;
                        location /rewrite/ {
                            proxy_pass http://127.0.0.1:19000/;
                        }
                        location /passthrough/ {
                            proxy_pass http://127.0.0.1:19000;
                        }
                    }
                }
            "#,
        &[],
    );
    let http = config.http.as_ref().unwrap();
    let server = &http.servers[0];
    let request = Request {
        method: "GET".into(),
        target: "/rewrite/a%20b?x=1".into(),
        version: "HTTP/1.1".into(),
        headers: (vec![Header {
            name: "Host".into(),
            value: "localhost".into(),
        }])
        .into(),
        body_stream: None,
        body: Vec::new(),
        remote_addr: "127.0.0.1:1234".into(),
        local_addr: "127.0.0.1:18087".into(),
        keep_alive: false,
    };
    let remote: SocketAddr = "127.0.0.1:1234".parse().unwrap();
    let local: SocketAddr = "127.0.0.1:18087".parse().unwrap();
    let variables = Variables::new(&request, "localhost", server, &local, &remote);

    let rewrite_location = &server.locations[0];
    let rewrite_env = RequestEnv {
        uri: normalize_uri("/rewrite/a%20b").unwrap(),
        args: Some("x=1".into()),
        unparsed_uri: Some("/rewrite/a%20b?x=1".into()),
        valid_location: true,
        internal: false,
        bypass_try_files: false,
        add_uri_to_alias: false,
        limits_checked: false,
        auth_request_checked: false,
        mirror_checked: false,
    };
    assert_eq!(
        proxy_path(
            rewrite_location.context.proxy_pass.as_ref().unwrap(),
            &rewrite_env,
            Some(rewrite_location),
            &variables,
        ),
        "/a%20b?x=1"
    );

    let passthrough_location = &server.locations[1];
    let mut passthrough_env = rewrite_env.clone();
    passthrough_env.uri = normalize_uri("/passthrough/a%20b").unwrap();
    passthrough_env.unparsed_uri = Some("/passthrough/a%20b?x=1".into());
    assert_eq!(
        proxy_path(
            passthrough_location.context.proxy_pass.as_ref().unwrap(),
            &passthrough_env,
            Some(passthrough_location),
            &variables,
        ),
        "/passthrough/a%20b?x=1"
    );

    passthrough_env.unparsed_uri = None;
    assert_eq!(
        proxy_path(
            passthrough_location.context.proxy_pass.as_ref().unwrap(),
            &passthrough_env,
            Some(passthrough_location),
            &variables,
        ),
        "/passthrough/a%20b?x=1"
    );
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn applies_proxy_pass_override_from_limit_except() {
    let upstream = match TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => listener,
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!("skipping limit_except proxy test: binding TCP sockets is not permitted");
            return;
        }
        Err(error) => panic!("bind upstream: {error}"),
    };
    let upstream_address = upstream.local_addr().unwrap();
    let backend = tokio::spawn(async move {
        let (mut stream, _) = upstream.accept().await.unwrap();
        let mut reader = BufReader::new(&mut stream);
        let mut request_line = String::new();
        reader.read_line(&mut request_line).await.unwrap();
        assert!(request_line.starts_with("POST /limited "), "{request_line}");
        let mut line = String::new();
        loop {
            line.clear();
            reader.read_line(&mut line).await.unwrap();
            if line == "\r\n" || line.is_empty() {
                break;
            }
        }
        reader
            .get_mut()
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Length: 8\r\nConnection: close\r\n\r\nproxied\n",
            )
            .await
            .unwrap();
    });
    let source = format!(
        r#"
                worker_processes 1;
                events {{ worker_connections 64; }}
                http {{
                    server {{
                        listen 127.0.0.1:18186;
                        location /limited {{
                            limit_except GET {{
                                proxy_pass http://{upstream_address};
                            }}
                        }}
                    }}
                }}
            "#
    );
    let (prefix, config) = test_config(&source, &[]);
    let response = request_body(
        &config,
        "POST /limited HTTP/1.1\r\nHost: localhost\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(
        response.starts_with("HTTP/1.1 200 OK\r\n") && response.ends_with("proxied\n"),
        "unexpected limit_except proxy response:\n{response}"
    );
    backend.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}
