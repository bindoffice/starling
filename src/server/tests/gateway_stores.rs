use super::support::*;
use super::*;

#[tokio::test]
async fn stores_complete_gateway_responses_for_all_gateway_types() {
    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            http {
                server {
                    listen 127.0.0.1:18135;
                    location /fastcgi {
                        fastcgi_pass 127.0.0.1:9000;
                        fastcgi_store gateway-fastcgi/$host$uri;
                        fastcgi_store_access user:r group:r;
                    }
                    location /uwsgi {
                        uwsgi_pass 127.0.0.1:9001;
                        uwsgi_store gateway-uwsgi/$host$uri;
                        uwsgi_store_access user:r;
                    }
                    location /scgi {
                        scgi_pass 127.0.0.1:9002;
                        scgi_store gateway-scgi/$host$uri;
                        scgi_store_access group:r all:r;
                    }
                }
            }
        "#,
        &[],
    );
    let server = &config.http.as_ref().unwrap().servers[0];
    for (index, (uri, expected, expected_mode)) in [
        ("/fastcgi", "gateway-fastcgi/localhost/fastcgi", 0o440),
        ("/uwsgi", "gateway-uwsgi/localhost/uwsgi", 0o400),
        ("/scgi", "gateway-scgi/localhost/scgi", 0o644),
    ]
    .into_iter()
    .enumerate()
    {
        let location = &server.locations[index];
        let request = request(uri);
        let variables = request_variables(server, &request);
        let response = Response::bytes(200, "text/plain", format!("body-{uri}"));
        let stored = store_gateway_response(
            &response,
            &request_env(uri),
            Some(location),
            &location.context,
            &variables,
        )
        .await
        .unwrap();
        let expected = prefix.join(expected);
        assert_eq!(stored.as_deref(), Some(expected.as_path()));
        assert_eq!(
            std::fs::read(&expected).unwrap(),
            format!("body-{uri}").as_bytes()
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&expected).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, expected_mode, "{uri}");
        }
    }
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn skips_gateway_store_for_off_and_non_200_responses() {
    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            http {
                server {
                    listen 127.0.0.1:18136;
                    location /off {
                        fastcgi_pass 127.0.0.1:9000;
                    }
                    location /status {
                        uwsgi_pass 127.0.0.1:9001;
                        uwsgi_store on;
                    }
                }
            }
        "#,
        &[],
    );
    let server = &config.http.as_ref().unwrap().servers[0];

    let off_request = request("/off/file");
    assert!(
        store_gateway_response(
            &Response::bytes(200, "text/plain", "off"),
            &request_env("/off/file"),
            Some(&server.locations[0]),
            &server.locations[0].context,
            &request_variables(server, &off_request),
        )
        .await
        .unwrap()
        .is_none()
    );

    let status_request = request("/status/file");
    assert!(
        store_gateway_response(
            &Response::bytes(201, "text/plain", "created"),
            &request_env("/status/file"),
            Some(&server.locations[1]),
            &server.locations[1].context,
            &request_variables(server, &status_request),
        )
        .await
        .unwrap()
        .is_none()
    );
    assert!(!prefix.join("html/off/file").exists());
    assert!(!prefix.join("html/status/file").exists());
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn stores_fastcgi_response_through_request_processing() {
    let listener = match TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => listener,
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!("skipping gateway_store TCP test: binding sockets is not permitted");
            return;
        }
        Err(error) => panic!("bind upstream: {error}"),
    };
    let upstream = listener.local_addr().unwrap();
    let backend = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        loop {
            let (record_type, content) = read_fastcgi_test_record(&mut stream).await;
            if record_type == FCGI_STDIN && content.is_empty() {
                break;
            }
        }
        write_fastcgi_record(
            &mut stream,
            FCGI_STDOUT,
            b"Status: 200 OK\r\nContent-Type: text/plain\r\n\r\ncgi-store\n",
        )
        .await
        .unwrap();
        write_fastcgi_record(&mut stream, FCGI_END_REQUEST, &[0; 8])
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
                        listen 127.0.0.1:18137;
                        location /stored {{
                            fastcgi_pass {upstream};
                            fastcgi_param SCRIPT_FILENAME /srv$fastcgi_script_name;
                            fastcgi_store on;
                            fastcgi_store_access user:r group:r;
                        }}
                    }}
                }}
            "#
        ),
        &[],
    );
    let response = request_body(
        &config,
        "GET /stored/file HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(response.starts_with("HTTP/1.1 200 OK\r\n"), "{response}");
    assert!(response.ends_with("cgi-store\n"), "{response}");
    assert_eq!(
        std::fs::read(prefix.join("html/stored/file")).unwrap(),
        b"cgi-store\n"
    );
    backend.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn streaming_gateway_store_finishes_after_client_abort_without_tcp() {
    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            http {
                server {
                    listen 127.0.0.1:18139;
                    location / {
                        uwsgi_pass 127.0.0.1:9001;
                        uwsgi_buffering off;
                        uwsgi_store on;
                        uwsgi_store_access user:r group:r;
                    }
                }
            }
        "#,
        &[],
    );
    let server = &config.http.as_ref().unwrap().servers[0];
    let location = &server.locations[0];
    let request = request("/stored");
    let variables = request_variables(server, &request);
    let (sender, receiver) = tokio::sync::mpsc::channel(1);
    let mut response = Response::new(200);
    response.headers.push(Header {
        name: "Content-Length".into(),
        value: "11".into(),
    });
    response.body = ResponseBody::Stream(ResponseStream::new(receiver, Some(11), false));

    serve_gateway_store(
        &mut response,
        &request_env("/stored"),
        Some(location),
        &location.context,
        &variables,
    )
    .await;
    let ResponseBody::Stream(mut stream) =
        std::mem::replace(&mut response.body, ResponseBody::Empty)
    else {
        panic!("gateway store should wrap the streaming body");
    };
    sender
        .send(Ok(ResponseStreamItem::Data(Bytes::from_static(b"first"))))
        .await
        .unwrap();
    sender
        .send(Ok(ResponseStreamItem::Data(Bytes::from_static(b"second"))))
        .await
        .unwrap();
    drop(sender);

    let first = stream.next().await.unwrap().unwrap();
    assert!(matches!(
        first,
        ResponseStreamItem::Data(data) if &data[..] == b"first"
    ));
    response.body = ResponseBody::Stream(stream);
    let drain = drain_after_client_abort(&mut response).expect("store tee should be preserved");
    tokio::time::timeout(Duration::from_secs(1), drain)
        .await
        .expect("gateway store drain should finish")
        .expect("gateway store drain should not panic");
    assert_eq!(
        std::fs::read(prefix.join("html/stored")).unwrap(),
        b"firstsecond"
    );
    let _ = std::fs::remove_dir_all(prefix);
}

fn request(uri: &str) -> Request {
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
        local_addr: "127.0.0.1:18135".into(),
        keep_alive: false,
    }
}

fn request_env(uri: &str) -> RequestEnv {
    RequestEnv {
        uri: uri.into(),
        args: None,
        unparsed_uri: Some(uri.into()),
        internal: false,
        bypass_try_files: false,
        limits_checked: false,
        auth_request_checked: false,
        mirror_checked: false,
    }
}

fn request_variables(server: &ServerConfig, request: &Request) -> Variables {
    let remote: SocketAddr = request.remote_addr.parse().unwrap();
    let local: SocketAddr = request.local_addr.parse().unwrap();
    Variables::new(request, "localhost", server, &local, &remote)
}
