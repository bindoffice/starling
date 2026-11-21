use super::framing::{consume_fastcgi_stdin, fastcgi_result, scgi_result, uwsgi_result};
use super::*;

#[tokio::test]
async fn fastcgi_end_request_before_content_length_errors_midstream() {
    let (client, mut backend) = tokio::io::duplex(64 * 1024);
    let backend_task = tokio::spawn(async move {
        consume_fastcgi_stdin(&mut backend).await;
        write_fastcgi_record(
            &mut backend,
            FCGI_STDOUT,
            b"Status: 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 4\r\n\r\nab",
        )
        .await
        .unwrap();
        write_fastcgi_record(&mut backend, FCGI_END_REQUEST, &[0; 8])
            .await
            .unwrap();
        backend.shutdown().await.unwrap();
    });

    let response = fastcgi_result(18172, client).await.unwrap();
    let error = expect_prefix_then_error(response, b"ab").await;
    assert_eq!(error.kind(), io::ErrorKind::UnexpectedEof);
    backend_task.await.unwrap();
}

#[tokio::test]
async fn fastcgi_truncated_stdout_record_after_body_errors_midstream() {
    let (client, mut backend) = tokio::io::duplex(64 * 1024);
    let backend_task = tokio::spawn(async move {
        consume_fastcgi_stdin(&mut backend).await;
        write_fastcgi_record(
            &mut backend,
            FCGI_STDOUT,
            b"Status: 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 4\r\n\r\nab",
        )
        .await
        .unwrap();
        write_fastcgi_record_header(&mut backend, 1, FCGI_STDOUT, 1, 4).await;
        backend.write_all(b"cd").await.unwrap();
        backend.shutdown().await.unwrap();
    });

    let response = fastcgi_result(18173, client).await.unwrap();
    let error = expect_prefix_then_error(response, b"ab").await;
    assert_eq!(error.kind(), io::ErrorKind::UnexpectedEof);
    backend_task.await.unwrap();
}

#[tokio::test]
async fn fastcgi_wrong_version_after_body_errors_midstream() {
    let (client, mut backend) = tokio::io::duplex(64 * 1024);
    let backend_task = tokio::spawn(async move {
        consume_fastcgi_stdin(&mut backend).await;
        write_fastcgi_record(
            &mut backend,
            FCGI_STDOUT,
            b"Status: 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 4\r\n\r\nab",
        )
        .await
        .unwrap();
        write_fastcgi_record_header(&mut backend, 2, FCGI_STDOUT, 1, 0).await;
        backend.shutdown().await.unwrap();
    });

    let response = fastcgi_result(18174, client).await.unwrap();
    let error = expect_prefix_then_error(response, b"ab").await;
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    backend_task.await.unwrap();
}

#[tokio::test]
async fn fastcgi_wrong_request_id_after_body_errors_midstream() {
    let (client, mut backend) = tokio::io::duplex(64 * 1024);
    let backend_task = tokio::spawn(async move {
        consume_fastcgi_stdin(&mut backend).await;
        write_fastcgi_record(
            &mut backend,
            FCGI_STDOUT,
            b"Status: 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 4\r\n\r\nab",
        )
        .await
        .unwrap();
        write_fastcgi_record_header(&mut backend, 1, FCGI_STDOUT, 2, 0).await;
        backend.shutdown().await.unwrap();
    });

    let response = fastcgi_result(18175, client).await.unwrap();
    let error = expect_prefix_then_error(response, b"ab").await;
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    backend_task.await.unwrap();
}

#[tokio::test]
async fn uwsgi_content_length_eof_after_body_errors_midstream() {
    let (client, mut backend) = tokio::io::duplex(64 * 1024);
    let backend_task = tokio::spawn(async move {
        read_uwsgi_header(&mut backend).await;
        backend
            .write_all(b"Status: 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 4\r\n\r\nab")
            .await
            .unwrap();
        backend.shutdown().await.unwrap();
    });

    let response = uwsgi_result(18176, client).await.unwrap();
    let error = expect_prefix_then_error(response, b"ab").await;
    assert_eq!(error.kind(), io::ErrorKind::UnexpectedEof);
    backend_task.await.unwrap();
}

