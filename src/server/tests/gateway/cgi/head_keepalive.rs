use super::*;

#[tokio::test]
async fn head_responses_do_not_return_fastcgi_connections_to_the_pool() {
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
                    listen 127.0.0.1:18146;
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
    let request = plain_request("/", "127.0.0.1:18146");
    let mut variables = Variables::new(&request, "localhost", server, &local(18146), &remote());
    let env = test_env();

    let server_task = tokio::spawn(async move {
        let (mut first, _) = listener.accept().await.unwrap();
        read_fastcgi_request_body(&mut first).await;
        write_fastcgi_response(&mut first, "first-body").await;

        let (mut second, _) = tokio::time::timeout(Duration::from_secs(1), listener.accept())
            .await
            .expect("HEAD must not return its FastCGI connection to the pool")
            .unwrap();
        read_fastcgi_request_body(&mut second).await;
        write_fastcgi_response(&mut second, "second-body").await;
    });

    let mut head = request.clone();
    head.method = "HEAD".into();
    let first = fastcgi_request(
        context.fastcgi_pass.as_ref().unwrap(),
        config.http.as_ref().unwrap(),
        &head,
        &env,
        context,
        &mut variables,
    )
    .await;
    assert_eq!(first.status, 200);

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
    assert_eq!(second.into_bytes(), b"second-body");
    server_task.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

async fn read_fastcgi_request_body<R: AsyncRead + Unpin>(reader: &mut R) {
    loop {
        let record = read_fastcgi_record(reader).await.unwrap();
        if record.record_type == FCGI_STDIN && record.content.is_empty() {
            return;
        }
    }
}

async fn write_fastcgi_response<W: AsyncWrite + Unpin>(writer: &mut W, body: &str) {
    let stdout = format!(
        "Status: 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    write_fastcgi_record(writer, FCGI_STDOUT, stdout.as_bytes())
        .await
        .unwrap();
    write_fastcgi_record(writer, FCGI_END_REQUEST, &[0; 8])
        .await
        .unwrap();
    writer.flush().await.unwrap();
}
