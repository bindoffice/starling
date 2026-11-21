use super::support::*;

#[test]
fn compiles_add_trailer_inheritance_and_conditions() {
    let (prefix, config) = test_config(
        r#"
                worker_processes 1;
                events { worker_connections 64; }
                http {
                    add_trailer X-Http http;
                    server {
                        listen 127.0.0.1:18173;
                        add_trailer X-Server server;
                        location /inherit/ { }
                        location /override/ {
                            add_trailer X-Location location;
                            if ($arg_condition = yes) {
                                add_trailer X-Conditional matched;
                            }
                        }
                    }
                }
            "#,
        &[],
    );
    let server = &config.http.as_ref().unwrap().servers[0];
    assert_eq!(server.context.add_trailers.len(), 1);
    assert_eq!(server.context.add_trailers[0].name, "X-Server");
    assert_eq!(server.locations[0].context.add_trailers[0].name, "X-Server");
    let location = &server.locations[1];
    assert_eq!(location.context.add_trailers.len(), 1);
    assert_eq!(location.context.add_trailers[0].name, "X-Location");
    assert_eq!(
        location.context.conditionals[0].add_trailers[0].name,
        "X-Conditional"
    );
    let _ = std::fs::remove_dir_all(prefix);
}

#[test]
fn rejects_invalid_add_trailer_arguments() {
    let error = test_config_error(
        r#"
                events { worker_connections 64; }
                http {
                    server {
                        listen 127.0.0.1:18173;
                        add_trailer X-Test;
                    }
                }
            "#,
    );
    assert!(
        error.contains("invalid number of arguments in \"add_trailer\" directive"),
        "{error}"
    );
}

#[tokio::test]
async fn adds_chunked_http11_trailers_with_variables() {
    let (prefix, config) = test_config(
        r#"
                worker_processes 1;
                events { worker_connections 64; }
                http {
                    server {
                        listen 127.0.0.1:18174;
                        add_trailer X-Status $status always;
                        add_trailer X-Empty $arg_missing always;
                        location = / { return 200 "hello\n"; }
                    }
                }
            "#,
        &[],
    );
    let response = request_body(
        &config,
        "GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(
        response.contains("Transfer-Encoding: chunked\r\n"),
        "{response}"
    );
    assert!(!response.contains("Content-Length:"), "{response}");
    assert!(
        response.ends_with("\r\n\r\n6\r\nhello\n\r\n0\r\nX-Status: 200\r\n\r\n"),
        "{response:?}"
    );
    assert!(!response.contains("X-Empty:"), "{response}");
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn filters_unsafe_and_empty_trailers() {
    let (prefix, config) = test_config(
        r#"
                worker_processes 1;
                events { worker_connections 64; }
                http {
                    server {
                        listen 127.0.0.1:18175;
                        add_trailer X-Safe unsafe;
                        add_trailer X-Always "$status" always;
                        location /conditional/ {
                            if ($arg_condition = yes) {
                                add_trailer_inherit merge;
                                add_trailer X-Conditional matched;
                            }
                            return 200 "ok\n";
                        }
                        location = / { return 404 "missing\n"; }
                    }
                }
            "#,
        &[],
    );
    let missing = request_body(
        &config,
        "GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(!missing.contains("X-Safe:"), "{missing}");
    assert!(missing.ends_with("0\r\nX-Always: 404\r\n\r\n"), "{missing}");

    let conditional = request_body(
        &config,
        "GET /conditional/?condition=yes HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(conditional.contains("X-Safe: unsafe\r\n"), "{conditional}");
    assert!(
        conditional.contains("X-Conditional: matched\r\n"),
        "{conditional}"
    );
    assert_eq!(
        conditional.matches("X-Conditional: matched\r\n").count(),
        1,
        "{conditional}"
    );
    assert!(conditional.ends_with("\r\n\r\n"), "{conditional:?}");
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn omits_trailers_for_http10_and_head_requests() {
    let (prefix, config) = test_config(
        r#"
                worker_processes 1;
                events { worker_connections 64; }
                http {
                    server {
                        listen 127.0.0.1:18176;
                        add_trailer X-Trailer value always;
                        location = / { return 200 "hello\n"; }
                    }
                }
            "#,
        &[],
    );
    let http10 = request_body(
        &config,
        "GET / HTTP/1.0\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(!http10.contains("Transfer-Encoding:"), "{http10}");
    assert!(http10.contains("Content-Length: 6\r\n"), "{http10}");
    assert!(http10.ends_with("\r\n\r\nhello\n"), "{http10:?}");

    let head = request_body(
        &config,
        "HEAD / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(!head.contains("Transfer-Encoding:"), "{head}");
    assert!(head.contains("Content-Length: 6\r\n"), "{head}");
    assert!(head.ends_with("\r\n\r\n"), "{head:?}");
    let _ = std::fs::remove_dir_all(prefix);
}