#[tokio::test]
async fn scgi_content_length_eof_after_body_errors_midstream() {
    let (client, mut backend) = tokio::io::duplex(64 * 1024);
    let backend_task = tokio::spawn(async move {
        read_scgi_header(&mut backend).await;
        backend
            .write_all(b"Status: 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 4\r\n\r\nab")
            .await
            .unwrap();
        backend.shutdown().await.unwrap();
    });

    let response = scgi_result(18177, client).await.unwrap();
    let error = expect_prefix_then_error(response, b"ab").await;
    assert_eq!(error.kind(), io::ErrorKind::UnexpectedEof);
    backend_task.await.unwrap();
}

#[tokio::test]
async fn uwsgi_excess_body_is_truncated_at_content_length() {
    let (client, mut backend) = tokio::io::duplex(64 * 1024);
    let backend_task = tokio::spawn(async move {
        read_uwsgi_header(&mut backend).await;
        backend
            .write_all(
                b"Status: 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 2\r\n\r\nabextra",
            )
            .await
            .unwrap();
        backend.shutdown().await.unwrap();
    });

    let response = uwsgi_result(18178, client).await.unwrap();
    assert_eq!(expect_stream_body(response).await, b"ab");
    backend_task.await.unwrap();
}

#[tokio::test]
async fn scgi_excess_body_is_truncated_at_content_length() {
    let (client, mut backend) = tokio::io::duplex(64 * 1024);
    let backend_task = tokio::spawn(async move {
        read_scgi_header(&mut backend).await;
        backend
            .write_all(
                b"Status: 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 2\r\n\r\nabextra",
            )
            .await
            .unwrap();
        backend.shutdown().await.unwrap();
    });

    let response = scgi_result(18179, client).await.unwrap();
    assert_eq!(expect_stream_body(response).await, b"ab");
    backend_task.await.unwrap();
}

#[tokio::test]
async fn uwsgi_complete_buffer_stops_at_content_length_without_upstream_eof() {
    let (client, mut backend) = tokio::io::duplex(64 * 1024);
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    let backend_task = tokio::spawn(async move {
        read_uwsgi_header(&mut backend).await;
        backend
            .write_all(
                b"Status: 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 4\r\n\r\nabcdef",
            )
            .await
            .unwrap();
        backend.flush().await.unwrap();
        release_rx.await.unwrap();
        backend.shutdown().await.unwrap();
    });

    let response =
        tokio::time::timeout(Duration::from_secs(1), uwsgi_buffered_result(18180, client))
            .await
            .expect("complete-buffered gateway waited for upstream EOF after Content-Length")
            .unwrap();
    assert_eq!(response.into_bytes(), b"abcd");
    release_tx.send(()).unwrap();
    backend_task.await.unwrap();
}

#[tokio::test]
async fn scgi_complete_buffer_stops_at_content_length_without_upstream_eof() {
    let (client, mut backend) = tokio::io::duplex(64 * 1024);
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    let backend_task = tokio::spawn(async move {
        read_scgi_header(&mut backend).await;
        backend
            .write_all(
                b"Status: 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 4\r\n\r\nabcdef",
            )
            .await
            .unwrap();
        backend.flush().await.unwrap();
        release_rx.await.unwrap();
        backend.shutdown().await.unwrap();
    });

    let response =
        tokio::time::timeout(Duration::from_secs(1), scgi_buffered_result(18181, client))
            .await
            .expect("complete-buffered gateway waited for upstream EOF after Content-Length")
            .unwrap();
    assert_eq!(response.into_bytes(), b"abcd");
    release_tx.send(()).unwrap();
    backend_task.await.unwrap();
}

