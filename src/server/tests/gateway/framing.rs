use super::*;

#[tokio::test]
async fn uwsgi_eof_before_response_headers_is_an_invalid_header() {
    let (client, mut backend) = tokio::io::duplex(64 * 1024);
    let backend_task = tokio::spawn(async move {
        read_uwsgi_header(&mut backend).await;
        backend.shutdown().await.unwrap();
    });
    let result = uwsgi_result(18163, client).await;
    assert!(matches!(result, Err(ProxyFailure::InvalidHeader)));
    backend_task.await.unwrap();
}

#[tokio::test]
async fn scgi_eof_before_response_headers_is_an_invalid_header() {
    let (client, mut backend) = tokio::io::duplex(64 * 1024);
    let backend_task = tokio::spawn(async move {
        read_scgi_header(&mut backend).await;
        backend.shutdown().await.unwrap();
    });
    let result = scgi_result(18164, client).await;
    assert!(matches!(result, Err(ProxyFailure::InvalidHeader)));
    backend_task.await.unwrap();
}

#[tokio::test]
async fn uwsgi_truncated_response_headers_are_an_invalid_header() {
    let (client, mut backend) = tokio::io::duplex(64 * 1024);
    let backend_task = tokio::spawn(async move {
        read_uwsgi_header(&mut backend).await;
        backend
            .write_all(b"Status: 200 OK\r\nContent-Type: text/plain\r\n")
            .await
            .unwrap();
        backend.shutdown().await.unwrap();
    });
    let result = uwsgi_result(18165, client).await;
    assert!(matches!(result, Err(ProxyFailure::InvalidHeader)));
    backend_task.await.unwrap();
}

#[tokio::test]
async fn scgi_truncated_response_headers_are_an_invalid_header() {
    let (client, mut backend) = tokio::io::duplex(64 * 1024);
    let backend_task = tokio::spawn(async move {
        read_scgi_header(&mut backend).await;
        backend
            .write_all(b"Status: 200 OK\r\nContent-Type: text/plain\r\n")
            .await
            .unwrap();
        backend.shutdown().await.unwrap();
    });
    let result = scgi_result(18166, client).await;
    assert!(matches!(result, Err(ProxyFailure::InvalidHeader)));
    backend_task.await.unwrap();
}

#[tokio::test]
async fn uwsgi_invalid_status_header_is_an_invalid_header() {
    let (client, mut backend) = tokio::io::duplex(64 * 1024);
    let backend_task = tokio::spawn(async move {
        read_uwsgi_header(&mut backend).await;
        backend
            .write_all(b"Status: nope\r\nContent-Type: text/plain\r\n\r\nok")
            .await
            .unwrap();
        backend.shutdown().await.unwrap();
    });
    let result = uwsgi_result(18170, client).await;
    assert!(matches!(result, Err(ProxyFailure::InvalidHeader)));
    backend_task.await.unwrap();
}

#[tokio::test]
async fn scgi_invalid_status_header_is_an_invalid_header() {
    let (client, mut backend) = tokio::io::duplex(64 * 1024);
    let backend_task = tokio::spawn(async move {
        read_scgi_header(&mut backend).await;
        backend
            .write_all(b"Status: nope\r\nContent-Type: text/plain\r\n\r\nok")
            .await
            .unwrap();
        backend.shutdown().await.unwrap();
    });
    let result = scgi_result(18171, client).await;
    assert!(matches!(result, Err(ProxyFailure::InvalidHeader)));
    backend_task.await.unwrap();
}

#[tokio::test]
async fn fastcgi_end_request_before_stdout_is_an_invalid_header() {
    let (client, mut backend) = tokio::io::duplex(64 * 1024);
    let backend_task = tokio::spawn(async move {
        consume_fastcgi_stdin(&mut backend).await;
        write_fastcgi_record(&mut backend, FCGI_END_REQUEST, &[0; 8])
            .await
            .unwrap();
        backend.shutdown().await.unwrap();
    });
    let result = fastcgi_result(18167, client).await;
    assert!(matches!(result, Err(ProxyFailure::InvalidHeader)));
    backend_task.await.unwrap();
}

