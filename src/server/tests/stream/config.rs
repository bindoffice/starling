use super::*;

#[test]
fn compiles_tcp_stream_proxy_configuration() {
    let (prefix, config) = test_config(
        r#"
                worker_processes 1;
                events { worker_connections 64; }
                stream {
                    ssl_password_file cert-password.txt;
                    proxy_ssl_password_file client-password.txt;
                    upstream database {
                        least_time first_byte inflight;
                        server 127.0.0.1:15432;
                    }
                    server {
                        listen 127.0.0.1:13306 proxy_protocol;
                        proxy_pass database;
                        proxy_timeout 30s;
                        proxy_connect_timeout 5s;
                        proxy_ssl on;
                        proxy_ssl_server_name on;
                        proxy_ssl_name localhost;
                        proxy_ssl_verify on;
                        proxy_ssl_trusted_certificate ca.pem;
                        proxy_ssl_verify_depth 2;
                        proxy_ssl_crl stream.crl;
                        proxy_ssl_certificate stream-client.crt;
                        proxy_ssl_certificate_key stream-client.key;
                    }
                }
            "#,
        &[
            ("cert-password.txt", "server-secret\n"),
            ("client-password.txt", "wrong\nstream-secret\n"),
        ],
    );
    let stream = config.stream.as_ref().unwrap();
    assert_eq!(stream.servers.len(), 1);
    assert_eq!(
        stream.servers[0].ssl_passwords,
        vec![b"server-secret".to_vec()]
    );
    assert_eq!(
        stream.servers[0].proxy_ssl_passwords,
        vec![b"wrong".to_vec(), b"stream-secret".to_vec()]
    );
    assert_eq!(
        stream.upstreams["database"].method,
        UpstreamMethod::LeastTime(LeastTimeConfig {
            mode: LeastTimeMode::FirstByte,
            inflight: true,
        })
    );
    assert_eq!(
        stream.servers[0]
            .proxy_pass
            .as_ref()
            .unwrap()
            .upstream
            .as_deref(),
        Some("database")
    );
    assert_eq!(stream.servers[0].proxy_timeout, Duration::from_secs(30));
    assert_eq!(
        stream.servers[0].proxy_connect_timeout,
        Duration::from_secs(5)
    );
    assert!(stream.servers[0].proxy_ssl);
    assert!(stream.servers[0].proxy_ssl_server_name);
    assert_eq!(
        stream.servers[0].proxy_ssl_name.as_deref(),
        Some("localhost")
    );
    assert!(stream.servers[0].proxy_ssl_verify);
    assert_eq!(
        stream.servers[0].proxy_ssl_trusted_certificate.as_deref(),
        Some(prefix.join("ca.pem").as_path())
    );
    assert_eq!(stream.servers[0].proxy_ssl_verify_depth, 2);
    assert_eq!(
        stream.servers[0].proxy_ssl_crl.as_deref(),
        Some(prefix.join("stream.crl").as_path())
    );
    assert_eq!(
        stream.servers[0].proxy_ssl_certificate.as_deref(),
        Some(prefix.join("stream-client.crt").as_path())
    );
    assert_eq!(
        stream.servers[0].proxy_ssl_certificate_key.as_deref(),
        Some(prefix.join("stream-client.key").as_path())
    );
    let routes = build_stream_routes(&config);
    assert_eq!(routes.len(), 1);
    assert_eq!(routes[0].address.port(), 13306);
    assert!(routes[0].proxy_protocol);
    let _ = std::fs::remove_dir_all(prefix);
}

