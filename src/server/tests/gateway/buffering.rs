use super::*;

#[derive(Clone, Copy)]
enum Gateway {
    FastCgi,
    Uwsgi,
    Scgi,
}

#[tokio::test]
async fn gateways_use_the_buffered_window_or_direct_streaming() {
    let body = "g".repeat(16 * 1024);
    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            http {
                server {
                    listen 127.0.0.1:18147;
                    location = /fastcgi-no {
                        fastcgi_pass 127.0.0.1:9000;
                        fastcgi_buffers 2 4k;
                        fastcgi_busy_buffers_size 4k;
                    }
                    location = /fastcgi-ignored {
                        fastcgi_pass 127.0.0.1:9000;
                        fastcgi_buffers 2 4k;
                        fastcgi_busy_buffers_size 4k;
                        fastcgi_ignore_headers X-Accel-Buffering;
                    }
                    location = /fastcgi-forced {
                        fastcgi_pass 127.0.0.1:9000;
                        fastcgi_buffering off;
                        fastcgi_buffers 2 4k;
                        fastcgi_busy_buffers_size 4k;
                    }
                    location = /uwsgi-no {
                        uwsgi_pass 127.0.0.1:9001;
                        uwsgi_buffers 2 4k;
                        uwsgi_busy_buffers_size 4k;
                    }
                    location = /uwsgi-ignored {
                        uwsgi_pass 127.0.0.1:9001;
                        uwsgi_buffers 2 4k;
                        uwsgi_busy_buffers_size 4k;
                        uwsgi_ignore_headers X-Accel-Buffering;
                    }
                    location = /uwsgi-forced {
                        uwsgi_pass 127.0.0.1:9001;
                        uwsgi_buffering off;
                        uwsgi_buffers 2 4k;
                        uwsgi_busy_buffers_size 4k;
                    }
                    location = /scgi-no {
                        scgi_pass 127.0.0.1:9002;
                        scgi_buffers 2 4k;
                        scgi_busy_buffers_size 4k;
                    }
                    location = /scgi-ignored {
                        scgi_pass 127.0.0.1:9002;
                        scgi_buffers 2 4k;
                        scgi_busy_buffers_size 4k;
                        scgi_ignore_headers X-Accel-Buffering;
                    }
                    location = /scgi-forced {
                        scgi_pass 127.0.0.1:9002;
                        scgi_buffering off;
                        scgi_buffers 2 4k;
                        scgi_busy_buffers_size 4k;
                    }
                }
            }
        "#,
        &[],
    );
    let server = &config.http.as_ref().unwrap().servers[0];
    let response_text = format!(
        "Status: 200 OK\r\nContent-Type: text/plain\r\nX-Accel-Buffering: no\r\n\r\n{body}"
    );
    let forced_response_text = format!(
        "Status: 200 OK\r\nContent-Type: text/plain\r\nX-Accel-Buffering: yes\r\n\r\n{body}"
    );

    for (gateway, no_path, ignored_path, forced_path) in [
        (
            Gateway::FastCgi,
            "/fastcgi-no",
            "/fastcgi-ignored",
            "/fastcgi-forced",
        ),
        (
            Gateway::Uwsgi,
            "/uwsgi-no",
            "/uwsgi-ignored",
            "/uwsgi-forced",
        ),
        (Gateway::Scgi, "/scgi-no", "/scgi-ignored", "/scgi-forced"),
    ] {
        let context = context_for(server, no_path);
        let response = run_gateway(gateway, server, context, no_path, &response_text)
            .await
            .unwrap();
        assert_eq!(response.status, 200);
        assert_eq!(
            response
                .upstream_control()
                .and_then(|control| control.buffering),
            Some(AccelBuffering::No)
        );
        assert_eq!(stream_body(response.body).await, body.as_bytes());

        let context = context_for(server, ignored_path);
        let response = run_gateway(gateway, server, context, ignored_path, &response_text)
            .await
            .unwrap();
        assert_eq!(
            response
                .upstream_control()
                .and_then(|control| control.buffering),
            None
        );
        assert_eq!(stream_body(response.body).await, body.as_bytes());

        let context = context_for(server, forced_path);
        let response = run_gateway(gateway, server, context, forced_path, &forced_response_text)
            .await
            .unwrap();
        assert_eq!(
            response
                .upstream_control()
                .and_then(|control| control.buffering),
            Some(AccelBuffering::Yes)
        );
        assert_eq!(stream_body(response.body).await, body.as_bytes());
    }

    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn gateways_keep_small_buffered_responses_whole() {
    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            http {
                server {
                    listen 127.0.0.1:18148;
                    location = /fastcgi { fastcgi_pass 127.0.0.1:9000; }
                    location = /uwsgi { uwsgi_pass 127.0.0.1:9001; }
                    location = /scgi { scgi_pass 127.0.0.1:9002; }
                }
            }
        "#,
        &[],
    );
    let server = &config.http.as_ref().unwrap().servers[0];
    let response_text = "Status: 200 OK\r\nContent-Type: text/plain\r\n\r\nsmall";
    for (gateway, path) in [
        (Gateway::FastCgi, "/fastcgi"),
        (Gateway::Uwsgi, "/uwsgi"),
        (Gateway::Scgi, "/scgi"),
    ] {
        let context = context_for(server, path);
        let response = run_gateway(gateway, server, context, path, response_text)
            .await
            .unwrap();
        assert!(
            matches!(&response.body, ResponseBody::Bytes(_)),
            "small {path} response should stay fully buffered"
        );
        assert_eq!(
            response.into_bytes(),
            b"small",
            "small {path} response body"
        );
    }
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn gateways_stream_when_accel_headers_are_ignored() {
    let body = "h".repeat(16 * 1024);
    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            http {
                server {
                    listen 127.0.0.1:18150;
                    location = /fastcgi {
                        fastcgi_pass 127.0.0.1:9000;
                        fastcgi_buffering off;
                        fastcgi_ignore_headers X-Accel-Redirect X-Accel-Charset;
                    }
                    location = /uwsgi {
                        uwsgi_pass 127.0.0.1:9001;
                        uwsgi_buffering off;
                        uwsgi_ignore_headers X-Accel-Redirect X-Accel-Charset;
                    }
                    location = /scgi {
                        scgi_pass 127.0.0.1:9002;
                        scgi_buffering off;
                        scgi_ignore_headers X-Accel-Redirect X-Accel-Charset;
                    }
                }
            }
        "#,
        &[],
    );
    let server = &config.http.as_ref().unwrap().servers[0];
    let response_text = format!(
        "Status: 200 OK\r\nContent-Type: text/plain\r\n\
         X-Accel-Redirect: /internal\r\nX-Accel-Charset: utf-8\r\n\r\n{body}"
    );
    for (gateway, path) in [
        (Gateway::FastCgi, "/fastcgi"),
        (Gateway::Uwsgi, "/uwsgi"),
        (Gateway::Scgi, "/scgi"),
    ] {
        let context = context_for(server, path);
        let response = run_gateway(gateway, server, context, path, &response_text)
            .await
            .unwrap();
        assert_eq!(response.status, 200);
        assert_eq!(stream_body(response.body).await, body.as_bytes());
    }
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn gateways_reject_truncation_before_buffered_commit() {
    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            http {
                server {
                    listen 127.0.0.1:18149;
                    location = /fastcgi { fastcgi_pass 127.0.0.1:9000; }
                    location = /uwsgi { uwsgi_pass 127.0.0.1:9001; }
                    location = /scgi { scgi_pass 127.0.0.1:9002; }
                }
            }
        "#,
        &[],
    );
    let server = &config.http.as_ref().unwrap().servers[0];
    let response_text =
        "Status: 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 100\r\n\r\nshort";
    for (gateway, path) in [
        (Gateway::FastCgi, "/fastcgi"),
        (Gateway::Uwsgi, "/uwsgi"),
        (Gateway::Scgi, "/scgi"),
    ] {
        let context = context_for(server, path);
        assert!(
            run_gateway(gateway, server, context, path, response_text)
                .await
                .is_err(),
            "{path} should reject a truncated buffered response"
        );
    }
    let _ = std::fs::remove_dir_all(prefix);
}

