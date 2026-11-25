use std::io;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;

use super::support::*;
use super::*;

fn request(target: &str, body: &[u8]) -> Request {
    Request {
        method: "POST".into(),
        target: target.into(),
        version: "HTTP/1.1".into(),
        headers: (vec![Header {
            name: "Host".into(),
            value: "localhost".into(),
        }])
        .into(),
        body_stream: None,
        body: body.to_vec(),
        remote_addr: "127.0.0.1:45678".into(),
        local_addr: "127.0.0.1:18080".into(),
        keep_alive: false,
    }
}

fn initial_env(uri: &str) -> RequestEnv {
    RequestEnv {
        uri: uri.into(),
        args: None,
        unparsed_uri: Some(uri.into()),
        valid_location: true,
        internal: false,
        bypass_try_files: false,
        add_uri_to_alias: false,
        limits_checked: false,
        auth_request_checked: false,
        mirror_checked: false,
    }
}

#[tokio::test]
async fn executes_path_and_named_post_actions() {
    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            http {
                server {
                    listen 127.0.0.1:18474;
                    location /main {
                        post_action /after?step=one;
                        return 200;
                    }
                    location = /after {
                        return 202 "step=$arg_step body=$request_body";
                    }
                    location @finish { return 418 "finished"; }
                }
            }
        "#,
        &[],
    );
    let http = config.http.as_ref().unwrap();
    let server = &http.servers[0];
    let request = request("/main", b"payload");
    let local = ListenAddress::Inet("127.0.0.1:18474".parse().unwrap());
    let remote = ListenAddress::Inet("127.0.0.1:45678".parse().unwrap());
    let mut variables = Variables::new(&request, "localhost", server, &local, &remote);

    let path = execute_post_actions(
        http,
        server,
        &request,
        initial_env("/main"),
        &mut variables,
        "/after?step=one".into(),
        1,
    )
    .await
    .expect("path post_action");
    assert_eq!(path.response.status, 202);
    let body = match path.response.body.into_memory(MAX_BODY_BYTES as u64).await {
        Ok(body) => body,
        Err(_) => panic!("post_action response body is too large"),
    };
    assert_eq!(body, b"step=one body=payload");

    let named = execute_post_actions(
        http,
        server,
        &request,
        initial_env("/main"),
        &mut variables,
        "@finish".into(),
        1,
    )
    .await
    .expect("named post_action");
    assert_eq!(named.response.status, 418);
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn chains_post_actions_until_no_target_remains() {
    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            http {
                server {
                    listen 127.0.0.1:18475;
                    location = /first {
                        post_action /second;
                        return 201;
                    }
                    location = /second {
                        post_action @third;
                        return 202;
                    }
                    location @third { return 203; }
                }
            }
        "#,
        &[],
    );
    let http = config.http.as_ref().unwrap();
    let server = &http.servers[0];
    let request = request("/first", b"");
    let local = ListenAddress::Inet("127.0.0.1:18475".parse().unwrap());
    let remote = ListenAddress::Inet("127.0.0.1:45678".parse().unwrap());
    let mut variables = Variables::new(&request, "localhost", server, &local, &remote);
    let result = execute_post_actions(
        http,
        server,
        &request,
        initial_env("/first"),
        &mut variables,
        "/first".into(),
        1,
    )
    .await
    .expect("post_action chain");
    assert_eq!(result.response.status, 203);
    let _ = std::fs::remove_dir_all(prefix);
}

async fn read_request(stream: &mut TcpStream) -> Vec<u8> {
    let mut data = Vec::new();
    let header_end = loop {
        if let Some(position) = data.windows(4).position(|window| window == b"\r\n\r\n") {
            break position + 4;
        }
        let mut buffer = [0_u8; 4096];
        let read = stream.read(&mut buffer).await.unwrap();
        assert!(read > 0, "upstream closed before request headers");
        data.extend_from_slice(&buffer[..read]);
    };
    let headers = String::from_utf8_lossy(&data[..header_end]);
    let content_length = headers
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().unwrap())
        })
        .unwrap_or(0);
    while data.len() < header_end + content_length {
        let mut buffer = [0_u8; 4096];
        let read = stream.read(&mut buffer).await.unwrap();
        assert!(read > 0, "upstream closed before request body");
        data.extend_from_slice(&buffer[..read]);
    }
    data
}

#[tokio::test]
async fn dispatches_post_action_without_replacing_client_response() {
    let listener = match TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => listener,
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!("skipping post_action dispatch test: binding sockets is not permitted");
            return;
        }
        Err(error) => panic!("bind post_action upstream: {error}"),
    };
    let upstream_port = listener.local_addr().unwrap().port();
    let (sender, mut receiver) = mpsc::unbounded_channel();
    let upstream = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let request = read_request(&mut stream).await;
        sender.send(request).unwrap();
        stream
            .write_all(b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();
    });

    let (prefix, config) = test_config(
        &r#"
            worker_processes 1;
            events { worker_connections 64; }
            http {
                server {
                    listen 127.0.0.1:18476;
                    location = /main {
                        post_action /after;
                        return 201 "original";
                    }
                    location = /after {
                        proxy_pass http://127.0.0.1:UPSTREAM_PORT;
                    }
                }
            }
        "#
        .replace("UPSTREAM_PORT", &upstream_port.to_string()),
        &[],
    );
    let response = request_bytes(
        &config,
        "GET /main HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(response.starts_with(b"HTTP/1.1 201 Created\r\n"));
    assert!(response.ends_with(b"original"));

    let post = tokio::time::timeout(Duration::from_secs(2), receiver.recv())
        .await
        .expect("post_action request timed out")
        .unwrap();
    assert!(
        String::from_utf8_lossy(&post).starts_with("GET /after HTTP/1.1\r\n"),
        "{}",
        String::from_utf8_lossy(&post)
    );
    upstream.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}
