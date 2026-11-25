use std::future::poll_fn;

use super::support::*;
use super::*;

#[test]
fn evaluates_early_hint_predicates_with_nginx_truthiness() {
    let (prefix, config) = test_config(
        r#"
            events { worker_connections 64; }
            http {
                early_hints $http_link $arg_preload;
                server {
                    listen 127.0.0.1:18462;
                    location / {}
                    location /disabled/ {
                        early_hints $http_missing;
                    }
                }
            }
        "#,
        &[],
    );
    let server = &config.http.as_ref().unwrap().servers[0];
    let request = Request {
        method: "GET".into(),
        target: "/disabled/?preload=1".into(),
        version: "HTTP/1.1".into(),
        headers: (vec![
            Header {
                name: "Host".into(),
                value: "localhost".into(),
            },
            Header {
                name: "Link".into(),
                value: "</style.css>; rel=preload".into(),
            },
        ])
        .into(),
        body_stream: None,
        body: Vec::new(),
        remote_addr: "127.0.0.1:45678".into(),
        local_addr: "127.0.0.1:18462".into(),
        keep_alive: true,
    };
    let remote: SocketAddr = "127.0.0.1:45678".parse().unwrap();
    let local: SocketAddr = "127.0.0.1:18462".parse().unwrap();
    let variables = Variables::new(&request, "localhost", server, &local, &remote);

    assert!(should_send_early_hints(
        &server.locations[0].context.early_hints,
        &variables
    ));
    assert!(!should_send_early_hints(
        &server.locations[1].context.early_hints,
        &variables
    ));

    let mut zero = variables.clone();
    Arc::make_mut(&mut zero.headers)[1].value = "0".into();
    zero.args.clear();
    assert!(!should_send_early_hints(
        &server.locations[0].context.early_hints,
        &zero
    ));
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn writes_103_before_the_final_http11_response() {
    let (mut client, mut server) = tokio::io::duplex(16 * 1024);
    let task = tokio::spawn(async move {
        let mut response = Response::bytes(200, "text/plain", "ok");
        response.set_early_hints(vec![vec![
            Header {
                name: "Link".into(),
                value: "</style.css>; rel=preload".into(),
            },
            Header {
                name: "X-Test".into(),
                value: "line\r\ninjection".into(),
            },
        ]]);
        write_response(
            &mut server,
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
        .unwrap();
    });
    let mut output = Vec::new();
    client.read_to_end(&mut output).await.unwrap();
    task.await.unwrap();
    let output = String::from_utf8(output).unwrap();

    assert!(
        output.starts_with("HTTP/1.1 103 Early Hints\r\n"),
        "{output}"
    );
    assert!(output.contains("Link: </style.css>; rel=preload\r\n"));
    assert!(output.contains("X-Test: line  injection\r\n"));
    let informational = output.find("HTTP/1.1 103 Early Hints").unwrap();
    let final_response = output.find("HTTP/1.1 200 OK").unwrap();
    assert!(informational < final_response, "{output}");
}

#[tokio::test]
async fn does_not_write_103_for_http10() {
    let (mut client, mut server) = tokio::io::duplex(16 * 1024);
    let task = tokio::spawn(async move {
        let mut response = Response::bytes(200, "text/plain", "ok");
        response.set_early_hints(vec![vec![Header {
            name: "Link".into(),
            value: "</style.css>; rel=preload".into(),
        }]]);
        write_response(
            &mut server,
            &mut response,
            ResponseWriteOptions {
                request_method: "GET",
                request_version: "HTTP/1.0",
                keep_alive: false,
                keep_alive_header: None,
                server_tokens: ServerTokens::On,
                chunked_transfer_encoding: true,
                rate: ResponseRate::default(),
            },
        )
        .await
        .unwrap();
    });
    let mut output = Vec::new();
    client.read_to_end(&mut output).await.unwrap();
    task.await.unwrap();
    let output = String::from_utf8(output).unwrap();

    assert!(!output.contains("103 Early Hints"), "{output}");
    assert!(output.starts_with("HTTP/1.1 200 OK\r\n"), "{output}");
}

#[tokio::test]
async fn reads_103_headers_before_the_final_upstream_response() {
    let (mut upstream, server) = tokio::io::duplex(16 * 1024);
    upstream
        .write_all(
            b"HTTP/1.1 103 Early Hints\r\n\
              Link: </style.css>; rel=preload\r\n\
              Link: </app.js>; rel=modulepreload\r\n\r\n\
              HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok",
        )
        .await
        .unwrap();
    upstream.shutdown().await.unwrap();
    let mut reader = BufReader::new(server);
    let response = crate::http::read_upstream_response(&mut reader)
        .await
        .unwrap();

    assert_eq!(response.status, 200);
    assert_eq!(response.body.len(), 2);
    assert_eq!(response.early_hints.len(), 1);
    assert_eq!(response.early_hints[0].len(), 2);
    assert_eq!(response.early_hints[0][0].name, "Link");
    assert_eq!(
        response.early_hints[0][1].value,
        "</app.js>; rel=modulepreload"
    );
}

#[tokio::test]
async fn sends_103_as_http2_informational_headers() {
    let (client_io, server_io) = tokio::io::duplex(16 * 1024);
    let server = tokio::spawn(async move {
        let mut connection = h2::server::handshake(server_io).await.unwrap();
        let (_, mut respond) = connection.accept().await.unwrap().unwrap();
        let send = tokio::spawn(async move {
            send_h2_early_hints(
                &mut respond,
                &[vec![Header {
                    name: "Link".into(),
                    value: "</style.css>; rel=preload".into(),
                }]],
            )
            .unwrap();
            send_h2_response(
                respond,
                Response::bytes(200, "text/plain", "ok"),
                "GET",
                ServerTokens::On,
                16 * 1024,
            )
            .await
            .unwrap();
        });
        connection.graceful_shutdown();
        while connection.accept().await.is_some() {}
        send.await.unwrap();
    });

    let (mut sender, connection) = h2::client::handshake(client_io).await.unwrap();
    let connection = tokio::spawn(connection);
    let request = http::Request::builder()
        .uri("https://localhost/")
        .body(())
        .unwrap();
    let (mut response, _send_stream) = sender.send_request(request, true).unwrap();
    let informational = poll_fn(|context| response.poll_informational(context))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(informational.status(), StatusCode::EARLY_HINTS);
    assert_eq!(
        informational.headers().get("link").unwrap(),
        "</style.css>; rel=preload"
    );

    let response = response.await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.into_body().data().await.unwrap().unwrap(), "ok");
    server.await.unwrap();
    let _ = connection.await;
}

#[tokio::test]
async fn proxies_upstream_103_through_http11() {
    let upstream = match TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => listener,
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!("skipping early hints proxy test: binding TCP sockets is not permitted");
            return;
        }
        Err(error) => panic!("bind upstream: {error}"),
    };
    let upstream_address = upstream.local_addr().unwrap();
    let backend = tokio::spawn(async move {
        let (mut stream, _) = upstream.accept().await.unwrap();
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
            .write_all(
                b"HTTP/1.1 103 Early Hints\r\n\
                  Link: </style.css>; rel=preload\r\n\r\n\
                  HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok",
            )
            .await
            .unwrap();
    });
    let source = format!(
        r#"
            events {{ worker_connections 64; }}
            http {{
                server {{
                    listen 127.0.0.1:18463;
                    location / {{
                        early_hints $http_x_want_hints;
                        proxy_pass http://{upstream_address};
                        proxy_http_version 1.1;
                    }}
                }}
            }}
        "#
    );
    let (prefix, config) = test_config(&source, &[]);
    let response = request_body(
        &config,
        "GET / HTTP/1.1\r\nHost: localhost\r\nX-Want-Hints: yes\r\nConnection: close\r\n\r\n",
    )
    .await;
    backend.await.unwrap();

    assert!(
        response.starts_with("HTTP/1.1 103 Early Hints\r\n"),
        "{response}"
    );
    assert!(response.contains("Link: </style.css>; rel=preload\r\n"));
    assert!(
        response.find("HTTP/1.1 103 Early Hints").unwrap()
            < response.find("HTTP/1.1 200 OK").unwrap()
    );
    assert!(response.ends_with("\r\n\r\nok"), "{response}");
    let _ = std::fs::remove_dir_all(prefix);
}
