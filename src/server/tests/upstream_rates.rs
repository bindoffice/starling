use super::support::*;
use super::*;

async fn read_headers<R: AsyncRead + Unpin>(stream: &mut R) {
    let mut request = Vec::new();
    let mut byte = [0_u8; 1];
    while !request.ends_with(b"\r\n\r\n") {
        stream.read_exact(&mut byte).await.unwrap();
        request.push(byte[0]);
    }
}

#[tokio::test]
async fn throttles_proxy_fastcgi_uwsgi_and_scgi_reads() {
    let body = "x".repeat(2048);
    let proxy_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let fastcgi_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let uwsgi_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let scgi_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy_address = proxy_listener.local_addr().unwrap();
    let fastcgi_address = fastcgi_listener.local_addr().unwrap();
    let uwsgi_address = uwsgi_listener.local_addr().unwrap();
    let scgi_address = scgi_listener.local_addr().unwrap();

    let proxy_body = body.clone();
    let proxy = tokio::spawn(async move {
        let (mut stream, _) = proxy_listener.accept().await.unwrap();
        read_headers(&mut stream).await;
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\n\r\n{}",
            proxy_body.len(),
            proxy_body
        );
        stream.write_all(response.as_bytes()).await.unwrap();
        stream.shutdown().await.unwrap();
    });
    let fastcgi_body = body.clone();
    let fastcgi = tokio::spawn(async move {
        let (mut stream, _) = fastcgi_listener.accept().await.unwrap();
        loop {
            let (record_type, content) = read_fastcgi_test_record(&mut stream).await;
            if record_type == FCGI_STDIN && content.is_empty() {
                break;
            }
        }
        let response = format!(
            "Status: 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\n\r\n{}",
            fastcgi_body.len(),
            fastcgi_body
        );
        write_fastcgi_record(&mut stream, FCGI_STDOUT, response.as_bytes())
            .await
            .unwrap();
        write_fastcgi_record(&mut stream, FCGI_END_REQUEST, &[0; 8])
            .await
            .unwrap();
        stream.shutdown().await.unwrap();
    });
    let uwsgi_body = body.clone();
    let uwsgi = tokio::spawn(async move {
        let (mut stream, _) = uwsgi_listener.accept().await.unwrap();
        let mut header = [0_u8; 4];
        stream.read_exact(&mut header).await.unwrap();
        let mut parameters = vec![0_u8; u16::from_le_bytes([header[1], header[2]]) as usize];
        stream.read_exact(&mut parameters).await.unwrap();
        let response = format!(
            "Status: 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\n\r\n{}",
            uwsgi_body.len(),
            uwsgi_body
        );
        stream.write_all(response.as_bytes()).await.unwrap();
        stream.shutdown().await.unwrap();
    });
    let scgi_body = body.clone();
    let scgi = tokio::spawn(async move {
        let (mut stream, _) = scgi_listener.accept().await.unwrap();
        let mut length = Vec::new();
        let mut byte = [0_u8; 1];
        loop {
            stream.read_exact(&mut byte).await.unwrap();
            if byte[0] == b':' {
                break;
            }
            length.push(byte[0]);
        }
        let length = std::str::from_utf8(&length)
            .unwrap()
            .parse::<usize>()
            .unwrap();
        let mut parameters = vec![0_u8; length];
        stream.read_exact(&mut parameters).await.unwrap();
        let response = format!(
            "Status: 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\n\r\n{}",
            scgi_body.len(),
            scgi_body
        );
        stream.write_all(response.as_bytes()).await.unwrap();
        stream.shutdown().await.unwrap();
    });

    let source = format!(
        r#"
            events {{ worker_connections 64; }}
            http {{
                server {{
                    listen 127.0.0.1:18475;
                    location = /proxy {{
                        proxy_pass http://{proxy_address};
                        proxy_limit_rate $http_x_rate;
                    }}
                    location = /fastcgi {{
                        fastcgi_pass {fastcgi_address};
                        fastcgi_limit_rate 8192;
                    }}
                    location = /uwsgi {{
                        uwsgi_pass {uwsgi_address};
                        uwsgi_limit_rate 8192;
                    }}
                    location = /scgi {{
                        scgi_pass {scgi_address};
                        scgi_limit_rate 8192;
                    }}
                }}
            }}
        "#
    );
    let (prefix, config) = test_config(&source, &[]);
    for path in ["/proxy", "/fastcgi", "/uwsgi", "/scgi"] {
        let rate_header = if path == "/proxy" {
            "X-Rate: 8192\r\n"
        } else {
            ""
        };
        let request = format!(
            "GET {path} HTTP/1.1\r\nHost: localhost\r\n{rate_header}Connection: close\r\n\r\n"
        );
        let started = Instant::now();
        let response = request_body(&config, &request).await;
        assert!(
            started.elapsed() >= Duration::from_millis(180),
            "{path} was not throttled: {:?}",
            started.elapsed()
        );
        assert!(
            response.starts_with("HTTP/1.1 200 OK\r\n"),
            "{path}: {response}"
        );
        assert!(response.ends_with(&body), "{path}: response body mismatch");
    }
    proxy.await.unwrap();
    fastcgi.await.unwrap();
    uwsgi.await.unwrap();
    scgi.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}
