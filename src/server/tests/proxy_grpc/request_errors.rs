use super::support::*;
use super::*;
use crate::http::RequestBodyStream;

#[tokio::test]
async fn truncated_file_backed_request_body_maps_to_500_without_retry() {
    let Some(upstream) = bind_grpc_listener("gRPC truncated request body upstream").await else {
        return;
    };
    let upstream_address = upstream.local_addr().unwrap();
    let backend = tokio::spawn(async move {
        let (stream, _) = upstream.accept().await.unwrap();
        let mut connection = h2::server::handshake(stream).await.unwrap();
        let (request, _respond) = connection.accept().await.unwrap().unwrap();
        let work = async move {
            let mut body = request.into_body();
            let mut received = Vec::new();
            while let Some(chunk) = body.data().await {
                match chunk {
                    Ok(chunk) => received.extend_from_slice(&chunk),
                    Err(_) => break,
                }
            }
            received
        };
        tokio::pin!(work);
        loop {
            tokio::select! {
                received = &mut work => break received,
                accepted = connection.accept() => {
                    if accepted.is_none() {
                        break work.await;
                    }
                }
            }
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
                        listen 127.0.0.1:18597;
                        location / {{
                            grpc_pass {upstream_address};
                        }}
                    }}
                }}
            "#
        ),
        &[],
    );
    let http = config.http.as_ref().unwrap();
    let server = &http.servers[0];
    let context = &server.locations[0].context;
    let (sender, stream) = RequestBodyStream::channel();
    let mut request = Request {
        method: "POST".into(),
        target: "/pkg.Service/Call".into(),
        version: "HTTP/1.1".into(),
        headers: vec![
            Header {
                name: "Host".into(),
                value: "localhost".into(),
            },
            Header {
                name: "Content-Length".into(),
                value: "8".into(),
            },
        ],
        body_stream: Some(stream),
        body: Vec::new(),
        remote_addr: "127.0.0.1:45678".into(),
        local_addr: "127.0.0.1:18597".into(),
        keep_alive: false,
    };
    request.headers.push(Header {
        name: "Content-Type".into(),
        value: "application/grpc".into(),
    });
    request.headers.push(Header {
        name: "TE".into(),
        value: "trailers".into(),
    });
    sender.send(Bytes::from_static(b"part")).await;
    sender.send(Bytes::from_static(b"more")).await;
    drop(sender);
    let remote: SocketAddr = "127.0.0.1:45678".parse().unwrap();
    let local: SocketAddr = "127.0.0.1:18597".parse().unwrap();
    let mut variables = Variables::new(&request, "localhost", server, &local, &remote);
    prepare_request_body(&mut request, context, &mut variables)
        .await
        .unwrap();
    let file = request.body_file().expect("file-backed request body");
    std::fs::write(file.path(), b"part").unwrap();

    let response = tokio::time::timeout(
        Duration::from_secs(2),
        grpc_request(
            context.grpc.pass.as_ref().unwrap(),
            http,
            &request,
            &RequestEnv {
                uri: "/pkg.Service/Call".into(),
                args: None,
                unparsed_uri: Some("/pkg.Service/Call".into()),
                valid_location: true,
                internal: false,
                bypass_try_files: false,
                add_uri_to_alias: false,
                limits_checked: false,
                auth_request_checked: false,
                mirror_checked: false,
            },
            context,
            &mut variables,
        ),
    )
    .await
    .expect("truncated gRPC request body should not hang");
    assert_eq!(response.status, 500);
    let received = tokio::time::timeout(Duration::from_secs(2), backend)
        .await
        .expect("gRPC backend should observe the truncated request body")
        .unwrap();
    assert_eq!(received, b"part");

    drop(file);
    drop(variables);
    drop(request);
    assert!(
        !contains_generated_temp_file(&prefix.join("body_temp")),
        "truncated file-backed gRPC request body should be removed"
    );
    let _ = std::fs::remove_dir_all(prefix);
}
