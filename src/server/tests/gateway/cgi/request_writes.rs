use super::*;

#[derive(Default)]
struct RecordingWriter {
    bytes: Vec<u8>,
    writes: usize,
    first_write: Vec<u8>,
}

#[derive(Clone)]
struct SharedWriter {
    first_write: std::sync::Arc<tokio::sync::Semaphore>,
    writes: std::sync::Arc<std::sync::Mutex<Vec<Vec<u8>>>>,
    signaled: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl SharedWriter {
    fn new() -> Self {
        Self {
            first_write: std::sync::Arc::new(tokio::sync::Semaphore::new(0)),
            writes: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
            signaled: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }
}

impl AsyncWrite for RecordingWriter {
    fn poll_write(
        mut self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
        buffer: &[u8],
    ) -> std::task::Poll<io::Result<usize>> {
        self.bytes.extend_from_slice(buffer);
        if self.writes == 0 {
            self.first_write.extend_from_slice(buffer);
        }
        self.writes += 1;
        std::task::Poll::Ready(Ok(buffer.len()))
    }

    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        std::task::Poll::Ready(Ok(()))
    }

    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        std::task::Poll::Ready(Ok(()))
    }
}

impl AsyncWrite for SharedWriter {
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
        buffer: &[u8],
    ) -> std::task::Poll<io::Result<usize>> {
        self.writes.lock().unwrap().push(buffer.to_vec());
        if !self
            .signaled
            .swap(true, std::sync::atomic::Ordering::AcqRel)
        {
            self.first_write.add_permits(1);
        }
        std::task::Poll::Ready(Ok(buffer.len()))
    }

    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        std::task::Poll::Ready(Ok(()))
    }

    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        std::task::Poll::Ready(Ok(()))
    }
}

