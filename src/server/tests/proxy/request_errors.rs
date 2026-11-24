use super::super::support::*;
use super::super::*;
use crate::http::RequestBodyStream;

#[tokio::test]
async fn truncated_file_backed_request_body_maps_to_500_and_truncates_upstream_body() {
    let upstream = match TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => listener,
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!("skipping proxy request body error test: TCP binding is not permitted");
            return;
        }
        Err(error) => panic!("bind proxy request body upstream: {error}"),
    };
    let upstream_address = upstream.local_addr().unwrap();
    let backend = tokio::spawn(async move {
        let (stream, _) = upstream.accept().await.unwrap();
        let mut reader = BufReader::new(stream);
        let mut line = String::new();
        loop {
            line.clear();
            reader.read_line(&mut line).await.unwrap();
            assert!(!line.is_empty(), "proxy request headers ended unexpectedly");
            if line == "\r\n" {
                break;
            }
        }
        let mut body = Vec::new();
        let _ = reader.read_to_end(&mut body).await;
        body
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
                        listen 127.0.0.1:18599;
                        location / {{
                            proxy_pass http://{upstream_address};
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
    sender.send(Bytes::from_static(b"part")).await;
    sender.send(Bytes::from_static(b"more")).await;
    drop(sender);
    let spooled = crate::http::spool_request_body(
        Some(&stream),
        true,
        4,
        &context.client_body.temp_path,
        context.client_body.in_file_only,
        0,
    )
    .await
    .unwrap();
    let file = spooled.file.expect("file-backed request body");
    std::fs::write(file.path(), b"part").unwrap();
    let request = Request {
        method: "POST".into(),
        target: "/".into(),
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
        body_stream: Some(RequestBodyStream::from_file(file.clone())),
        body: Vec::new(),
        remote_addr: "127.0.0.1:45678".into(),
        local_addr: "127.0.0.1:18599".into(),
        keep_alive: false,
    };
    let env = RequestEnv {
        uri: "/".into(),
        args: None,
        unparsed_uri: Some("/".into()),
        valid_location: true,
        internal: false,
        bypass_try_files: false,
        add_uri_to_alias: false,
        limits_checked: false,
        auth_request_checked: false,
        mirror_checked: false,
    };
    let remote: SocketAddr = "127.0.0.1:45678".parse().unwrap();
    let local: SocketAddr = "127.0.0.1:18599".parse().unwrap();
    let mut variables = Variables::new(&request, "localhost", server, &local, &remote);
    let response = proxy_request(
        context.proxy_pass.as_ref().unwrap(),
        http,
        &request,
        &env,
        None,
        context,
        &mut variables,
    )
    .await;

    assert_eq!(response.response.status, 500);
    assert_eq!(backend.await.unwrap(), b"part");

    drop(request);
    drop(variables);
    drop(file);
    assert!(
        !contains_generated_temp_file(&prefix.join("body_temp")),
        "truncated file-backed proxy request body should be removed"
    );
    let _ = std::fs::remove_dir_all(prefix);
}