#[tokio::test]
async fn fastcgi_end_request_before_header_terminator_is_an_invalid_header() {
    let (client, mut backend) = tokio::io::duplex(64 * 1024);
    let backend_task = tokio::spawn(async move {
        consume_fastcgi_stdin(&mut backend).await;
        write_fastcgi_record(
            &mut backend,
            FCGI_STDOUT,
            b"Status: 200 OK\r\nContent-Type: text/plain\r\n",
        )
        .await
        .unwrap();
        write_fastcgi_record(&mut backend, FCGI_END_REQUEST, &[0; 8])
            .await
            .unwrap();
        backend.shutdown().await.unwrap();
    });
    let result = fastcgi_result(18168, client).await;
    assert!(matches!(result, Err(ProxyFailure::InvalidHeader)));
    backend_task.await.unwrap();
}

#[tokio::test]
async fn fastcgi_unknown_record_type_is_an_invalid_header() {
    let (client, mut backend) = tokio::io::duplex(64 * 1024);
    let backend_task = tokio::spawn(async move {
        consume_fastcgi_stdin(&mut backend).await;
        write_fastcgi_record(&mut backend, 2, b"unexpected")
            .await
            .unwrap();
        write_fastcgi_record(
            &mut backend,
            FCGI_STDOUT,
            b"Status: 200 OK\r\nContent-Type: text/plain\r\n\r\nok",
        )
        .await
        .unwrap();
        write_fastcgi_record(&mut backend, FCGI_END_REQUEST, &[0; 8])
            .await
            .unwrap();
        backend.shutdown().await.unwrap();
    });
    let result = fastcgi_result(18169, client).await;
    assert!(matches!(result, Err(ProxyFailure::InvalidHeader)));
    backend_task.await.unwrap();
}

pub(super) async fn uwsgi_result<S>(port: u16, stream: S) -> Result<Response, ProxyFailure>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let (_, config) = test_config(
        &format!(
            r#"
                worker_processes 1;
                events {{ worker_connections 64; }}
                http {{
                    server {{
                        listen 127.0.0.1:{port};
                        location / {{
                            uwsgi_pass 127.0.0.1:9000;
                            uwsgi_buffering off;
                        }}
                    }}
                }}
            "#
        ),
        &[],
    );
    let server = &config.http.as_ref().unwrap().servers[0];
    let context = server.locations[0].context.clone();
    let request = plain_request("/", &format!("127.0.0.1:{port}"));
    let variables = Variables::new(&request, "localhost", server, &remote(), &local(port));
    uwsgi_exchange(stream, &request, &context, &variables).await
}

pub(super) async fn scgi_result<S>(port: u16, stream: S) -> Result<Response, ProxyFailure>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let (_, config) = test_config(
        &format!(
            r#"
                worker_processes 1;
                events {{ worker_connections 64; }}
                http {{
                    server {{
                        listen 127.0.0.1:{port};
                        location / {{
                            scgi_pass 127.0.0.1:9002;
                            scgi_buffering off;
                        }}
                    }}
                }}
            "#
        ),
        &[],
    );
    let server = &config.http.as_ref().unwrap().servers[0];
    let context = server.locations[0].context.clone();
    let request = plain_request("/", &format!("127.0.0.1:{port}"));
    let variables = Variables::new(&request, "localhost", server, &remote(), &local(port));
    scgi_exchange(stream, &request, &context, &variables).await
}

pub(super) async fn fastcgi_result<S>(port: u16, stream: S) -> Result<Response, ProxyFailure>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let (_, config) = test_config(
        &format!(
            r#"
                worker_processes 1;
                events {{ worker_connections 64; }}
                http {{
                    server {{
                        listen 127.0.0.1:{port};
                        location / {{
                            fastcgi_pass 127.0.0.1:9000;
                            fastcgi_buffering off;
                        }}
                    }}
                }}
            "#
        ),
        &[],
    );
    let server = &config.http.as_ref().unwrap().servers[0];
    let context = server.locations[0].context.clone();
    let request = plain_request("/", &format!("127.0.0.1:{port}"));
    let variables = Variables::new(&request, "localhost", server, &remote(), &local(port));
    fastcgi_exchange(stream, &request, &test_env(), None, &context, &variables).await
}

pub(super) async fn consume_fastcgi_stdin<R: AsyncRead + Unpin>(reader: &mut R) {
    loop {
        if next_fastcgi_stdin(reader).await.is_empty() {
            return;
        }
    }
}