#[test]
fn compiles_stream_log_formats_and_access_log_inheritance() {
    let (prefix, config) = test_config(
        r#"
                worker_processes 1;
                events { worker_connections 64; }
                stream {
                    log_format session '$protocol $status $server_name $bytes_in $bytes_out';
                    access_log logs/stream.log session if=$status;
                    server {
                        listen 127.0.0.1:13308;
                        server_name .Example.COM;
                        proxy_pass 127.0.0.1:15434;
                    }
                    server {
                        listen 127.0.0.1:13309;
                        access_log off;
                        proxy_pass 127.0.0.1:15434;
                    }
                }
            "#,
        &[],
    );
    let stream = config.stream.as_ref().unwrap();
    assert_eq!(
        stream.log_formats["session"].template,
        "$protocol $status $server_name $bytes_in $bytes_out"
    );
    assert_eq!(stream.servers[0].server_name, "example.com");
    assert_eq!(
        stream.servers[0]
            .access_log
            .logs
            .first()
            .and_then(|log| log.destination.as_fixed_path()),
        Some(prefix.join("logs/stream.log").as_path())
    );
    assert!(stream.servers[1].access_log.off);
    assert!(stream.servers[1].access_log.logs.is_empty());
    assert_eq!(
        stream.servers[0]
            .access_log
            .logs
            .first()
            .and_then(|log| log.condition.as_deref()),
        Some("$status")
    );

    let missing_format = test_config_error(
        r#"
                worker_processes 1;
                events { worker_connections 64; }
                stream {
                    server {
                        listen 127.0.0.1:13310;
                        access_log logs/stream.log;
                        proxy_pass 127.0.0.1:15434;
                    }
                }
            "#,
    );
    assert!(missing_format.contains("log format is not specified"));

    let unknown_format = test_config_error(
        r#"
                worker_processes 1;
                events { worker_connections 64; }
                stream {
                    server {
                        listen 127.0.0.1:13311;
                        access_log logs/stream.log missing;
                        proxy_pass 127.0.0.1:15434;
                    }
                }
            "#,
    );
    assert!(unknown_format.contains("unknown log format \"missing\""));
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn writes_stream_access_logs_with_session_and_upstream_variables() {
    let backend = match TcpListener::bind("127.0.0.1:0").await {
        Ok(backend) => backend,
        Err(err) if err.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!("skipping stream access log test: binding sockets is not permitted");
            return;
        }
        Err(err) => panic!("bind stream access log backend: {err}"),
    };
    let backend_address = backend.local_addr().unwrap();
    let backend_task = tokio::spawn(async move {
        let (mut stream, _) = backend.accept().await.unwrap();
        let mut request = [0_u8; 4];
        stream.read_exact(&mut request).await.unwrap();
        assert_eq!(&request, b"ping");
        stream.write_all(b"pong").await.unwrap();
        stream.shutdown().await.unwrap();
    });
    let source = format!(
        r#"
                worker_processes 1;
                events {{ worker_connections 64; }}
                stream {{
                    log_format session
                        '$remote_addr:$remote_port $server_addr:$server_port '
                        '$protocol $status $server_name '
                        '$bytes_received $bytes_sent '
                        '$upstream_addr $upstream_bytes_sent $upstream_bytes_received';
                    access_log logs/stream-access.log session;
                    server {{
                        listen 127.0.0.1:13312;
                        server_name demo;
                        proxy_pass {backend_address};
                    }}
                }}
            "#
    );
    let (prefix, config) = test_config(&source, &[]);
    let route = build_stream_routes(&config)[0].clone();
    let state = test_state(&config);
    let remote: SocketAddr = "127.0.0.1:45679".parse().unwrap();
    let addresses = ProxyProtocolAddresses::without_header(&ListenAddress::Inet(remote));
    let (mut client, inbound) = tokio::io::duplex(1024);
    let proxy = tokio::spawn(async move {
        handle_stream_connection(inbound, &addresses, route, state).await;
    });
    client.write_all(b"ping").await.unwrap();
    let mut response = Vec::new();
    client.read_to_end(&mut response).await.unwrap();
    assert_eq!(response, b"pong");
    proxy.await.unwrap();
    backend_task.await.unwrap();

    let logged = std::fs::read_to_string(prefix.join("logs/stream-access.log")).unwrap();
    assert_eq!(
        logged,
        format!("127.0.0.1:45679 127.0.0.1:13312 TCP 200 demo 4 4 {backend_address} 4 4\n")
    );
    let _ = std::fs::remove_dir_all(prefix);
}

#[test]
fn validates_least_time_modes_by_upstream_context() {
    let http_error = test_config_error(&upstream_config("least_time first_byte;"));
    assert!(http_error.contains("invalid parameter \"first_byte\""));
    let stream_error = test_config_error(
        r#"
                worker_processes 1;
                events { worker_connections 64; }
                stream {
                    upstream backend {
                        least_time header;
                        server 127.0.0.1:15433;
                    }
                    server {
                        listen 127.0.0.1:13307;
                        proxy_pass backend;
                    }
                }
            "#,
    );
    assert!(stream_error.contains("invalid parameter \"header\""));
}
