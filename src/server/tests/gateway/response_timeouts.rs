use super::*;

const TRICKLE_DELAY: Duration = Duration::from_millis(20);
const READ_TIMEOUT: &str = "80ms";
const BODY: &[u8] = b"abcdefgh";

const RESPONSE_HEAD_CHUNKS: &[&[u8]] = &[
    b"Status: 200 OK\r\n",
    b"Content-Type: text/plain\r\n",
    b"Content-Length: 8\r\n",
    b"Cache-Control: no-store\r\n",
    b"X-Trickle: yes\r\n",
    b"\r\n",
];

const RESPONSE_HEAD: &[u8] =
    b"Status: 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 2\r\n\r\n";

#[derive(Clone, Copy)]
enum CgiGateway {
    Uwsgi,
    Scgi,
}

#[tokio::test]
async fn uwsgi_read_timeout_tracks_progress_across_response_head() {
    let (prefix, config) =
        gateway_timeout_config(18139, "uwsgi_pass 127.0.0.1:9000; uwsgi_buffering off;");
    let server = &config.http.as_ref().unwrap().servers[0];
    let context = server.locations[0].context.clone();
    let request = plain_request("/", "127.0.0.1:18139");
    let variables = Variables::new(&request, "localhost", server, &remote(), &local(18139));
    let (client, mut backend) = tokio::io::duplex(64 * 1024);
    let backend_task = tokio::spawn(async move {
        read_uwsgi_header(&mut backend).await;
        write_chunks_trickled(&mut backend, RESPONSE_HEAD_CHUNKS).await;
        backend.write_all(BODY).await.unwrap();
        backend.flush().await.unwrap();
    });

    let response = tokio::time::timeout(
        Duration::from_secs(2),
        uwsgi_exchange(client, &request, &context, &variables),
    )
    .await
    .expect("uWSGI header trickle should not hang")
    .unwrap();
    assert_eq!(response.status, 200);
    assert_eq!(
        response
            .body
            .into_memory(MAX_BODY_BYTES as u64)
            .await
            .unwrap(),
        BODY
    );
    backend_task.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn uwsgi_read_timeout_tracks_progress_across_response_body() {
    let (prefix, config) =
        gateway_timeout_config(18140, "uwsgi_pass 127.0.0.1:9000; uwsgi_buffering off;");
    let server = &config.http.as_ref().unwrap().servers[0];
    let context = server.locations[0].context.clone();
    let request = plain_request("/", "127.0.0.1:18140");
    let variables = Variables::new(&request, "localhost", server, &remote(), &local(18140));
    let (client, mut backend) = tokio::io::duplex(64 * 1024);
    let backend_task = tokio::spawn(async move {
        read_uwsgi_header(&mut backend).await;
        write_chunks_trickled(&mut backend, RESPONSE_HEAD_CHUNKS).await;
        write_bytes_trickled(&mut backend, BODY).await;
    });

    let response = tokio::time::timeout(
        Duration::from_secs(2),
        uwsgi_exchange(client, &request, &context, &variables),
    )
    .await
    .expect("uWSGI body trickle should not hang")
    .unwrap();
    assert_eq!(response.status, 200);
    assert_eq!(
        response
            .body
            .into_memory(MAX_BODY_BYTES as u64)
            .await
            .unwrap(),
        BODY
    );
    backend_task.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn scgi_read_timeout_tracks_progress_across_response_head() {
    let (prefix, config) =
        gateway_timeout_config(18141, "scgi_pass 127.0.0.1:9002; scgi_buffering off;");
    let server = &config.http.as_ref().unwrap().servers[0];
    let context = server.locations[0].context.clone();
    let request = plain_request("/", "127.0.0.1:18141");
    let variables = Variables::new(&request, "localhost", server, &remote(), &local(18141));
    let (client, mut backend) = tokio::io::duplex(64 * 1024);
    let backend_task = tokio::spawn(async move {
        read_scgi_header(&mut backend).await;
        write_chunks_trickled(&mut backend, RESPONSE_HEAD_CHUNKS).await;
        backend.write_all(BODY).await.unwrap();
        backend.flush().await.unwrap();
    });

    let response = tokio::time::timeout(
        Duration::from_secs(2),
        scgi_exchange(client, &request, &context, &variables),
    )
    .await
    .expect("SCGI header trickle should not hang")
    .unwrap();
    assert_eq!(response.status, 200);
    assert_eq!(
        response
            .body
            .into_memory(MAX_BODY_BYTES as u64)
            .await
            .unwrap(),
        BODY
    );
    backend_task.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn scgi_read_timeout_tracks_progress_across_response_body() {
    let (prefix, config) =
        gateway_timeout_config(18142, "scgi_pass 127.0.0.1:9002; scgi_buffering off;");
    let server = &config.http.as_ref().unwrap().servers[0];
    let context = server.locations[0].context.clone();
    let request = plain_request("/", "127.0.0.1:18142");
    let variables = Variables::new(&request, "localhost", server, &remote(), &local(18142));
    let (client, mut backend) = tokio::io::duplex(64 * 1024);
    let backend_task = tokio::spawn(async move {
        read_scgi_header(&mut backend).await;
        write_chunks_trickled(&mut backend, RESPONSE_HEAD_CHUNKS).await;
        write_bytes_trickled(&mut backend, BODY).await;
    });

    let response = tokio::time::timeout(
        Duration::from_secs(2),
        scgi_exchange(client, &request, &context, &variables),
    )
    .await
    .expect("SCGI body trickle should not hang")
    .unwrap();
    assert_eq!(response.status, 200);
    assert_eq!(
        response
            .body
            .into_memory(MAX_BODY_BYTES as u64)
            .await
            .unwrap(),
        BODY
    );
    backend_task.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn uwsgi_read_timeout_expires_when_response_head_stalls() {
    let (prefix, config) =
        gateway_timeout_config(18143, "uwsgi_pass 127.0.0.1:9000; uwsgi_buffering off;");
    let server = &config.http.as_ref().unwrap().servers[0];
    let context = server.locations[0].context.clone();
    let request = plain_request("/", "127.0.0.1:18143");
    let variables = Variables::new(&request, "localhost", server, &remote(), &local(18143));
    let (client, mut backend) = tokio::io::duplex(64 * 1024);
    let backend_task = tokio::spawn(async move {
        read_uwsgi_header(&mut backend).await;
        tokio::time::sleep(Duration::from_millis(250)).await;
    });

    let result = tokio::time::timeout(
        Duration::from_secs(2),
        uwsgi_exchange(client, &request, &context, &variables),
    )
    .await
    .expect("uWSGI read timeout should not hang");
    assert!(matches!(result, Err(ProxyFailure::Timeout)));
    backend_task.abort();
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn scgi_read_timeout_expires_when_response_body_stalls() {
    let (prefix, config) =
        gateway_timeout_config(18144, "scgi_pass 127.0.0.1:9002; scgi_buffering off;");
    let server = &config.http.as_ref().unwrap().servers[0];
    let context = server.locations[0].context.clone();
    let request = plain_request("/", "127.0.0.1:18144");
    let variables = Variables::new(&request, "localhost", server, &remote(), &local(18144));
    let (client, mut backend) = tokio::io::duplex(64 * 1024);
    let backend_task = tokio::spawn(async move {
        read_scgi_header(&mut backend).await;
        backend.write_all(RESPONSE_HEAD).await.unwrap();
        backend.write_all(b"a").await.unwrap();
        backend.flush().await.unwrap();
        tokio::time::sleep(Duration::from_millis(250)).await;
    });

    let response = tokio::time::timeout(
        Duration::from_secs(2),
        scgi_exchange(client, &request, &context, &variables),
    )
    .await
    .expect("SCGI response head should arrive")
    .unwrap();
    assert_eq!(response.status, 200);
    let ResponseBody::Stream(mut stream) = response.body else {
        panic!("SCGI response body should be streamed");
    };
    let first = tokio::time::timeout(Duration::from_secs(1), stream.next())
        .await
        .expect("SCGI first body byte should not hang")
        .unwrap()
        .unwrap();
    let ResponseStreamItem::Data(data) = first else {
        panic!("SCGI first stream item should be data");
    };
    assert_eq!(&data[..], b"a");
    let failure = tokio::time::timeout(Duration::from_secs(1), stream.next())
        .await
        .expect("SCGI body timeout should not hang")
        .expect_err("stalled SCGI body should time out");
    assert_eq!(failure.kind(), io::ErrorKind::TimedOut);
    backend_task.abort();
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn uwsgi_read_timeout_expires_between_response_head_and_body() {
    response_head_body_gap_times_out(CgiGateway::Uwsgi, 18160).await;
}

#[tokio::test]
async fn scgi_read_timeout_expires_between_response_head_and_body() {
    response_head_body_gap_times_out(CgiGateway::Scgi, 18161).await;
}

async fn response_head_body_gap_times_out(gateway: CgiGateway, port: u16) {
    let gateway_directive = match gateway {
        CgiGateway::Uwsgi => "uwsgi_pass 127.0.0.1:9000; uwsgi_buffering off;",
        CgiGateway::Scgi => "scgi_pass 127.0.0.1:9002; scgi_buffering off;",
    };
    let (prefix, config) = gateway_timeout_config(port, gateway_directive);
    let server = &config.http.as_ref().unwrap().servers[0];
    let context = server.locations[0].context.clone();
    let request = plain_request("/", &format!("127.0.0.1:{port}"));
    let variables = Variables::new(&request, "localhost", server, &remote(), &local(port));
    let (client, mut backend) = tokio::io::duplex(64 * 1024);
    let backend_task = tokio::spawn(async move {
        match gateway {
            CgiGateway::Uwsgi => read_uwsgi_header(&mut backend).await,
            CgiGateway::Scgi => read_scgi_header(&mut backend).await,
        }
        backend.write_all(RESPONSE_HEAD).await.unwrap();
        backend.flush().await.unwrap();
        tokio::time::sleep(Duration::from_millis(250)).await;
    });
    let exchange = async move {
        match gateway {
            CgiGateway::Uwsgi => uwsgi_exchange(client, &request, &context, &variables).await,
            CgiGateway::Scgi => scgi_exchange(client, &request, &context, &variables).await,
        }
    };
    let response = tokio::time::timeout(Duration::from_secs(1), exchange)
        .await
        .expect("response head should arrive before the body gap")
        .unwrap();
    assert_eq!(response.status, 200);
    let ResponseBody::Stream(mut stream) = response.body else {
        panic!("gateway response body should be streamed");
    };
    let failure = tokio::time::timeout(Duration::from_secs(1), stream.next())
        .await
        .expect("response body timeout should not hang")
        .expect_err("a body gap longer than read timeout should fail");
    assert_eq!(failure.kind(), io::ErrorKind::TimedOut);
    backend_task.abort();
    let _ = std::fs::remove_dir_all(prefix);
}

fn gateway_timeout_config(port: u16, gateway: &str) -> (PathBuf, Arc<RuntimeConfig>) {
    test_config(
        &format!(
            r#"
                worker_processes 1;
                events {{ worker_connections 64; }}
                http {{
                    server {{
                        listen 127.0.0.1:{port};
                        location / {{
                            {gateway}
                            uwsgi_read_timeout {READ_TIMEOUT};
                            scgi_read_timeout {READ_TIMEOUT};
                        }}
                    }}
                }}
            "#
        ),
        &[],
    )
}

async fn write_chunks_trickled<W>(writer: &mut W, chunks: &[&[u8]])
where
    W: AsyncWrite + Unpin,
{
    for chunk in chunks {
        writer.write_all(chunk).await.unwrap();
        writer.flush().await.unwrap();
        tokio::time::sleep(TRICKLE_DELAY).await;
    }
}

async fn write_bytes_trickled<W>(writer: &mut W, bytes: &[u8])
where
    W: AsyncWrite + Unpin,
{
    for byte in bytes {
        writer.write_all(&[*byte]).await.unwrap();
        writer.flush().await.unwrap();
        tokio::time::sleep(TRICKLE_DELAY).await;
    }
}