#[tokio::test]
async fn small_buffered_fastcgi_requests_are_written_as_one_record_batch() {
    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            http {
                server {
                    listen 127.0.0.1:18145;
                    location / {
                        fastcgi_pass 127.0.0.1:9000;
                        fastcgi_param REQUEST_URI $request_uri;
                    }
                }
            }
        "#,
        &[],
    );
    let server = &config.http.as_ref().unwrap().servers[0];
    let context = &server.locations[0].context;
    let mut request = plain_request("/", "127.0.0.1:18145");
    request.method = "POST".into();
    Arc::make_mut(&mut request.headers).push(Header {
        name: "Content-Length".into(),
        value: "6".into(),
    });
    request.body = b"foobar".to_vec();
    let variables = Variables::new(&request, "localhost", server, &local(18145), &remote());
    let env = test_env();
    let mut writer = RecordingWriter::default();

    write_fastcgi_request_with_stream(
        &mut writer,
        &request,
        &env,
        context,
        &variables,
        FastCgiRequestBody {
            stream: None,
            limit: 1024,
            send_timeout: Duration::from_secs(1),
        },
    )
    .await
    .unwrap();

    assert_eq!(writer.writes, 1);
    assert!(writer.bytes.windows(6).any(|window| window == b"foobar"));
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn queued_preread_body_is_written_with_the_fastcgi_header_batch() {
    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            http {
                server {
                    listen 127.0.0.1:18146;
                    location / {
                        fastcgi_pass 127.0.0.1:9000;
                        fastcgi_param REQUEST_URI $request_uri;
                    }
                }
            }
        "#,
        &[],
    );
    let server = &config.http.as_ref().unwrap().servers[0];
    let context = &server.locations[0].context;
    let (sender, stream) = crate::http::RequestBodyStream::channel();
    assert!(sender.try_send(bytes::Bytes::from_static(b"01234")));
    drop(sender);
    let mut request = plain_request("/", "127.0.0.1:18146");
    request.method = "POST".into();
    Arc::make_mut(&mut request.headers).push(Header {
        name: "Content-Length".into(),
        value: "5".into(),
    });
    request.body_stream = Some(stream.clone());
    let variables = Variables::new(&request, "localhost", server, &local(18146), &remote());
    let env = test_env();
    let mut writer = RecordingWriter::default();

    write_fastcgi_request_with_stream(
        &mut writer,
        &request,
        &env,
        context,
        &variables,
        FastCgiRequestBody {
            stream: Some(&stream),
            limit: 1024,
            send_timeout: Duration::from_secs(1),
        },
    )
    .await
    .unwrap();

    assert_eq!(writer.writes, 2);
    assert_eq!(
        fastcgi_stdin_records(&writer.first_write),
        vec![b"01234".to_vec()]
    );
    assert_eq!(
        fastcgi_stdin_records(&writer.bytes),
        vec![b"01234".to_vec(), Vec::new()]
    );
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn final_streaming_body_chunk_and_terminal_record_share_a_write() {
    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            http {
                server {
                    listen 127.0.0.1:18148;
                    location / {
                        fastcgi_pass 127.0.0.1:9000;
                        fastcgi_param REQUEST_URI $request_uri;
                    }
                }
            }
        "#,
        &[],
    );
    let server = &config.http.as_ref().unwrap().servers[0];
    let context = server.locations[0].context.clone();
    let (sender, stream) = crate::http::RequestBodyStream::channel();
    let mut request = plain_request("/", "127.0.0.1:18148");
    request.method = "POST".into();
    Arc::make_mut(&mut request.headers).push(Header {
        name: "Content-Length".into(),
        value: "10".into(),
    });
    request.body_stream = Some(stream.clone());
    let variables = Variables::new(&request, "localhost", server, &local(18148), &remote());
    let env = test_env();
    let writer = SharedWriter::new();
    let task_writer = writer.clone();

    let write = tokio::spawn(async move {
        let mut writer = task_writer;
        write_fastcgi_request_with_stream(
            &mut writer,
            &request,
            &env,
            &context,
            &variables,
            FastCgiRequestBody {
                stream: Some(&stream),
                limit: 1024,
                send_timeout: Duration::from_secs(1),
            },
        )
        .await
    });

    writer.first_write.acquire().await.unwrap().forget();
    assert!(sender.send(bytes::Bytes::from_static(b"0123456789")).await);
    sender.finish();
    write.await.unwrap().unwrap();

    let writes = writer.writes.lock().unwrap();
    assert_eq!(writes.len(), 2);
    assert_eq!(
        fastcgi_stdin_records(&writes[1]),
        vec![b"0123456789".to_vec(), Vec::new()]
    );
    drop(writes);
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn file_backed_fastcgi_bodies_larger_than_one_record_are_sent_in_full() {
    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            http {
                server {
                    listen 127.0.0.1:18147;
                    location / {
                        fastcgi_pass 127.0.0.1:9000;
                        fastcgi_param REQUEST_URI $request_uri;
                    }
                }
            }
        "#,
        &[],
    );
    let server = &config.http.as_ref().unwrap().servers[0];
    let context = &server.locations[0].context;
    let body = vec![b'x'; 100 * 1024];
    let path = prefix.join("request-body");
    std::fs::write(&path, &body).unwrap();
    let file = std::sync::Arc::new(crate::http::RequestBodyFile::new(
        crate::http::TempFileGuard::new(path, false),
        body.len() as u64,
    ));
    let mut request = plain_request("/", "127.0.0.1:18147");
    request.method = "POST".into();
    Arc::make_mut(&mut request.headers).push(Header {
        name: "Content-Length".into(),
        value: body.len().to_string(),
    });
    request.body_stream = Some(crate::http::RequestBodyStream::from_file(file));
    let variables = Variables::new(&request, "localhost", server, &local(18147), &remote());
    let env = test_env();
    let mut writer = RecordingWriter::default();

    write_fastcgi_request_with_stream(
        &mut writer,
        &request,
        &env,
        context,
        &variables,
        FastCgiRequestBody {
            stream: request.body_stream.as_ref(),
            limit: 1024 * 1024,
            send_timeout: Duration::from_secs(1),
        },
    )
    .await
    .unwrap();

    let stdin = fastcgi_stdin_records(&writer.bytes);
    assert_eq!(stdin.len(), 4);
    assert!(stdin[3].is_empty());
    assert_eq!(
        stdin[..3]
            .iter()
            .flat_map(|record| record.iter())
            .copied()
            .collect::<Vec<_>>(),
        body
    );
    let _ = std::fs::remove_dir_all(prefix);
}

fn fastcgi_stdin_records(bytes: &[u8]) -> Vec<Vec<u8>> {
    let mut records = Vec::new();
    let mut position = 0;
    while position < bytes.len() {
        let header = &bytes[position..position + 8];
        let length = u16::from_be_bytes([header[4], header[5]]) as usize;
        let padding = header[6] as usize;
        let content = &bytes[position + 8..position + 8 + length];
        if header[1] == FCGI_STDIN {
            records.push(content.to_vec());
        }
        position += 8 + length + padding;
    }
    records
}