async fn stream_body(body: ResponseBody) -> Vec<u8> {
    let ResponseBody::Stream(mut stream) = body else {
        panic!("large gateway response should use streaming");
    };
    let mut body = Vec::new();
    while let Some(item) = stream.next().await.unwrap() {
        match item {
            ResponseStreamItem::Data(data) => body.extend_from_slice(&data),
            ResponseStreamItem::Trailers(_) => panic!("unexpected gateway trailers"),
        }
    }
    body
}

fn context_for<'a>(server: &'a ServerConfig, uri: &str) -> &'a EffectiveConfig {
    &resolve_location(&server.locations, uri)
        .unwrap_or_else(|| panic!("missing location {uri}"))
        .context
}

async fn run_gateway(
    gateway: Gateway,
    server: &ServerConfig,
    context: &EffectiveConfig,
    path: &str,
    response: &str,
) -> Result<Response, ProxyFailure> {
    let request = Request {
        method: "GET".into(),
        target: path.into(),
        version: "HTTP/1.1".into(),
        headers: (vec![Header {
            name: "Host".into(),
            value: "localhost".into(),
        }])
        .into(),
        body_stream: None,
        body: Vec::new(),
        remote_addr: "127.0.0.1:45678".into(),
        local_addr: "127.0.0.1:18147".into(),
        keep_alive: false,
    };
    let env = RequestEnv {
        uri: path.into(),
        args: None,
        unparsed_uri: Some(path.into()),
        valid_location: true,
        internal: false,
        bypass_try_files: false,
        add_uri_to_alias: false,
        limits_checked: false,
        auth_request_checked: false,
        mirror_checked: false,
    };
    let remote: SocketAddr = "127.0.0.1:45678".parse().unwrap();
    let local: SocketAddr = "127.0.0.1:18147".parse().unwrap();
    let variables = Variables::new(&request, "localhost", server, &local, &remote);
    let (client, mut backend) = tokio::io::duplex(64 * 1024);
    let response = response.to_string();
    let writer = tokio::spawn(async move {
        match gateway {
            Gateway::FastCgi => {
                write_fastcgi_record(&mut backend, FCGI_STDOUT, response.as_bytes())
                    .await
                    .unwrap();
                write_fastcgi_record(&mut backend, FCGI_END_REQUEST, &[0; 8])
                    .await
                    .unwrap();
            }
            Gateway::Uwsgi | Gateway::Scgi => {
                backend.write_all(response.as_bytes()).await.unwrap();
            }
        }
        backend.shutdown().await.unwrap();
    });
    let result = match gateway {
        Gateway::FastCgi => {
            fastcgi_exchange(client, &request, &env, None, context, &variables).await
        }
        Gateway::Uwsgi => uwsgi_exchange(client, &request, context, &variables).await,
        Gateway::Scgi => scgi_exchange(client, &request, context, &variables).await,
    };
    writer.await.unwrap();
    result
}
