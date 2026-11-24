use super::*;

#[tokio::test]
async fn reuses_uwsgi_connections_with_named_upstream_keepalive() {
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
                    listen 127.0.0.1:18170;
                    location / {{
                        uwsgi_pass backend;
                        uwsgi_param REQUEST_METHOD $request_method;
                    }}
                }}
            }}
        "#
    );
    let (prefix, config) = test_config(&source, &[]);
    let http = config.http.as_ref().unwrap();
    let server = &http.servers[0];
    let context = &server.locations[0].context;
    let request = plain_request("/", "127.0.0.1:18170");
    let mut variables = Variables::new(&request, "localhost", server, &local(18170), &remote());

    let backend = tokio::spawn(async move {
        let (mut connection, _) = listener.accept().await.unwrap();
        for body in ["first", "second"] {
            read_uwsgi_header(&mut connection).await;
            write_cgi_response(&mut connection, body).await;
        }
        assert!(
            tokio::time::timeout(Duration::from_millis(100), listener.accept())
                .await
                .is_err(),
            "uWSGI keepalive should reuse the named upstream connection"
        );
    });

    for body in ["first", "second"] {
        let response = uwsgi_request(
            context.uwsgi_pass.as_ref().unwrap(),
            http,
            &request,
            context,
            &mut variables,
        )
        .await;
        assert_eq!(response.status, 200);
        assert_eq!(response.into_bytes(), body.as_bytes());
    }
    backend.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn reuses_scgi_connections_with_named_upstream_keepalive() {
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
                    listen 127.0.0.1:18171;
                    location / {{
                        scgi_pass backend;
                        scgi_param REQUEST_METHOD $request_method;
                    }}
                }}
            }}
        "#
    );
    let (prefix, config) = test_config(&source, &[]);
    let http = config.http.as_ref().unwrap();
    let server = &http.servers[0];
    let context = &server.locations[0].context;
    let request = plain_request("/", "127.0.0.1:18171");
    let mut variables = Variables::new(&request, "localhost", server, &local(18171), &remote());

    let backend = tokio::spawn(async move {
        let (mut connection, _) = listener.accept().await.unwrap();
        for body in ["first", "second"] {
            read_scgi_request_header(&mut connection).await;
            write_cgi_response(&mut connection, body).await;
        }
        assert!(
            tokio::time::timeout(Duration::from_millis(100), listener.accept())
                .await
                .is_err(),
            "SCGI keepalive should reuse the named upstream connection"
        );
    });

    for body in ["first", "second"] {
        let response = scgi_request(
            context.scgi_pass.as_ref().unwrap(),
            http,
            &request,
            context,
            &mut variables,
        )
        .await;
        assert_eq!(response.status, 200);
        assert_eq!(response.into_bytes(), body.as_bytes());
    }
    backend.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn uwsgi_response_without_content_length_is_not_pooled() {
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
                    listen 127.0.0.1:18172;
                    location / {{
                        uwsgi_pass backend;
                        uwsgi_param REQUEST_METHOD $request_method;
                    }}
                }}
            }}
        "#
    );
    let (prefix, config) = test_config(&source, &[]);
    let http = config.http.as_ref().unwrap();
    let server = &http.servers[0];
    let context = &server.locations[0].context;
    let request = plain_request("/", "127.0.0.1:18172");
    let mut variables = Variables::new(&request, "localhost", server, &local(18172), &remote());

    let backend = tokio::spawn(async move {
        let (mut first, _) = listener.accept().await.unwrap();
        read_uwsgi_header(&mut first).await;
        first
            .write_all(b"Status: 200 OK\r\nContent-Type: text/plain\r\n\r\nfirst")
            .await
            .unwrap();
        first.shutdown().await.unwrap();

        let (mut second, _) = tokio::time::timeout(Duration::from_secs(1), listener.accept())
            .await
            .expect("a response delimited by EOF must not be pooled")
            .unwrap();
        read_uwsgi_header(&mut second).await;
        write_cgi_response(&mut second, "second").await;
    });

    for body in ["first", "second"] {
        let response = uwsgi_request(
            context.uwsgi_pass.as_ref().unwrap(),
            http,
            &request,
            context,
            &mut variables,
        )
        .await;
        assert_eq!(response.status, 200);
        assert_eq!(response.into_bytes(), body.as_bytes());
    }
    backend.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

async fn write_cgi_response<W>(writer: &mut W, body: &str)
where
    W: AsyncWrite + Unpin,
{
    writer
        .write_all(
            format!(
                "Status: 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            )
            .as_bytes(),
        )
        .await
        .unwrap();
    writer.flush().await.unwrap();
}

async fn read_scgi_request_header<R>(reader: &mut R)
where
    R: AsyncRead + Unpin,
{
    read_scgi_header(reader).await;
}
