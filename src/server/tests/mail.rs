use super::support::*;
use super::*;
use crate::server::mail::{build_mail_routes, handle_mail_accepted};

#[tokio::test]
async fn proxies_smtp_after_auth_http_login() {
    let upstream = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let upstream_port = upstream.local_addr().unwrap().port();
    let auth = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let auth_port = auth.local_addr().unwrap().port();
    let (prefix, config) = test_config(
        &format!(
            r#"
                worker_processes 1;
                events {{ worker_connections 64; }}
                mail {{
                    auth_http http://127.0.0.1:{auth_port}/mail/auth;
                    xclient off;
                    server {{
                        listen 127.0.0.1:18025;
                        protocol smtp;
                        server_name mail.example.test;
                        smtp_auth plain login;
                    }}
                }}
            "#
        ),
        &[],
    );
    let auth_task = tokio::spawn(async move {
        let (mut stream, _) = auth.accept().await.unwrap();
        let request = read_until_headers(&mut stream).await;
        let request = String::from_utf8(request).unwrap();
        assert!(request.starts_with("GET /mail/auth HTTP/1.0\r\n"));
        assert!(request.contains("Auth-Method: plain\r\n"));
        assert!(request.contains("Auth-User: test@example.com\r\n"));
        assert!(request.contains("Auth-Pass: secret\r\n"));
        assert!(request.contains("Auth-Protocol: smtp\r\n"));
        stream
            .write_all(
                format!(
                    "HTTP/1.1 204 No Content\r\n\
                     Auth-Status: OK\r\n\
                     Auth-Server: 127.0.0.1\r\n\
                     Auth-Port: {upstream_port}\r\n\
                     Auth-Pass: secret\r\n\r\n"
                )
                .as_bytes(),
            )
            .await
            .unwrap();
    });
    let upstream_task = tokio::spawn(async move {
        let (stream, _) = upstream.accept().await.unwrap();
        let mut stream = BufReader::new(stream);
        stream.write_all(b"220 upstream ESMTP\r\n").await.unwrap();
        assert!(
            read_line_string(&mut stream)
                .await
                .starts_with("EHLO mail.example.test")
        );
        stream.write_all(b"250 upstream\r\n").await.unwrap();
        assert!(
            read_line_string(&mut stream)
                .await
                .starts_with("EHLO client.example")
        );
        stream.write_all(b"250 upstream\r\n").await.unwrap();
        assert_eq!(
            read_line_string(&mut stream).await,
            "MAIL FROM:<test@example.com>\r\n"
        );
        stream.write_all(b"250 queued\r\n").await.unwrap();
        stream.shutdown().await.unwrap();
    });

    let route = build_mail_routes(&config)[0].clone();
    let state = test_state(&config);
    let (mut client, server) = tokio::io::duplex(64 * 1024);
    let task = tokio::spawn(handle_mail_accepted(
        server,
        ListenAddress::Inet("127.0.0.1:45690".parse().unwrap()),
        route,
        state,
    ));
    assert!(read_duplex_line(&mut client).await.starts_with("220 "));
    client.write_all(b"EHLO client.example\r\n").await.unwrap();
    read_smtp_reply(&mut client).await;
    let credentials =
        base64::engine::general_purpose::STANDARD.encode(b"\0test@example.com\0secret");
    client
        .write_all(format!("AUTH PLAIN {credentials}\r\n").as_bytes())
        .await
        .unwrap();
    assert_eq!(read_duplex_line(&mut client).await, "235 2.0.0 OK\r\n");
    client
        .write_all(b"MAIL FROM:<test@example.com>\r\n")
        .await
        .unwrap();
    assert_eq!(read_duplex_line(&mut client).await, "250 queued\r\n");
    client.shutdown().await.unwrap();
    task.await.unwrap();
    auth_task.await.unwrap();
    upstream_task.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn proxies_pop3_user_and_pass() {
    let upstream = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let upstream_port = upstream.local_addr().unwrap().port();
    let auth = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let auth_port = auth.local_addr().unwrap().port();
    let (prefix, config) = test_config(
        &format!(
            r#"
                worker_processes 1;
                events {{ worker_connections 64; }}
                mail {{
                    auth_http http://127.0.0.1:{auth_port}/mail/auth;
                    server {{
                        listen 127.0.0.1:18110;
                        protocol pop3;
                    }}
                }}
            "#,
        ),
        &[],
    );
    let auth_task = tokio::spawn(async move {
        let (mut stream, _) = auth.accept().await.unwrap();
        let request = String::from_utf8(read_until_headers(&mut stream).await).unwrap();
        assert!(request.contains("Auth-Protocol: pop3\r\n"));
        stream
            .write_all(
                format!(
                    "HTTP/1.1 204 No Content\r\n\
                     Auth-Status: OK\r\n\
                     Auth-Server: 127.0.0.1\r\n\
                     Auth-Port: {upstream_port}\r\n\r\n"
                )
                .as_bytes(),
            )
            .await
            .unwrap();
    });
    let upstream_task = tokio::spawn(async move {
        let (stream, _) = upstream.accept().await.unwrap();
        let mut stream = BufReader::new(stream);
        stream.write_all(b"+OK upstream ready\r\n").await.unwrap();
        assert_eq!(
            read_line_string(&mut stream).await,
            "USER test@example.com\r\n"
        );
        stream.write_all(b"+OK user\r\n").await.unwrap();
        assert_eq!(read_line_string(&mut stream).await, "PASS secret\r\n");
        stream.write_all(b"+OK logged in\r\n").await.unwrap();
        assert_eq!(read_line_string(&mut stream).await, "STAT\r\n");
        stream.write_all(b"+OK 1 100\r\n").await.unwrap();
        stream.shutdown().await.unwrap();
    });

    let route = build_mail_routes(&config)[0].clone();
    let state = test_state(&config);
    let (mut client, server) = tokio::io::duplex(64 * 1024);
    let task = tokio::spawn(handle_mail_accepted(
        server,
        ListenAddress::Inet("127.0.0.1:45691".parse().unwrap()),
        route,
        state,
    ));
    assert!(read_duplex_line(&mut client).await.starts_with("+OK "));
    client
        .write_all(b"USER test@example.com\r\nPASS secret\r\n")
        .await
        .unwrap();
    assert_eq!(read_duplex_line(&mut client).await, "+OK\r\n");
    assert_eq!(read_duplex_line(&mut client).await, "+OK logged in\r\n");
    client.write_all(b"STAT\r\n").await.unwrap();
    assert_eq!(read_duplex_line(&mut client).await, "+OK 1 100\r\n");
    client.shutdown().await.unwrap();
    task.await.unwrap();
    auth_task.await.unwrap();
    upstream_task.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

async fn read_until_headers(stream: &mut TcpStream) -> Vec<u8> {
    let mut data = Vec::new();
    let mut byte = [0_u8; 1];
    while !data.ends_with(b"\r\n\r\n") {
        stream.read_exact(&mut byte).await.unwrap();
        data.push(byte[0]);
    }
    data
}

async fn read_line_string(stream: &mut BufReader<TcpStream>) -> String {
    let mut line = String::new();
    stream.read_line(&mut line).await.unwrap();
    line
}

async fn read_duplex_line(stream: &mut tokio::io::DuplexStream) -> String {
    let mut line = Vec::new();
    let mut byte = [0_u8; 1];
    loop {
        stream.read_exact(&mut byte).await.unwrap();
        line.push(byte[0]);
        if byte[0] == b'\n' {
            return String::from_utf8(line).unwrap();
        }
    }
}

async fn read_smtp_reply(stream: &mut tokio::io::DuplexStream) {
    loop {
        let line = read_duplex_line(stream).await;
        if line.as_bytes().get(3) != Some(&b'-') {
            break;
        }
    }
}
