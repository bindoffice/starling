use super::*;

#[tokio::test]
async fn enforces_basic_auth_with_nginx_password_formats() {
    let (prefix, config) = test_config(
        r#"
                worker_processes 1;
                events { worker_connections 64; }
                http {
                    auth_basic "Restricted";
                    auth_basic_user_file conf/htpasswd;
                    server {
                        listen 127.0.0.1:18110;
                        location / { return 200 "$remote_user\n"; }
                        location /public/ {
                            auth_basic off;
                            return 200 "public\n";
                        }
                    }
                }
            "#,
        &[(
            "conf/htpasswd",
            "plain:{PLAIN}pass\nsha:{SHA}5en6G6MezRroT3XKqkdPOmY/BfQ=\n",
        )],
    );
    let challenge = request_body(
        &config,
        "GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(
        challenge.starts_with("HTTP/1.1 401 Unauthorized\r\n"),
        "unexpected auth challenge:\n{challenge}"
    );
    assert!(
        challenge.contains("WWW-Authenticate: Basic realm=\"Restricted\"\r\n"),
        "unexpected auth challenge:\n{challenge}"
    );

    let wrong = request_body(
            &config,
            "GET / HTTP/1.1\r\nHost: localhost\r\nAuthorization: Basic cGxhaW46d3Jvbmc=\r\nConnection: close\r\n\r\n",
        )
        .await;
    assert!(wrong.starts_with("HTTP/1.1 401 Unauthorized\r\n"));

    let plain = request_body(
            &config,
            "GET / HTTP/1.1\r\nHost: localhost\r\nAuthorization: Basic cGxhaW46cGFzcw==\r\nConnection: close\r\n\r\n",
        )
        .await;
    assert!(
        plain.starts_with("HTTP/1.1 200 OK\r\n") && plain.ends_with("plain\n"),
        "unexpected authenticated response:\n{plain}"
    );

    let sha_user = request_body(
            &config,
            "GET / HTTP/1.1\r\nHost: localhost\r\nAuthorization: Basic c2hhOnNlY3JldA==\r\nConnection: close\r\n\r\n",
        )
        .await;
    assert!(
        sha_user.starts_with("HTTP/1.1 200 OK\r\n") && sha_user.ends_with("sha\n"),
        "unexpected SHA-authenticated response:\n{sha_user}"
    );

    let public = request_body(
        &config,
        "GET /public/ HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(
        public.starts_with("HTTP/1.1 200 OK\r\n") && public.ends_with("public\n"),
        "unexpected public response:\n{public}"
    );
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn applies_basic_auth_overrides_from_limit_except() {
    let (prefix, config) = test_config(
        r#"
                worker_processes 1;
                events { worker_connections 64; }
                http {
                    server {
                        listen 127.0.0.1:18185;
                        location /limited {
                            auth_basic off;
                            limit_except GET {
                                auth_basic "Limited";
                                auth_basic_user_file conf/htpasswd;
                            }
                            return 200 "$request_method\n";
                        }
                    }
                }
            "#,
        &[("conf/htpasswd", "user:{PLAIN}pass\n")],
    );
    let get = request_body(
        &config,
        "GET /limited HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(
        get.starts_with("HTTP/1.1 200 OK\r\n") && get.ends_with("GET\n"),
        "unexpected unauthenticated GET response:\n{get}"
    );

    let post = request_body(
        &config,
        "POST /limited HTTP/1.1\r\nHost: localhost\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(
        post.starts_with("HTTP/1.1 401 Unauthorized\r\n"),
        "unexpected unauthenticated POST response:\n{post}"
    );
    assert!(
        post.contains("WWW-Authenticate: Basic realm=\"Limited\"\r\n"),
        "unexpected challenge:\n{post}"
    );

    let authenticated = request_body(
        &config,
        "POST /limited HTTP/1.1\r\nHost: localhost\r\nContent-Length: 0\r\nAuthorization: Basic dXNlcjpwYXNz\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(
        authenticated.starts_with("HTTP/1.1 200 OK\r\n") && authenticated.ends_with("POST\n"),
        "unexpected authenticated POST response:\n{authenticated}"
    );
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn applies_access_log_override_from_limit_except() {
    let (prefix, config) = test_config(
        r#"
                worker_processes 1;
                events { worker_connections 64; }
                http {
                    server {
                        listen 127.0.0.1:18187;
                        access_log logs/general.log;
                        location /limited {
                            limit_except GET {
                                access_log logs/limited.log combined if=$arg_log;
                            }
                            return 200 "$request_method\n";
                        }
                    }
                }
            "#,
        &[],
    );
    let get = request_body(
        &config,
        "GET /limited HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(get.ends_with("GET\n"), "{get}");
    let post_without_argument = request_body(
        &config,
        "POST /limited HTTP/1.1\r\nHost: localhost\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(
        post_without_argument.ends_with("POST\n"),
        "{post_without_argument}"
    );
    let post_with_zero = request_body(
        &config,
        "POST /limited?log=0 HTTP/1.1\r\nHost: localhost\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(post_with_zero.ends_with("POST\n"), "{post_with_zero}");
    let post_logged = request_body(
        &config,
        "POST /limited?log=1 HTTP/1.1\r\nHost: localhost\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(post_logged.ends_with("POST\n"), "{post_logged}");

    let general = std::fs::read_to_string(prefix.join("logs/general.log")).unwrap();
    assert!(general.contains("\"GET /limited HTTP/1.1\""), "{general}");
    assert!(!general.contains("\"POST /limited HTTP/1.1\""), "{general}");
    let limited = std::fs::read_to_string(prefix.join("logs/limited.log")).unwrap();
    assert!(
        limited.contains("\"POST /limited?log=1 HTTP/1.1\""),
        "{limited}"
    );
    assert!(
        !limited.contains("\"POST /limited HTTP/1.1\"")
            && !limited.contains("\"POST /limited?log=0 HTTP/1.1\""),
        "{limited}"
    );
    assert!(!limited.contains("\"GET /limited HTTP/1.1\""), "{limited}");
    let _ = std::fs::remove_dir_all(prefix);
}