#[tokio::test]
async fn fastcgi_direct_stream_stops_at_content_length_without_end_request() {
    let (client, mut backend) = tokio::io::duplex(64 * 1024);
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    let backend_task = tokio::spawn(async move {
        consume_fastcgi_stdin(&mut backend).await;
        write_fastcgi_record(
            &mut backend,
            FCGI_STDOUT,
            b"Status: 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 4\r\n\r\nabcdef",
        )
        .await
        .unwrap();
        backend.flush().await.unwrap();
        release_rx.await.unwrap();
        backend.shutdown().await.unwrap();
    });

    let response = tokio::time::timeout(Duration::from_secs(1), fastcgi_result(18182, client))
        .await
        .expect("FastCGI stream waited for END_REQUEST after Content-Length")
        .unwrap();
    assert_eq!(expect_stream_body(response).await, b"abcd");
    release_tx.send(()).unwrap();
    backend_task.await.unwrap();
}

#[tokio::test]
async fn fastcgi_complete_buffer_stops_at_content_length_without_end_request() {
    let (client, mut backend) = tokio::io::duplex(64 * 1024);
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    let backend_task = tokio::spawn(async move {
        consume_fastcgi_stdin(&mut backend).await;
        write_fastcgi_record(
            &mut backend,
            FCGI_STDOUT,
            b"Status: 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 4\r\n\r\nabcdef",
        )
        .await
        .unwrap();
        backend.flush().await.unwrap();
        release_rx.await.unwrap();
        backend.shutdown().await.unwrap();
    });

    let response = tokio::time::timeout(
        Duration::from_secs(1),
        fastcgi_buffered_result(18183, client),
    )
    .await
    .expect("complete-buffered FastCGI waited for END_REQUEST after Content-Length")
    .unwrap();
    assert_eq!(response.into_bytes(), b"abcd");
    release_tx.send(()).unwrap();
    backend_task.await.unwrap();
}

async fn uwsgi_buffered_result<S>(port: u16, stream: S) -> Result<Response, ProxyFailure>
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

async fn scgi_buffered_result<S>(port: u16, stream: S) -> Result<Response, ProxyFailure>
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

async fn fastcgi_buffered_result<S>(port: u16, stream: S) -> Result<Response, ProxyFailure>
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

async fn write_fastcgi_record_header<W: AsyncWrite + Unpin>(
    writer: &mut W,
    version: u8,
    record_type: u8,
    request_id: u16,
    content_length: u16,
) {
    let request_id = request_id.to_be_bytes();
    let content_length = content_length.to_be_bytes();
    writer
        .write_all(&[
            version,
            record_type,
            request_id[0],
            request_id[1],
            content_length[0],
            content_length[1],
            0,
            0,
        ])
        .await
        .unwrap();
}

async fn expect_prefix_then_error(response: Response, expected: &[u8]) -> io::Error {
    let ResponseBody::Stream(mut stream) = response.body else {
        panic!("gateway response should be streamed");
    };
    let mut body = Vec::new();
    loop {
        match next_stream_item(&mut stream).await {
            Ok(Some(ResponseStreamItem::Data(data))) => body.extend_from_slice(&data),
            Ok(Some(ResponseStreamItem::Trailers(_))) => panic!("unexpected trailers"),
            Ok(None) => panic!("stream ended after {:?}, expected an error", body),
            Err(error) => {
                assert_eq!(body, expected);
                return error;
            }
        }
    }
}

async fn expect_stream_body(response: Response) -> Vec<u8> {
    let ResponseBody::Stream(mut stream) = response.body else {
        panic!("gateway response should be streamed");
    };
    let mut body = Vec::new();
    loop {
        match next_stream_item(&mut stream).await {
            Ok(Some(ResponseStreamItem::Data(data))) => body.extend_from_slice(&data),
            Ok(Some(ResponseStreamItem::Trailers(_))) => panic!("unexpected trailers"),
            Ok(None) => return body,
            Err(error) => panic!("unexpected stream error: {error}"),
        }
    }
}

async fn next_stream_item(stream: &mut ResponseStream) -> io::Result<Option<ResponseStreamItem>> {
    tokio::time::timeout(Duration::from_secs(1), stream.next())
        .await
        .expect("streamed body timed out")
}
