use super::*;

#[test]
fn rewrites_proxy_cookie_domains_paths_and_regexes() {
    let (prefix, config) = test_config(
        r#"
                worker_processes 1;
                events { worker_connections 64; }
                http {
                    server {
                        listen 127.0.0.1:18101;
                        location /cookies/ {
                            proxy_pass http://127.0.0.1:19000;
                            proxy_cookie_domain backend.test frontend.test;
                            proxy_cookie_domain ~^api\.(.+)$ $1;
                            proxy_cookie_domain ~^(.+)\.com$ $1.org;
                            proxy_cookie_path /api /service;
                            proxy_cookie_path ~^/old/(.*)$ /new/$1;
                        }
                        location /cookies-off/ {
                            proxy_pass http://127.0.0.1:19000;
                            proxy_cookie_domain off;
                            proxy_cookie_path off;
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
        headers: vec![Header {
            name: "Host".into(),
            value: "localhost".into(),
        }],
        body_stream: None,
        body: Vec::new(),
        remote_addr: "127.0.0.1:1234".into(),
        local_addr: "127.0.0.1:18101".into(),
        keep_alive: false,
    };
    let remote: SocketAddr = "127.0.0.1:1234".parse().unwrap();
    let local: SocketAddr = "127.0.0.1:18101".parse().unwrap();
    let variables = Variables::new(&request, "localhost", server, &local, &remote);
    let headers = || {
        vec![
            Header {
                name: "Set-Cookie".into(),
                value: "session=abc; Domain=.backend.test; Path=/api/file; HttpOnly".into(),
            },
            Header {
                name: "Set-Cookie".into(),
                value: "token=x; Domain=backend.test; Path=/old/item; Secure".into(),
            },
            Header {
                name: "Set-Cookie".into(),
                value: "api=y; Domain=api.backend.test; Path=/other".into(),
            },
            Header {
                name: "Set-Cookie".into(),
                value: "mixed=z; Domain=www.example.COM; Path=/mixed".into(),
            },
        ]
    };

    let mut response = Response {
        status: 200,
        headers: headers(),
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
    apply_proxy_cookie_rewrites(&mut response, &server.locations[0].context, &variables);
    assert_eq!(
        response.headers[0].value,
        "session=abc; Domain=.frontend.test; Path=/service/file; HttpOnly"
    );
    assert_eq!(
        response.headers[1].value,
        "token=x; Domain=frontend.test; Path=/new/item; Secure"
    );
    assert_eq!(
        response.headers[2].value,
        "api=y; Domain=backend.test; Path=/other"
    );
    assert_eq!(
        response.headers[3].value,
        "mixed=z; Domain=www.example.org; Path=/mixed"
    );

    let mut untouched = Response {
        status: 200,
        headers: headers(),
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
    apply_proxy_cookie_rewrites(&mut untouched, &server.locations[1].context, &variables);
    assert_eq!(
        untouched.headers[0].value,
        "session=abc; Domain=.backend.test; Path=/api/file; HttpOnly"
    );
    let _ = std::fs::remove_dir_all(prefix);
}
