use super::*;

#[tokio::test]
async fn evaluates_geo_variables_by_cidr_and_source_variable() {
    let (prefix, config) = test_config(
        r#"
                worker_processes 1;
                events { worker_connections 64; }
                http {
                    geo $zone {
                        default world;
                        127.0.0.0/8 loopback;
                        127.0.0.1 direct;
                    }
                    geo $http_x_forwarded_for $forwarded {
                        default none;
                        10.0.0.0/8 internal;
                    }
                    geo $http_x_forwarded_for $ranged {
                        ranges;
                        default none;
                        10.0.0.1-10.0.0.9 internal;
                    }
                    server {
                        listen 127.0.0.1:18103;
                        location / {
                            return 200 "$zone|$forwarded|$ranged\n";
                        }
                    }
                }
            "#,
        &[],
    );
    let direct = request_body(
        &config,
        "GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(
        direct.ends_with("direct|none|none\n"),
        "unexpected geo response:\n{direct}"
    );

    let forwarded = request_body(
            &config,
            "GET / HTTP/1.1\r\nHost: localhost\r\nX-Forwarded-For: 10.2.3.4, 192.0.2.1\r\nConnection: close\r\n\r\n",
        )
        .await;
    assert!(
        forwarded.ends_with("direct|internal|none\n"),
        "unexpected geo response:\n{forwarded}"
    );

    let ranged = request_body(
            &config,
            "GET / HTTP/1.1\r\nHost: localhost\r\nX-Forwarded-For: 10.0.0.5\r\nConnection: close\r\n\r\n",
        )
        .await;
    assert!(
        ranged.ends_with("direct|internal|internal\n"),
        "unexpected geo response:\n{ranged}"
    );
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn applies_trusted_real_ip_headers_before_access_and_geo() {
    let (prefix, config) = test_config(
        r#"
                worker_processes 1;
                events { worker_connections 64; }
                http {
                    set_real_ip_from 127.0.0.0/8;
                    real_ip_header X-Forwarded-For;
                    real_ip_recursive on;
                    geo $remote_addr $zone {
                        default public;
                        203.0.113.0/24 client;
                    }
                    server {
                        listen 127.0.0.1:18105;
                        location / {
                            allow 203.0.113.0/24;
                            deny all;
                            return 200 "$remote_addr|$zone\n";
                        }
                        location /untrusted/ {
                            set_real_ip_from 10.0.0.0/8;
                            real_ip_header X-Real-IP;
                            return 200 "$remote_addr\n";
                        }
                    }
                }
            "#,
        &[],
    );
    let trusted = request_body(
            &config,
            "GET / HTTP/1.1\r\nHost: localhost\r\nX-Forwarded-For: 203.0.113.10, 127.0.0.1\r\nConnection: close\r\n\r\n",
        )
        .await;
    assert!(
        trusted.ends_with("203.0.113.10|client\n"),
        "unexpected real_ip response:\n{trusted}"
    );

    let missing = request_body(
        &config,
        "GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(missing.starts_with("HTTP/1.1 403 Forbidden\r\n"));

    let untrusted = request_body(
            &config,
            "GET /untrusted/ HTTP/1.1\r\nHost: localhost\r\nX-Real-IP: 203.0.113.20\r\nConnection: close\r\n\r\n",
        )
        .await;
    assert!(
        untrusted.ends_with("127.0.0.1\n"),
        "unexpected real_ip response:\n{untrusted}"
    );
    let _ = std::fs::remove_dir_all(prefix);
}

#[test]
fn trusts_unix_peers_for_http_real_ip_rewrites() {
    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            http {
                set_real_ip_from unix:;
                real_ip_header X-Forwarded-For;
                server {
                    listen 127.0.0.1:18114;
                    location / { return 204; }
                }
            }
        "#,
        &[],
    );
    let server = &config.http.as_ref().unwrap().servers[0];
    let request = Request {
        method: "GET".into(),
        target: "/".into(),
        version: "HTTP/1.1".into(),
        headers: vec![
            Header {
                name: "Host".into(),
                value: "localhost".into(),
            },
            Header {
                name: "X-Forwarded-For".into(),
                value: "203.0.113.10".into(),
            },
        ],
        body_stream: None,
        body: Vec::new(),
        remote_addr: "unix:/tmp/client.sock".into(),
        local_addr: "127.0.0.1:18114".into(),
        keep_alive: false,
    };

    assert_eq!(
        effective_real_ip(server, &request, "/", None, "unix:/tmp/client.sock", None,),
        "203.0.113.10"
    );
    let _ = std::fs::remove_dir_all(prefix);
}
