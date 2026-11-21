use std::io;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;

use super::support::*;

async fn read_request(stream: &mut TcpStream) -> Vec<u8> {
    let mut data = Vec::new();
    let header_end = loop {
        if let Some(position) = data.windows(4).position(|window| window == b"\r\n\r\n") {
            break position + 4;
        }
        let mut buffer = [0_u8; 4096];
        let read = stream.read(&mut buffer).await.unwrap();
        assert!(
            read > 0,
            "upstream connection closed before request headers"
        );
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
        assert!(read > 0, "upstream connection closed before request body");
        data.extend_from_slice(&buffer[..read]);
    }
    data
}

#[tokio::test]
async fn mirrors_requests_in_the_background_with_body_control() {
    let listener = match TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => listener,
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!("skipping mirror test: binding sockets is not permitted");
            return;
        }
        Err(error) => panic!("bind mirror upstream: {error}"),
    };
    let upstream_port = listener.local_addr().unwrap().port();
    let (sender, mut receiver) = mpsc::unbounded_channel();
    let upstream = tokio::spawn(async move {
        for _ in 0..2 {
            let (mut stream, _) = listener.accept().await.unwrap();
            let request = read_request(&mut stream).await;
            sender.send(request).unwrap();
            stream
                .write_all(
                    b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                )
                .await
                .unwrap();
        }
    });

    let source = r#"
            worker_processes 1;
            events { worker_connections 64; }
            http {
                server {
                    listen 127.0.0.1:18171;
                    root html;
                    mirror /mirror;
                    location = /mirror {
                        proxy_pass http://127.0.0.1:UPSTREAM_PORT;
                    }
                    location /main/ { }
                    location /no-body/ {
                        mirror_request_body off;
                    }
                }
            }
        "#
    .replace("UPSTREAM_PORT", &upstream_port.to_string());
    let (prefix, config) = test_config(
        &source,
        &[
            ("html/main/site.txt", "main\n"),
            ("html/no-body/site.txt", "no body\n"),
        ],
    );

    let main = request_bytes(
        &config,
        "POST /main/site.txt?token=abc HTTP/1.1\r\nHost: localhost\r\nContent-Length: 7\r\nConnection: close\r\n\r\npayload",
    )
    .await;
    assert!(main.starts_with(b"HTTP/1.1 405 Not Allowed\r\n"));
    let mirrored = tokio::time::timeout(Duration::from_secs(2), receiver.recv())
        .await
        .expect("mirror request timed out")
        .unwrap();
    let mirrored = String::from_utf8(mirrored).unwrap();
    assert!(
        mirrored.starts_with("POST /mirror?token=abc HTTP/1.1\r\n"),
        "{mirrored}"
    );
    assert!(mirrored.ends_with("\r\n\r\npayload"), "{mirrored}");

    let without_body = request_bytes(
        &config,
        "POST /no-body/site.txt HTTP/1.1\r\nHost: localhost\r\nContent-Length: 7\r\nConnection: close\r\n\r\nignored",
    )
    .await;
    assert!(without_body.starts_with(b"HTTP/1.1 405 Not Allowed\r\n"));
    let mirrored = tokio::time::timeout(Duration::from_secs(2), receiver.recv())
        .await
        .expect("second mirror request timed out")
        .unwrap();
    let mirrored = String::from_utf8(mirrored).unwrap();
    assert!(
        mirrored.starts_with("POST /mirror HTTP/1.1\r\n"),
        "{mirrored}"
    );
    assert!(!mirrored.contains("Content-Length:"), "{mirrored}");
    assert!(mirrored.ends_with("\r\n\r\n"), "{mirrored}");

    upstream.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn mirror_and_main_upstream_replay_one_file_backed_request_body() {
    let listener = match TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => listener,
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!("skipping file-backed mirror test: binding sockets is not permitted");
            return;
        }
        Err(error) => panic!("bind file-backed mirror upstream: {error}"),
    };
    let upstream = listener.local_addr().unwrap();
    let (sender, mut receiver) = mpsc::unbounded_channel();
    let backend = tokio::spawn(async move {
        for _ in 0..2 {
            let (mut stream, _) = listener.accept().await.unwrap();
            let request = read_request(&mut stream).await;
            sender.send(request).unwrap();
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok")
                .await
                .unwrap();
        }
    });

    let (prefix, config) = test_config(
        &format!(
            r#"
                worker_processes 1;
                events {{ worker_connections 64; }}
                http {{
                    client_body_buffer_size 4;
                    client_body_temp_path body_temp;
                    server {{
                        listen 127.0.0.1:18667;
                        mirror /mirror;
                        location /main {{
                            proxy_pass http://{upstream};
                        }}
                        location = /mirror {{
                            proxy_pass http://{upstream};
                        }}
                    }}
                }}
            "#
        ),
        &[],
    );

    let response = request_body(
        &config,
        "POST /main HTTP/1.1\r\nHost: localhost\r\nContent-Length: 8\r\n\
         Connection: close\r\n\r\npartmore",
    )
    .await;
    assert!(response.starts_with("HTTP/1.1 200 OK\r\n"), "{response}");
    assert!(response.ends_with("ok"), "{response}");

    let first = receiver.recv().await.unwrap();
    let second = receiver.recv().await.unwrap();
    let mut requests = [first, second];
    requests.sort();
    assert!(
        requests[0].starts_with(b"POST /main HTTP/1.1\r\n"),
        "{requests:?}"
    );
    assert!(
        requests[1].starts_with(b"POST /mirror HTTP/1.1\r\n"),
        "{requests:?}"
    );
    for request in requests {
        assert!(request.ends_with(b"\r\n\r\npartmore"), "{request:?}");
    }

    backend.await.unwrap();
    let temp_root = prefix.join("body_temp");
    let mut removed = false;
    for _ in 0..200 {
        if !contains_generated_temp_file(&temp_root) {
            removed = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert!(
        removed,
        "file-backed body must be removed after main and mirror consumers finish"
    );
    let _ = std::fs::remove_dir_all(prefix);
}

#[test]
fn compiles_mirror_inheritance_and_off() {
    let (prefix, config) = test_config(
        r#"
                worker_processes 1;
                events { worker_connections 64; }
                http {
                    mirror /http;
                    server {
                        listen 127.0.0.1:18172;
                        mirror /first;
                        mirror /second;
                        location /inherit/ { }
                        location /off/ { mirror off; }
                    }
                }
            "#,
        &[],
    );
    let server = &config.http.as_ref().unwrap().servers[0];
    assert_eq!(server.context.mirrors, ["/first", "/second"]);
    assert_eq!(server.locations[0].context.mirrors, ["/first", "/second"]);
    assert!(server.locations[1].context.mirrors.is_empty());
    assert!(server.context.mirror_request_body);

    let duplicate = test_config_error(
        r#"
                events {}
                http {
                    server {
                        listen 127.0.0.1:18172;
                        mirror off;
                        mirror /later;
                    }
                }
            "#,
    );
    assert!(
        duplicate.contains("\"mirror\" directive is duplicate"),
        "{duplicate}"
    );
    let _ = std::fs::remove_dir_all(prefix);
}
