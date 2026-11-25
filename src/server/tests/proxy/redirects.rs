use super::*;

#[test]
fn builds_default_proxy_redirect_rules() {
    let (prefix, config) = test_config(
        r#"
                worker_processes 1;
                events { worker_connections 64; }
                http {
                    server {
                        listen 127.0.0.1:18096;
                        location /mapped/ {
                            proxy_pass http://127.0.0.1:19000/upstream/;
                        }
                        location /passthrough/ {
                            proxy_pass http://127.0.0.1:19000;
                        }
                        location ~ ^/regex-passthrough/ {
                            proxy_pass http://127.0.0.1:19000;
                        }
                    }
                }
            "#,
        &[],
    );
    let http = config.http.as_ref().unwrap();
    let server = &http.servers[0];
    let request = Request {
        method: "GET".into(),
        target: "/".into(),
        version: "HTTP/1.1".into(),
        headers: (vec![Header {
            name: "Host".into(),
            value: "localhost".into(),
        }])
        .into(),
        body_stream: None,
        body: Vec::new(),
        remote_addr: "127.0.0.1:1234".into(),
        local_addr: "127.0.0.1:18096".into(),
        keep_alive: false,
    };
    let remote: SocketAddr = "127.0.0.1:1234".parse().unwrap();
    let local: SocketAddr = "127.0.0.1:18096".parse().unwrap();
    let variables = Variables::new(&request, "localhost", server, &local, &remote);

    let mapped = &server.locations[0];
    assert_eq!(
        proxy_redirect_value(
            "http://127.0.0.1:19000/upstream/file",
            mapped.context.proxy_pass.as_ref().unwrap(),
            Some(mapped),
            &mapped.context,
            &variables,
        )
        .as_deref(),
        Some("/mapped/file")
    );

    let passthrough = &server.locations[1];
    assert_eq!(
        proxy_redirect_value(
            "http://127.0.0.1:19000/file",
            passthrough.context.proxy_pass.as_ref().unwrap(),
            Some(passthrough),
            &passthrough.context,
            &variables,
        )
        .as_deref(),
        Some("/file")
    );

    let regex = &server.locations[2];
    assert_eq!(
        proxy_redirect_value(
            "http://127.0.0.1:19000/file",
            regex.context.proxy_pass.as_ref().unwrap(),
            Some(regex),
            &regex.context,
            &variables,
        )
        .as_deref(),
        Some("/file")
    );
    let _ = std::fs::remove_dir_all(prefix);
}

#[test]
fn applies_explicit_proxy_redirect_rules_to_location_and_refresh() {
    let (prefix, config) = test_config(
        r#"
                worker_processes 1;
                events { worker_connections 64; }
                http {
                    server {
                        listen 127.0.0.1:18097;
                        location /off/ {
                            proxy_pass http://127.0.0.1:19000/upstream/;
                            proxy_redirect off;
                        }
                        location /prefix/ {
                            proxy_pass http://127.0.0.1:19000;
                            proxy_redirect http://backend.test/api/ /frontend/;
                        }
                        location /regex/ {
                            proxy_pass http://127.0.0.1:19000;
                            proxy_redirect ~^http://([^/]+)/old/(.*)$ https://$1/new/$2;
                        }
                        location /regex-case/ {
                            proxy_pass http://127.0.0.1:19000;
                            proxy_redirect ~*^http://backend.test/legacy/(.*)$ /modern/$1;
                        }
                        location /multi/ {
                            proxy_pass http://backend.test/;
                            proxy_redirect http://backend.test/special/ /override/;
                            proxy_redirect default;
                        }
                    }
                }
            "#,
        &[],
    );
    let http = config.http.as_ref().unwrap();
    let server = &http.servers[0];
    let request = Request {
        method: "GET".into(),
        target: "/".into(),
        version: "HTTP/1.1".into(),
        headers: (vec![Header {
            name: "Host".into(),
            value: "localhost".into(),
        }])
        .into(),
        body_stream: None,
        body: Vec::new(),
        remote_addr: "127.0.0.1:1234".into(),
        local_addr: "127.0.0.1:18097".into(),
        keep_alive: false,
    };
    let remote: SocketAddr = "127.0.0.1:1234".parse().unwrap();
    let local: SocketAddr = "127.0.0.1:18097".parse().unwrap();
    let variables = Variables::new(&request, "localhost", server, &local, &remote);

    let off = &server.locations[0];
    assert_eq!(
        proxy_redirect_value(
            "http://127.0.0.1:19000/upstream/file",
            off.context.proxy_pass.as_ref().unwrap(),
            Some(off),
            &off.context,
            &variables,
        ),
        None
    );

    let prefix_rule = &server.locations[1];
    assert_eq!(
        proxy_redirect_value(
            "http://backend.test/api/file",
            prefix_rule.context.proxy_pass.as_ref().unwrap(),
            Some(prefix_rule),
            &prefix_rule.context,
            &variables,
        )
        .as_deref(),
        Some("/frontend/file")
    );
    assert_eq!(
        proxy_redirect_value(
            "http://BACKEND.test/api/file",
            prefix_rule.context.proxy_pass.as_ref().unwrap(),
            Some(prefix_rule),
            &prefix_rule.context,
            &variables,
        ),
        None
    );

    let regex_rule = &server.locations[2];
    assert_eq!(
        proxy_redirect_value(
            "http://upstream.test/old/a/b",
            regex_rule.context.proxy_pass.as_ref().unwrap(),
            Some(regex_rule),
            &regex_rule.context,
            &variables,
        )
        .as_deref(),
        Some("https://upstream.test/new/a/b")
    );

    let regex_case_rule = &server.locations[3];
    assert_eq!(
        proxy_redirect_value(
            "HTTP://BACKEND.TEST/legacy/file",
            regex_case_rule.context.proxy_pass.as_ref().unwrap(),
            Some(regex_case_rule),
            &regex_case_rule.context,
            &variables,
        )
        .as_deref(),
        Some("/modern/file")
    );

    let multi_rule = &server.locations[4];
    assert_eq!(
        proxy_redirect_value(
            "http://backend.test/special/file",
            multi_rule.context.proxy_pass.as_ref().unwrap(),
            Some(multi_rule),
            &multi_rule.context,
            &variables,
        )
        .as_deref(),
        Some("/override/file")
    );
    assert_eq!(
        proxy_redirect_value(
            "http://backend.test/other/file",
            multi_rule.context.proxy_pass.as_ref().unwrap(),
            Some(multi_rule),
            &multi_rule.context,
            &variables,
        )
        .as_deref(),
        Some("/multi/other/file")
    );

    let prefix_rule = &server.locations[1];
    let mut response = Response {
        status: 302,
        headers: vec![
            Header {
                name: "Location".into(),
                value: "http://backend.test/api/file".into(),
            },
            Header {
                name: "Refresh".into(),
                value: "0; URL=http://backend.test/api/file".into(),
            },
        ],
        trailers: Vec::new(),
        chunked_expected: false,
        body: ResponseBody::Empty,
        body_temp_file: None,
        upstream: None,
        builtin_error: false,
        msie_padding: false,
        allow_ranges: false,
        single_range: false,
    };
    apply_proxy_redirect(
        &mut response,
        prefix_rule.context.proxy_pass.as_ref().unwrap(),
        Some(prefix_rule),
        &prefix_rule.context,
        &variables,
    );
    assert_eq!(response.headers[0].value, "/frontend/file");
    assert_eq!(response.headers[1].value, "0; URL=/frontend/file");
    let _ = std::fs::remove_dir_all(prefix);
}
