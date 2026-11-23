use super::support::*;
use super::*;

#[test]
fn applies_proxy_cookie_flags_by_cookie_name_and_regex() {
    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            http {
                server {
                    listen 127.0.0.1:18110;
                    location /flags {
                        proxy_cookie_flags session secure httponly samesite=lax;
                        proxy_cookie_flags token nosecure nohttponly nosamesite;
                        proxy_cookie_flags ~^api_ samesite=none;
                    }
                }
            }
        "#,
        &[],
    );
    let response = apply_cookie_rewrites(
        &config,
        "/flags",
        &[
            "session=abc; Secure; SameSite=strict; Path=/",
            "token=1; secure; HTTPONLY; samesite=Strict; Path=/token",
            "API_token=x; Path=/",
            "other=x; Secure",
            "session=2",
        ],
    );
    assert_eq!(
        response.headers[0].value,
        "session=abc; Secure; SameSite=Lax; Path=/; HttpOnly"
    );
    assert_eq!(response.headers[1].value, "token=1; Path=/token");
    assert_eq!(
        response.headers[2].value,
        "API_token=x; Path=/; SameSite=None"
    );
    assert_eq!(response.headers[3].value, "other=x; Secure");
    assert_eq!(
        response.headers[4].value,
        "session=2; Secure; HttpOnly; SameSite=Lax"
    );
    let _ = std::fs::remove_dir_all(prefix);
}

#[test]
fn evaluates_proxy_cookie_flag_variables_and_first_matching_rule() {
    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            http {
                server {
                    listen 127.0.0.1:18111;
                    location /variables {
                        proxy_cookie_flags $arg_cookie $arg_flag;
                    }
                    location /first-match {
                        proxy_cookie_flags ~^api_ secure;
                        proxy_cookie_flags api_session nohttponly;
                    }
                }
            }
        "#,
        &[],
    );
    let mut variables = apply_cookie_rewrites(
        &config,
        "/variables?cookie=Session&flag=secure",
        &["SESSION=abc; Path=/"],
    );
    assert_eq!(variables.headers[0].value, "SESSION=abc; Path=/; Secure");

    let unknown = apply_cookie_rewrites(
        &config,
        "/variables?cookie=session&flag=unrecognized",
        &["session=abc; HttpOnly"],
    );
    assert_eq!(unknown.headers[0].value, "session=abc; HttpOnly");

    variables = apply_cookie_rewrites(&config, "/first-match", &["api_session=x; HttpOnly"]);
    assert_eq!(
        variables.headers[0].value,
        "api_session=x; HttpOnly; Secure"
    );
    let _ = std::fs::remove_dir_all(prefix);
}

#[test]
fn combines_proxy_cookie_domain_path_and_flag_rewrites() {
    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            http {
                server {
                    listen 127.0.0.1:18112;
                    location /combined {
                        proxy_cookie_domain backend.test frontend.test;
                        proxy_cookie_path /api /service;
                        proxy_cookie_flags session secure;
                    }
                }
            }
        "#,
        &[],
    );
    let response = apply_cookie_rewrites(
        &config,
        "/combined",
        &["session=abc; Domain=.backend.test; Path=/api/file; HttpOnly"],
    );
    assert_eq!(
        response.headers[0].value,
        "session=abc; Domain=.frontend.test; Path=/service/file; HttpOnly; Secure"
    );
    let _ = std::fs::remove_dir_all(prefix);
}

fn apply_cookie_rewrites(config: &Arc<RuntimeConfig>, target: &str, cookies: &[&str]) -> Response {
    let server = &config.http.as_ref().unwrap().servers[0];
    let location = match target.split('?').next().unwrap() {
        "/variables" => &server.locations[0],
        "/first-match" => &server.locations[1],
        "/combined" => &server.locations[0],
        "/flags" => &server.locations[0],
        _ => panic!("unknown test location"),
    };
    let request = Request {
        method: "GET".into(),
        target: target.into(),
        version: "HTTP/1.1".into(),
        headers: vec![Header {
            name: "Host".into(),
            value: "localhost".into(),
        }],
        body_stream: None,
        body: Vec::new(),
        remote_addr: "127.0.0.1:1234".into(),
        local_addr: "127.0.0.1:18110".into(),
        keep_alive: false,
    };
    let remote: SocketAddr = "127.0.0.1:1234".parse().unwrap();
    let local: SocketAddr = "127.0.0.1:18110".parse().unwrap();
    let variables = Variables::new(&request, "localhost", server, &local, &remote);
    let mut response = Response {
        status: 200,
        headers: cookies
            .iter()
            .map(|value| Header {
                name: "Set-Cookie".into(),
                value: (*value).into(),
            })
            .collect(),
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
    apply_proxy_cookie_rewrites(&mut response, &location.context, &variables);
    response
}
