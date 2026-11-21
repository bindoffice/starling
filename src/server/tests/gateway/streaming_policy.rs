use super::*;

#[test]
fn gateway_request_streaming_defaults_to_buffering() {
    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            http {
                server {
                    listen 127.0.0.1:18151;
                    location /fastcgi { fastcgi_pass 127.0.0.1:9000; }
                    location /uwsgi { uwsgi_pass 127.0.0.1:9001; }
                    location /scgi { scgi_pass 127.0.0.1:9002; }
                }
            }
        "#,
        &[],
    );
    let server = &config.http.as_ref().unwrap().servers[0];
    for (index, port) in [18151, 18151, 18151].into_iter().enumerate() {
        assert!(
            !streaming_enabled(server, index, port),
            "gateway location {index} must buffer by default"
        );
    }
    let _ = std::fs::remove_dir_all(prefix);
}

#[test]
fn request_buffering_off_enables_streaming_for_all_gateways() {
    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            http {
                server {
                    listen 127.0.0.1:18152;
                    location /fastcgi {
                        fastcgi_pass 127.0.0.1:9000;
                        fastcgi_request_buffering off;
                    }
                    location /uwsgi {
                        uwsgi_pass 127.0.0.1:9001;
                        uwsgi_request_buffering off;
                    }
                    location /scgi {
                        scgi_pass 127.0.0.1:9002;
                        scgi_request_buffering off;
                    }
                }
            }
        "#,
        &[],
    );
    let server = &config.http.as_ref().unwrap().servers[0];
    for (index, port) in [18152, 18152, 18152].into_iter().enumerate() {
        assert!(
            streaming_enabled(server, index, port),
            "gateway location {index} should stream when request buffering is off"
        );
    }
    let _ = std::fs::remove_dir_all(prefix);
}

#[test]
fn pass_request_body_off_wins_over_request_buffering_off() {
    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            http {
                server {
                    listen 127.0.0.1:18153;
                    location /fastcgi {
                        fastcgi_pass 127.0.0.1:9000;
                        fastcgi_request_buffering off;
                        fastcgi_pass_request_body off;
                    }
                    location /uwsgi {
                        uwsgi_pass 127.0.0.1:9001;
                        uwsgi_request_buffering off;
                        uwsgi_pass_request_body off;
                    }
                    location /scgi {
                        scgi_pass 127.0.0.1:9002;
                        scgi_request_buffering off;
                        scgi_pass_request_body off;
                    }
                }
            }
        "#,
        &[],
    );
    let server = &config.http.as_ref().unwrap().servers[0];
    for (index, port) in [18153, 18153, 18153].into_iter().enumerate() {
        assert!(
            !streaming_enabled(server, index, port),
            "gateway location {index} must not stream without pass_request_body"
        );
    }
    let _ = std::fs::remove_dir_all(prefix);
}

#[test]
fn file_only_and_mirror_consumers_disable_request_streaming() {
    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            http {
                server {
                    listen 127.0.0.1:18154;
                    mirror /mirror;
                    location /file-only {
                        fastcgi_pass 127.0.0.1:9000;
                        fastcgi_request_buffering off;
                        client_body_in_file_only on;
                    }
                    location /mirrored {
                        uwsgi_pass 127.0.0.1:9001;
                        uwsgi_request_buffering off;
                    }
                }
            }
        "#,
        &[],
    );
    let server = &config.http.as_ref().unwrap().servers[0];
    assert!(!streaming_enabled(server, 0, 18154));
    assert!(
        !streaming_enabled(server, 1, 18154),
        "mirror_request_body default must block competing streaming"
    );
    let _ = std::fs::remove_dir_all(prefix);
}

#[test]
fn gateway_3xx_only_disables_streaming_when_an_error_page_intercepts_it() {
    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            http {
                server {
                    listen 127.0.0.1:18156;
                    location /matching {
                        fastcgi_pass 127.0.0.1:9000;
                        fastcgi_buffering off;
                        fastcgi_intercept_errors on;
                        error_page 302 /fallback;
                    }
                    location /no-match {
                        fastcgi_pass 127.0.0.1:9000;
                        fastcgi_buffering off;
                        fastcgi_intercept_errors on;
                        error_page 404 /fallback;
                    }
                }
            }
        "#,
        &[],
    );
    let server = &config.http.as_ref().unwrap().servers[0];
    for (index, expected_complete) in [(0, true), (1, false)] {
        let context = &server.locations[index].context;
        let policy = GatewayStreamingPolicy {
            buffering: &context.fastcgi_buffering,
            ignore_headers: &context.fastcgi_ignore_headers,
            force_ranges: context.fastcgi_force_ranges,
            intercept_errors: context.fastcgi_intercept_errors,
        };
        assert_eq!(
            matches!(
                gateway_body_handling(context, &policy, 302, &[]),
                GatewayBodyHandling::CompleteBuffer
            ),
            expected_complete,
            "gateway location {index}"
        );
    }
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn uwsgi_chunked_body_without_content_length_is_not_streamed() {
    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            http {
                server {
                    listen 127.0.0.1:18155;
                    location / {
                        uwsgi_pass 127.0.0.1:9001;
                        uwsgi_request_buffering off;
                    }
                }
            }
        "#,
        &[],
    );
    let server = &config.http.as_ref().unwrap().servers[0];
    let context = server.locations[0].context.clone();
    let (sender, mut request) = streaming_chunked_request("/", "127.0.0.1:18155");
    assert!(!request_body_stream_enabled(&request, &context));
    sender.send(Bytes::from_static(b"part")).await;
    sender.send(Bytes::from_static(b"more")).await;
    drop(sender);

    let mut variables = Variables::new(&request, "localhost", server, &remote(), &local(18155));
    prepare_request_body(&mut request, &context, &mut variables)
        .await
        .unwrap();
    assert_eq!(request.body, b"partmore");
    assert!(request.body_stream.is_none());
    let _ = std::fs::remove_dir_all(prefix);
}

fn streaming_enabled(server: &ServerConfig, location: usize, port: u16) -> bool {
    let context = server
        .locations
        .get(location)
        .expect("gateway location")
        .context
        .clone();
    let (_sender, request) = streaming_request("/", &format!("127.0.0.1:{port}"), 4);
    request_body_stream_enabled(&request, &context)
}
