use super::*;

#[tokio::test]
async fn pool_keys_keep_the_generation_captured_when_the_request_started() {
    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            http {
                upstream backend {
                    server 127.0.0.1:18180;
                    keepalive 2;
                }
                server {
                    listen 127.0.0.1:18181;
                    location / {
                        proxy_pass http://backend;
                        proxy_http_version 1.1;
                    }
                }
            }
        "#,
        &[],
    );
    let state = test_state(&config);
    let snapshot = state.config_snapshot();
    let request_config = snapshot.config.clone();
    let generation = snapshot.upstream_keepalive_generation;
    let server = &request_config.http.as_ref().unwrap().servers[0];
    let location = &server.locations[0];
    let context = &location.context;
    let request = Request {
        method: "GET".into(),
        target: "/".into(),
        version: "HTTP/1.1".into(),
        headers: vec![Header {
            name: "Host".into(),
            value: "localhost".into(),
        }],
        body_stream: None,
        body: Vec::new(),
        remote_addr: "127.0.0.1:45678".into(),
        local_addr: "127.0.0.1:18181".into(),
        keep_alive: false,
    };
    let remote: SocketAddr = "127.0.0.1:45678".parse().unwrap();
    let local: SocketAddr = "127.0.0.1:18181".parse().unwrap();
    let mut variables = Variables::new(&request, "localhost", server, &local, &remote);
    let pool = state.upstream_keepalive.clone();
    pool.clear().await;
    state.config.store(Arc::new(RuntimeSnapshot {
        config: request_config.clone(),
        listener_groups: build_listener_groups(&request_config).into(),
        http_error_loggers: build_http_error_loggers(&request_config).unwrap(),
        upstream_keepalive_generation: pool.generation(),
    }));

    variables.upstream_keepalive = pool.clone();
    variables.upstream_keepalive_generation = generation;
    let key = proxy_upstream_pool_key(
        context.proxy_pass.as_ref().unwrap(),
        "127.0.0.1",
        18180,
        "1.1",
        context,
        &variables,
    );
    assert_eq!(
        key.generation, generation,
        "an in-flight request must keep the generation captured at dispatch start"
    );
    assert_eq!(pool.generation(), generation + 1);
    let _ = std::fs::remove_dir_all(prefix);
}
