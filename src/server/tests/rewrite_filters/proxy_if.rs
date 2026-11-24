use super::*;

async fn serve_uri_upstream(listener: tokio::net::TcpListener) {
    loop {
        let Ok((mut stream, _)) = listener.accept().await else {
            return;
        };
        let mut request = Vec::new();
        let mut byte = [0_u8; 1];
        while !request.ends_with(b"\r\n\r\n") {
            stream.read_exact(&mut byte).await.unwrap();
            request.push(byte[0]);
        }
        let request = String::from_utf8_lossy(&request);
        let uri = request
            .lines()
            .next()
            .and_then(|line| line.split_whitespace().nth(1))
            .unwrap_or("/")
            .split('?')
            .next()
            .unwrap_or("/");
        let body = format!("uri:{uri}\n");
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\n\
             Connection: close\r\n\r\n{body}",
            body.len()
        );
        stream.write_all(response.as_bytes()).await.unwrap();
        stream.shutdown().await.unwrap();
    }
}

#[tokio::test]
async fn proxy_if_and_limit_except_match_nginx_uri_dispatch() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let upstream = listener.local_addr().unwrap();
    let backend = tokio::spawn(serve_uri_upstream(listener));
    let source = format!(
        r#"
            worker_processes 1;
            events {{ worker_connections 64; }}
            http {{
                server {{
                    listen 127.0.0.1:18578;
                    location / {{
                        proxy_pass http://{upstream};
                    }}
                    location /proxy-pass-uri {{
                        proxy_pass http://{upstream}/replacement;
                        if ($arg_if) {{
                        }}
                        location /proxy-pass-uri/inner {{
                            if ($arg_if) {{
                            }}
                        }}
                    }}
                    location /proxy-pass-uri-lmt {{
                        proxy_pass http://{upstream}/replacement;
                        limit_except POST {{
                        }}
                        location /proxy-pass-uri-lmt/inner {{
                            limit_except POST {{
                            }}
                        }}
                    }}
                    location /proxy-pass-uri-lmt-different {{
                        proxy_pass http://{upstream}/replacement;
                        limit_except POST {{
                            proxy_pass http://{upstream};
                        }}
                    }}
                    location /proxy-inside-if-crash {{
                        set $true 1;
                        if ($true) {{
                            proxy_pass http://{upstream};
                        }}
                        if ($true) {{
                        }}
                    }}
                    location /variables {{
                        proxy_pass http://{upstream}/outer/$host;
                        if ($arg_if) {{
                            proxy_pass http://{upstream};
                        }}
                        location /variables/inner {{
                            proxy_pass http://{upstream};
                        }}
                    }}
                }}
            }}
        "#
    );
    let (prefix, config) = test_config(&source, &[]);
    let get = |target: &str| {
        let request =
            format!("GET {target} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n");
        let config = config.clone();
        async move { request_body(&config, &request).await }
    };

    assert!(get("/").await.ends_with("uri:/\n"));
    assert!(get("/proxy-pass-uri").await.ends_with("uri:/replacement\n"));
    let conditional_uri = get("/proxy-pass-uri?if=1").await;
    assert!(
        conditional_uri.ends_with("uri:/replacement\n"),
        "{conditional_uri}"
    );
    assert!(
        get("/proxy-pass-uri/inner")
            .await
            .starts_with("HTTP/1.1 404")
    );
    assert!(
        get("/proxy-pass-uri/inner?if=1")
            .await
            .starts_with("HTTP/1.1 404")
    );
    assert!(
        get("/proxy-pass-uri-lmt")
            .await
            .ends_with("uri:/replacement\n")
    );
    assert!(
        get("/proxy-pass-uri-lmt/inner")
            .await
            .starts_with("HTTP/1.1 404")
    );
    assert!(
        get("/proxy-pass-uri-lmt-different")
            .await
            .ends_with("uri:/proxy-pass-uri-lmt-different\n")
    );
    assert!(
        get("/proxy-inside-if-crash")
            .await
            .starts_with("HTTP/1.1 500")
    );
    assert!(get("/variables").await.ends_with("uri:/outer/localhost\n"));
    assert!(get("/variables?if=1").await.ends_with("uri:/variables\n"));
    assert!(
        get("/variables/inner")
            .await
            .ends_with("uri:/variables/inner\n")
    );

    backend.abort();
    let _ = backend.await;
    let _ = std::fs::remove_dir_all(prefix);
}
