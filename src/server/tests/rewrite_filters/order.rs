use super::*;

#[tokio::test]
async fn executes_rewrite_actions_in_declaration_order() {
    let (prefix, config) = test_config(
        r#"
                worker_processes 1;
                events { worker_connections 64; }
                http {
                    server {
                        listen 127.0.0.1:19991;
                        location = /return-order {
                            set $state first;
                            return 200 "$state\n";
                            set $state second;
                        }
                        location = /if-order {
                            set $state before;
                            if ($arg_take = yes) {
                                set $state inside;
                            }
                            set $state after;
                            return 200 "$state\n";
                        }
                        location = /when-returned {
                            return 200 "first\n";
                            return 200 "second\n";
                        }
                        location = /if-rewrite-continues {
                            if ($arg_route = yes) {
                                rewrite ^ /after-if;
                            }
                            return 200 "$uri\n";
                        }
                    }
                }
            "#,
        &[],
    );

    let stopped = request_body(
        &config,
        "GET /return-order HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(stopped.ends_with("first\n"), "{stopped}");

    let ordered = request_body(
        &config,
        "GET /if-order?take=yes HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(ordered.ends_with("after\n"), "{ordered}");

    let first_return = request_body(
        &config,
        "GET /when-returned HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(first_return.ends_with("first\n"), "{first_return}");

    let continued = request_body(
        &config,
        "GET /if-rewrite-continues?route=yes HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(continued.ends_with("/after-if\n"), "{continued}");
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn if_set_values_are_visible_to_later_rewrites() {
    let (prefix, config) = test_config(
        r#"
                worker_processes 1;
                events { worker_connections 64; }
                http {
                    server {
                        listen 127.0.0.1:19992;
                        location = /dispatch {
                            set $target /fallback;
                            if ($arg_take = yes) {
                                set $target /matched;
                            }
                            rewrite ^ $target last;
                        }
                        location = /matched {
                            return 200 "matched\n";
                        }
                        location = /fallback {
                            return 200 "fallback\n";
                        }
                    }
                }
            "#,
        &[],
    );

    let matched = request_body(
        &config,
        "GET /dispatch?take=yes HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(matched.ends_with("matched\n"), "{matched}");

    let fallback = request_body(
        &config,
        "GET /dispatch HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(fallback.ends_with("fallback\n"), "{fallback}");
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn break_keeps_location_but_stops_the_script() {
    let (prefix, config) = test_config(
        r#"
                worker_processes 1;
                events { worker_connections 64; }
                http {
                    server {
                        listen 127.0.0.1:19993;
                        root html;
                        location /break/ {
                            rewrite ^/break/(.*)$ /target/$1 break;
                            return 200 "must-not-run\n";
                        }
                        location /target/ {
                            return 200 "must-not-rematch\n";
                        }
                        location = /bare-break {
                            break;
                            return 200 "must-not-run\n";
                        }
                    }
                }
            "#,
        &[("html/target/value", "served-after-break\n")],
    );

    let rewritten = request_body(
        &config,
        "GET /break/value HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(rewritten.starts_with("HTTP/1.1 200 OK\r\n"), "{rewritten}");
    assert!(rewritten.ends_with("served-after-break\n"), "{rewritten}");

    let bare = request_body(
        &config,
        "GET /bare-break HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(bare.starts_with("HTTP/1.1 404 Not Found\r\n"), "{bare}");
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn last_restarts_at_find_config_without_replaying_server_rewrite() {
    let (prefix, config) = test_config(
        r#"
                worker_processes 1;
                events { worker_connections 64; }
                http {
                    server {
                        listen 127.0.0.1:19994;
                        rewrite ^/entry$ /server-target last;
                        rewrite ^/server-target$ /loop last;

                        location = /server-target {
                            rewrite ^ /location-target last;
                        }
                        location = /location-target {
                            return 200 "location-ok\n";
                        }
                        location = /loop {
                            return 200 "server-rewrite-replayed\n";
                        }
                        location = /internal-source {
                            rewrite ^ /internal-target last;
                        }
                        location = /internal-target {
                            internal;
                            return 200 "internal-ok\n";
                        }
                    }
                }
            "#,
        &[],
    );

    let response = request_body(
        &config,
        "GET /entry HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(response.ends_with("location-ok\n"), "{response}");

    let internal = request_body(
        &config,
        "GET /internal-source HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(internal.ends_with("internal-ok\n"), "{internal}");
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn unqualified_rewrite_restarts_location_matching_after_the_script() {
    let (prefix, config) = test_config(
        r#"
                worker_processes 1;
                events { worker_connections 64; }
                http {
                    server {
                        listen 127.0.0.1:19995;
                        location = /source {
                            rewrite ^ /destination;
                        }
                        location = /destination {
                            return 200 "destination\n";
                        }
                        location /try/ {
                            try_files /actual.txt =404;
                        }
                        location = /actual.txt {
                            return 200 "must-not-rematch\n";
                        }
                    }
                }
            "#,
        &[("html/actual.txt", "try-files-found\n")],
    );

    let response = request_body(
        &config,
        "GET /source HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(response.ends_with("destination\n"), "{response}");

    let try_files = request_body(
        &config,
        "GET /try/missing HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(try_files.ends_with("try-files-found\n"), "{try_files}");
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn rewrite_cycles_fail_after_the_nginx_uri_change_limit() {
    let (prefix, config) = test_config(
        r#"
                worker_processes 1;
                events { worker_connections 64; }
                http {
                    server {
                        listen 127.0.0.1:19996;
                        location = /a {
                            rewrite ^ /b last;
                        }
                        location = /b {
                            rewrite ^ /a last;
                        }
                    }
                }
            "#,
        &[],
    );

    let response = request_body(
        &config,
        "GET /a HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(
        response.starts_with("HTTP/1.1 500 Internal Server Error\r\n"),
        "{response}"
    );
    let _ = std::fs::remove_dir_all(prefix);
}
