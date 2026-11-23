use super::*;

#[tokio::test]
async fn content_length_response_drains_end_request_before_keepalive_reuse() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let upstream = listener.local_addr().unwrap();
    let source = format!(
        r#"
            worker_processes 1;
            events {{ worker_connections 64; }}
            http {{
                upstream backend {{
                    server {upstream};
                    keepalive 2;
                }}
                server {{
                    listen 127.0.0.1:18139;
                    location / {{
                        fastcgi_pass backend;
                        fastcgi_keep_conn on;
                        fastcgi_read_timeout 2s;
                    }}
                }}
            }}
        "#
    );
    let (prefix, config) = test_config(&source, &[]);
    let server = &config.http.as_ref().unwrap().servers[0];
    let context = &server.locations[0].context;
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
        remote_addr: "127.0.0.1:45679".into(),
        local_addr: "127.0.0.1:18139".into(),
        keep_alive: false,
    };
    let remote: SocketAddr = "127.0.0.1:45679".parse().unwrap();
    let local: SocketAddr = "127.0.0.1:18139".parse().unwrap();
    let mut variables = Variables::new(&request, "localhost", server, &local, &remote);
    let env = RequestEnv {
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
    };

    let server_task = tokio::spawn(async move {
        let (mut fastcgi, _) = listener.accept().await.unwrap();
        for body in ["first", "second"] {
            loop {
                let (record_type, content) = read_fastcgi_test_record(&mut fastcgi).await;
                if record_type == FCGI_STDIN && content.is_empty() {
                    break;
                }
            }
            let stdout = format!(
                "Status: 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            );
            write_fastcgi_record(&mut fastcgi, FCGI_STDOUT, stdout.as_bytes())
                .await
                .unwrap();
            write_fastcgi_record(&mut fastcgi, FCGI_END_REQUEST, &[0; 8])
                .await
                .unwrap();
            fastcgi.flush().await.unwrap();
        }
        assert!(
            tokio::time::timeout(Duration::from_millis(100), listener.accept())
                .await
                .is_err(),
            "FastCGI keepalive should reuse the connection after draining END_REQUEST"
        );
    });

    let first = fastcgi_request(
        context.fastcgi_pass.as_ref().unwrap(),
        config.http.as_ref().unwrap(),
        &request,
        &env,
        context,
        &mut variables,
    )
    .await;
    assert_eq!(first.status, 200);
    assert_eq!(first.into_bytes(), b"first");
    tokio::time::sleep(Duration::from_millis(20)).await;

    let second = fastcgi_request(
        context.fastcgi_pass.as_ref().unwrap(),
        config.http.as_ref().unwrap(),
        &request,
        &env,
        context,
        &mut variables,
    )
    .await;
    assert_eq!(second.status, 200);
    assert_eq!(second.into_bytes(), b"second");
    server_task.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn reuses_fastcgi_connections_with_named_upstream_keepalive() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let upstream = listener.local_addr().unwrap();
    let source = format!(
        r#"
            worker_processes 1;
            events {{ worker_connections 64; }}
            http {{
                upstream backend {{
                    server {upstream};
                    keepalive 2;
                }}
                server {{
                    listen 127.0.0.1:18138;
                    location / {{
                        fastcgi_pass backend;
                        fastcgi_keep_conn on;
                        fastcgi_read_timeout 2s;
                    }}
                }}
            }}
        "#
    );
    let (prefix, config) = test_config(&source, &[]);
    let server = &config.http.as_ref().unwrap().servers[0];
    let context = &server.locations[0].context;
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
        local_addr: "127.0.0.1:18138".into(),
        keep_alive: false,
    };
    let remote: SocketAddr = "127.0.0.1:45678".parse().unwrap();
    let local: SocketAddr = "127.0.0.1:18138".parse().unwrap();
    let mut variables = Variables::new(&request, "localhost", server, &local, &remote);
    let env = RequestEnv {
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
    };

    let server_task = tokio::spawn(async move {
        let (mut fastcgi, _) = listener.accept().await.unwrap();
        for body in ["first", "second"] {
            loop {
                let (record_type, content) = read_fastcgi_test_record(&mut fastcgi).await;
                if record_type == FCGI_BEGIN_REQUEST {
                    assert_eq!(content[2], 1);
                }
                if record_type == FCGI_STDIN && content.is_empty() {
                    break;
                }
            }
            write_fastcgi_record(
                &mut fastcgi,
                FCGI_STDOUT,
                format!("Status: 200 OK\r\nContent-Type: text/plain\r\n\r\n{body}").as_bytes(),
            )
            .await
            .unwrap();
            write_fastcgi_record(&mut fastcgi, FCGI_END_REQUEST, &[0; 8])
                .await
                .unwrap();
            fastcgi.flush().await.unwrap();
        }
        assert!(
            tokio::time::timeout(Duration::from_millis(100), listener.accept())
                .await
                .is_err(),
            "FastCGI keepalive should not open a second connection"
        );
    });

    for body in ["first", "second"] {
        let response = fastcgi_request(
            context.fastcgi_pass.as_ref().unwrap(),
            config.http.as_ref().unwrap(),
            &request,
            &env,
            context,
            &mut variables,
        )
        .await;
        assert_eq!(response.status, 200);
        assert_eq!(response.into_bytes(), body.as_bytes());
    }
    server_task.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn rejects_matching_fastcgi_stderr_output() {
    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            http {
                server {
                    listen 127.0.0.1:18137;
                    location / {
                        fastcgi_pass 127.0.0.1:9000;
                        fastcgi_catch_stderr "PHP Fatal error";
                    }
                }
            }
        "#,
        &[],
    );
    let server = &config.http.as_ref().unwrap().servers[0];
    let context = &server.locations[0].context;
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
        local_addr: "127.0.0.1:18137".into(),
        keep_alive: false,
    };
    let remote: SocketAddr = "127.0.0.1:45678".parse().unwrap();
    let local: SocketAddr = "127.0.0.1:18137".parse().unwrap();
    let variables = Variables::new(&request, "localhost", server, &local, &remote);
    let env = RequestEnv {
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
    };
    let (client, mut fastcgi) = tokio::io::duplex(64 * 1024);
    let server_task = tokio::spawn(async move {
        loop {
            let mut header = [0_u8; 8];
            fastcgi.read_exact(&mut header).await.unwrap();
            let length = u16::from_be_bytes([header[4], header[5]]) as usize;
            let padding = header[6] as usize;
            let mut content = vec![0_u8; length];
            fastcgi.read_exact(&mut content).await.unwrap();
            if padding > 0 {
                let mut padding = vec![0_u8; padding];
                fastcgi.read_exact(&mut padding).await.unwrap();
            }
            if header[1] == FCGI_STDIN && content.is_empty() {
                break;
            }
        }
        write_fastcgi_record(
            &mut fastcgi,
            FCGI_STDERR,
            b"PHP Fatal error: backend failed",
        )
        .await
        .unwrap();
        fastcgi.flush().await.unwrap();
    });
    let result = fastcgi_exchange(
        client,
        &request,
        &env,
        Some(&server.locations[0]),
        context,
        &variables,
    )
    .await;
    assert!(matches!(result, Err(ProxyFailure::InvalidHeader)));
    server_task.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

#[test]
fn appends_fastcgi_index_to_script_names_ending_in_slash() {
    let (prefix, config) = test_config(
        r#"
                worker_processes 1;
                events { worker_connections 64; }
                http {
                    server {
                        listen 127.0.0.1:18121;
                        location / {
                            fastcgi_index index.php;
                            fastcgi_split_path_info ^(.+?\.php)(/.*)$;
                        }
                    }
                }
            "#,
        &[],
    );
    let context = &config.http.as_ref().unwrap().servers[0].locations[0].context;
    assert_eq!(
        fastcgi_names("/app/index.php/extra/path", context),
        ("/app/index.php".into(), "/extra/path".into())
    );
    assert_eq!(
        fastcgi_names("/app/", context),
        ("/app/index.php".into(), String::new())
    );
    let _ = std::fs::remove_dir_all(prefix);
}
