use super::*;
mod control;

#[tokio::test]
async fn handles_rewrite_arguments_and_captures() {
    let (prefix, config) = test_config(
        r#"
                worker_processes 1;
                events { worker_connections 64; }
                http {
                    server {
                        listen 127.0.0.1:18092;
                        location /last/ {
                            rewrite ^/last/(.*)$ /seen/${1} last;
                        }
                        location /clear/ {
                            rewrite ^/clear/(.*)$ /seen/$1? last;
                        }
                        location /explicit/ {
                            rewrite ^/explicit/(.*)$ /seen/$1?from=rewrite last;
                        }
                        location /redirect/ {
                            rewrite ^/redirect/(.*)$ /target/$1 redirect;
                        }
                        location /seen/ {
                            return 200 "$uri|$args\n";
                        }
                        location /multi/ {
                            rewrite ^/multi/(.*)$ /step/$1;
                            rewrite ^/step/(.*)$ /final/$1;
                            return 200 "final:$uri\n";
                        }
                    }
                }
            "#,
        &[],
    );
    let preserved = request_body(
        &config,
        "GET /last/item?x=1 HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(preserved.ends_with("/seen/item|x=1\n"));

    let cleared = request_body(
        &config,
        "GET /clear/item?x=1 HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(cleared.ends_with("/seen/item|\n"));

    let explicit = request_body(
        &config,
        "GET /explicit/item?x=1 HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(explicit.ends_with("/seen/item|from=rewrite&x=1\n"));

    let redirect = request_body(
        &config,
        "GET /redirect/item?x=1 HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(redirect.starts_with("HTTP/1.1 302 Moved Temporarily\r\n"));
    assert!(redirect.contains("Location: http://localhost:18092/target/item?x=1\r\n"));

    let continued = request_body(
        &config,
        "GET /multi/item HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(continued.ends_with("final:/final/item\n"));
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn applies_redirect_location_directives() {
    let (prefix, config) = test_config(
        r#"
                worker_processes 1;
                events { worker_connections 64; }
                http {
                    server {
                        listen 127.0.0.1:18133;
                        server_name configured.test;
                        location = /default {
                            return 302 /next?x=1;
                        }
                        location = /relative {
                            absolute_redirect off;
                            return 302 /next;
                        }
                        location = /configured {
                            server_name_in_redirect on;
                            return 302 /next;
                        }
                        location = /noport {
                            port_in_redirect off;
                            return 302 /next;
                        }
                        location /directory {
                            root html;
                        }
                    }
                }
            "#,
        &[],
    );
    std::fs::create_dir_all(prefix.join("html/directory")).unwrap();

    let default = request_body(
        &config,
        "GET /default HTTP/1.1\r\nHost: request.test\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(default.contains("Location: http://request.test:18133/next?x=1\r\n"));

    let relative = request_body(
        &config,
        "GET /relative HTTP/1.1\r\nHost: request.test\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(relative.contains("Location: /next\r\n"));

    let configured = request_body(
        &config,
        "GET /configured HTTP/1.1\r\nHost: request.test\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(configured.contains("Location: http://configured.test:18133/next\r\n"));

    let noport = request_body(
        &config,
        "GET /noport HTTP/1.1\r\nHost: request.test\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(noport.contains("Location: http://request.test/next\r\n"));

    let directory = request_body(
        &config,
        "GET /directory HTTP/1.1\r\nHost: request.test\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(directory.contains("Location: http://request.test:18133/directory/\r\n"));

    let mut secure = Response::empty(302);
    secure.set_header("Location", "/secure");
    let request = Request {
        method: "GET".into(),
        target: "/secure".into(),
        version: "HTTP/1.1".into(),
        headers: (vec![Header {
            name: "Host".into(),
            value: "request.test".into(),
        }])
        .into(),
        body_stream: None,
        body: Vec::new(),
        remote_addr: "127.0.0.1:45678".into(),
        local_addr: "127.0.0.1:443".into(),
        keep_alive: false,
    };
    apply_redirect_location(
        &mut secure,
        &config.http.as_ref().unwrap().servers[0].context,
        &config.http.as_ref().unwrap().servers[0],
        &request,
        &ListenAddress::Inet("127.0.0.1:443".parse().unwrap()),
        true,
    );
    assert_eq!(
        secure.header("Location"),
        Some("https://request.test/secure")
    );
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn exposes_regex_location_and_rewrite_captures() {
    let (prefix, config) = test_config(
        r#"
                worker_processes 1;
                events { worker_connections 64; }
                http {
                    server {
                        listen 127.0.0.1:18093;
                        location ~ ^/capture/(?<name>[a-z]+)/([0-9]+)$ {
                            return 200 "$1|$2|$name\n";
                        }
                        location /rewrite-capture/ {
                            rewrite ^/rewrite-capture/(.*)$ /target/$1;
                            return 200 "$1\n";
                        }
                    }
                }
            "#,
        &[],
    );
    let location = request_body(
        &config,
        "GET /capture/item/42 HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(location.ends_with("item|42|item\n"));

    let rewrite = request_body(
        &config,
        "GET /rewrite-capture/value HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(rewrite.ends_with("value\n"));
    let _ = std::fs::remove_dir_all(prefix);
}
