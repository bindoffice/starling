use super::super::support::*;
use super::super::*;
use super::support::*;

#[tokio::test]
async fn fastcgi_maps_truncated_file_backed_request_body_to_500() {
    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            http {
                client_body_buffer_size 4;
                client_body_temp_path body_temp;
                server {
                    listen 127.0.0.1:18592;
                    location / {
                        fastcgi_pass 127.0.0.1:9000;
                        fastcgi_buffering off;
                    }
                }
            }
        "#,
        &[],
    );
    let server = &config.http.as_ref().unwrap().servers[0];
    let context = server.locations[0].context.clone();
    let (sender, mut request) = streaming_chunked_request("/", "127.0.0.1:18592");
    sender.send(Bytes::from_static(b"part")).await;
    sender.send(Bytes::from_static(b"more")).await;
    drop(sender);
    let mut variables = Variables::new(&request, "localhost", server, &remote(), &local(18592));
    prepare_request_body(&mut request, &context, &mut variables)
        .await
        .unwrap();
    let file = request.body_file().expect("file-backed request body");
    std::fs::write(file.path(), b"part").unwrap();

    let (client, mut backend) = tokio::io::duplex(64 * 1024);
    let backend_task = tokio::spawn(async move {
        let mut received = Vec::new();
        let _ = backend.read_to_end(&mut received).await;
        received
    });
    let failure =
        match fastcgi_exchange(client, &request, &test_env(), None, &context, &variables).await {
            Ok(_) => panic!("truncated file-backed body must fail"),
            Err(failure) => failure,
        };
    assert!(matches!(failure, ProxyFailure::RequestBody(500)));
    let received = backend_task.await.unwrap();
    assert!(!received.ends_with(b"partmore"));

    drop(file);
    drop(variables);
    drop(request);
    assert!(
        !contains_generated_temp_file(&prefix.join("body_temp")),
        "truncated file-backed request body should be removed"
    );
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn uwsgi_maps_truncated_file_backed_request_body_to_500() {
    run_truncated_file_backed_request_body(RawGateway::Uwsgi, 18595).await;
}

#[tokio::test]
async fn scgi_maps_truncated_file_backed_request_body_to_500() {
    run_truncated_file_backed_request_body(RawGateway::Scgi, 18596).await;
}

#[derive(Clone, Copy)]
enum RawGateway {
    Uwsgi,
    Scgi,
}

async fn run_truncated_file_backed_request_body(gateway: RawGateway, port: u16) {
    let (directive, buffering) = match gateway {
        RawGateway::Uwsgi => ("uwsgi_pass", "uwsgi_buffering"),
        RawGateway::Scgi => ("scgi_pass", "scgi_buffering"),
    };
    let (prefix, config) = test_config(
        &format!(
            r#"
                worker_processes 1;
                events {{ worker_connections 64; }}
                http {{
                    client_body_buffer_size 4;
                    client_body_temp_path body_temp;
                    server {{
                        listen 127.0.0.1:{port};
                        location / {{
                            {directive} 127.0.0.1:9000;
                            {buffering} off;
                        }}
                    }}
                }}
            "#
        ),
        &[],
    );
    let server = &config.http.as_ref().unwrap().servers[0];
    let context = server.locations[0].context.clone();
    let (sender, mut request) = streaming_chunked_request("/", &format!("127.0.0.1:{port}"));
    sender.send(Bytes::from_static(b"part")).await;
    sender.send(Bytes::from_static(b"more")).await;
    drop(sender);
    let mut variables = Variables::new(&request, "localhost", server, &remote(), &local(port));
    prepare_request_body(&mut request, &context, &mut variables)
        .await
        .unwrap();
    let file = request.body_file().expect("file-backed request body");
    std::fs::write(file.path(), b"part").unwrap();

    let (client, mut backend) = tokio::io::duplex(64 * 1024);
    let backend_task = tokio::spawn(async move {
        let mut received = Vec::new();
        let _ = backend.read_to_end(&mut received).await;
        received
    });
    let result = match gateway {
        RawGateway::Uwsgi => uwsgi_exchange(client, &request, &context, &variables).await,
        RawGateway::Scgi => scgi_exchange(client, &request, &context, &variables).await,
    };
    let failure = match result {
        Ok(_) => panic!("truncated file-backed body must fail"),
        Err(failure) => failure,
    };
    assert!(matches!(failure, ProxyFailure::RequestBody(500)));
    let received = backend_task.await.unwrap();
    assert!(!received.ends_with(b"partmore"));

    drop(file);
    drop(variables);
    drop(request);
    assert!(
        !contains_generated_temp_file(&prefix.join("body_temp")),
        "truncated file-backed request body should be removed"
    );
    let _ = std::fs::remove_dir_all(prefix);
}
