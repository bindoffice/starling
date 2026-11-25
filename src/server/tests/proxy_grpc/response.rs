use super::*;

#[tokio::test]
async fn proxies_grpc_bodies_headers_and_trailers_over_http2() {
    let (client_io, server_io) = tokio::io::duplex(64 * 1024);
    let backend = tokio::spawn(async move {
        let mut connection = h2::server::handshake(server_io).await.unwrap();
        let (request, mut respond) = connection.accept().await.unwrap().unwrap();
        let work = async move {
            assert_eq!(request.method(), http::Method::POST);
            assert_eq!(request.uri().scheme_str(), Some("http"));
            assert_eq!(
                request.uri().authority().map(|value| value.as_str()),
                Some("backend.test")
            );
            assert_eq!(
                request.uri().path_and_query().unwrap().as_str(),
                "/pkg.Service/Echo?x=1"
            );
            let mut body = Vec::new();
            let mut request = request.into_body();
            while let Some(chunk) = request.data().await {
                body.extend_from_slice(&chunk.unwrap());
            }
            assert_eq!(body, b"\0\0\0\0\x05hello");

            let response = http::Response::builder()
                .status(200)
                .header("Content-Type", "application/grpc")
                .header("Date", "Thu, 01 Jan 1970 00:00:00 GMT")
                .header("Server", "hidden")
                .header("X-Hide", "hidden")
                .header("X-Pass", "visible")
                .body(())
                .unwrap();
            let mut stream = respond.send_response(response, false).unwrap();
            stream
                .send_data(Bytes::from_static(b"\0\0\0\0\x02ok"), false)
                .unwrap();
            let mut trailers = http::HeaderMap::new();
            trailers.insert("grpc-status", http::HeaderValue::from_static("0"));
            trailers.insert("grpc-message", http::HeaderValue::from_static("OK"));
            stream.send_trailers(trailers).unwrap();
        };
        tokio::pin!(work);
        loop {
            tokio::select! {
                () = &mut work => break,
                accepted = connection.accept() => {
                    if accepted.is_none() {
                        work.await;
                        break;
                    }
                }
            }
        }
        while connection.accept().await.is_some() {}
    });

    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            http {
                server {
                    listen 127.0.0.1:18571;
                    location / {
                        grpc_pass grpc://backend.test;
                        grpc_hide_header X-Hide;
                        grpc_pass_header X-Pass;
                    }
                }
            }
        "#,
        &[],
    );
    let server = &config.http.as_ref().unwrap().servers[0];
    let context = &server.locations[0].context;
    let request = Request {
        method: "POST".into(),
        target: "/pkg.Service/Echo?x=1".into(),
        version: "HTTP/1.1".into(),
        headers: (vec![Header {
            name: "Content-Type".into(),
            value: "application/grpc".into(),
        }])
        .into(),
        body_stream: None,
        body: b"\0\0\0\0\x05hello".to_vec(),
        remote_addr: "127.0.0.1:45678".into(),
        local_addr: "127.0.0.1:18571".into(),
        keep_alive: false,
    };
    let remote: SocketAddr = "127.0.0.1:45678".parse().unwrap();
    let local: SocketAddr = "127.0.0.1:18571".parse().unwrap();
    let variables = Variables::new(&request, "localhost", server, &local, &remote);
    let env = RequestEnv {
        uri: "/pkg.Service/Echo".into(),
        args: Some("x=1".into()),
        unparsed_uri: Some("/pkg.Service/Echo?x=1".into()),
        valid_location: true,
        internal: false,
        bypass_try_files: false,
        add_uri_to_alias: false,
        limits_checked: false,
        auth_request_checked: false,
        mirror_checked: false,
    };
    let outgoing =
        build_grpc_request(&request, &env, context, &variables, "backend.test", false).unwrap();
    let key = grpc_upstream_pool_key("backend.test", 80, false, context, &variables);
    let mut connection =
        ProxyV2Connection::connect(Box::new(client_io), key, Duration::from_secs(1))
            .await
            .unwrap();
    let (upstream, _) = timed_h2_exchange(
        &mut connection,
        outgoing.request,
        &outgoing.body,
        None,
        0,
        H2ExchangeOptions {
            send_timeout: Duration::from_secs(1),
            read_timeout: Duration::from_secs(1),
            buffer_policy: UpstreamBodyBufferPolicy::fixed(
                crate::http::UpstreamBodyBufferOptions::memory(MAX_BODY_BYTES),
            ),
            limit_rate: 0,
            streaming: None,
        },
    )
    .await
    .unwrap();
    let response = finalize_grpc_response(upstream, context);

    assert_eq!(response.status, 200);
    assert_eq!(response.header("Content-Type"), Some("application/grpc"));
    assert_eq!(response.header("X-Pass"), Some("visible"));
    assert!(response.header("X-Hide").is_none());
    assert!(response.header("Date").is_none());
    assert!(response.header("Server").is_none());
    assert_eq!(
        response
            .trailers
            .iter()
            .find(|header| header.name.eq_ignore_ascii_case("grpc-message"))
            .map(|header| header.value.as_str()),
        Some("OK")
    );
    assert_eq!(
        response
            .body
            .into_memory(MAX_BODY_BYTES as u64)
            .await
            .unwrap(),
        b"\0\0\0\0\x02ok"
    );
    drop(connection);
    backend.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}
