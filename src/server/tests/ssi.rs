use super::support::*;

#[tokio::test]
async fn processes_ssi_echo_set_conditions_and_regex_captures() {
    let (prefix, config) = test_config(
        r#"
                worker_processes 1;
                events { worker_connections 64; }
                http {
                    server {
                        listen 127.0.0.1:18191;
                        root html;
                        ssi on;
                        location = /page.html { }
                    }
                }
            "#,
        &[(
            "html/page.html",
            r#"before
<!--# set var="name" value="world" -->
<!--# echo var="name" -->
<!--# if expr="$arg_kind = /foo(.*)/" -->regex:<!--# echo var="1" --><!--# else -->other<!--# endif -->
<!--# if expr="$arg_kind = foo" -->equal<!--# elif expr="$arg_kind = bar" -->bar<!--# else -->fallback<!--# endif -->
"#,
        )],
    );

    let response = request_bytes(
        &config,
        "GET /page.html?kind=foobar HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    let (headers, body) = split_http_response(&response);
    assert!(headers.starts_with("HTTP/1.1 200 OK\r\n"), "{headers}");
    assert!(headers.contains("Content-Type: text/html"), "{headers}");
    assert!(headers.contains("Content-Length:"), "{headers}");
    assert!(!headers.contains("Accept-Ranges:"), "{headers}");
    assert!(!headers.contains("Last-Modified:"), "{headers}");
    assert!(!headers.contains("ETag:"), "{headers}");
    assert!(String::from_utf8_lossy(body).contains("world"));
    assert!(String::from_utf8_lossy(body).contains("regex:bar"));
    assert!(String::from_utf8_lossy(body).contains("fallback"));
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn includes_virtual_files_and_honors_types_and_escape_encoding() {
    let (prefix, config) = test_config(
        r#"
                worker_processes 1;
                events { worker_connections 64; }
                http {
                    server {
                        listen 127.0.0.1:18192;
                        root html;
                        ssi on;
                        location = /index.html { }
                        location /text/ { ssi_types text/plain; }
                    }
                }
            "#,
        &[
            (
                "html/index.html",
                "start<!--# include virtual=\"fragment.html?name=$arg_name\" -->end\n",
            ),
            (
                "html/fragment.html",
                "fragment<!--# echo var=\"arg_name\" encoding=\"entity\" -->",
            ),
            (
                "html/text/plain.txt",
                "plain<!--# echo var=\"arg_name\" default=\"none\" -->",
            ),
        ],
    );

    let response = request_bytes(
        &config,
        "GET /index.html?name=<tag> HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    let (_, body) = split_http_response(&response);
    assert_eq!(
        String::from_utf8_lossy(body),
        "startfragment&lt;tag&gt;end\n"
    );

    let text = request_bytes(
        &config,
        "GET /text/plain.txt?name=hello HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    let (_, body) = split_http_response(&text);
    assert_eq!(String::from_utf8_lossy(body), "plainhello");
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn supports_blocks_and_include_set() {
    let (prefix, config) = test_config(
        r#"
                worker_processes 1;
                events { worker_connections 64; }
                http {
                    server {
                        listen 127.0.0.1:18193;
                        root html;
                        ssi on;
                        location = /page.html { }
                        location = /capture.html { }
                        location = /source { return 200 "captured-body"; }
                    }
                }
            "#,
        &[
            (
                "html/page.html",
                "A<!--# block name=\"fallback\" -->fallback-body<!--# endblock --> B<!--# include virtual=\"/source\" set=\"captured\" --> C<!--# include virtual=\"/missing\" stub=\"fallback\" -->D E<!--# echo var=\"captured\" -->",
            ),
            ("html/capture.html", "capture.html"),
        ],
    );

    let response = request_bytes(
        &config,
        "GET /page.html HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    let (_, body) = split_http_response(&response);
    assert_eq!(
        String::from_utf8_lossy(body),
        "A B Cfallback-bodyD Ecaptured-body"
    );
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn processes_return_bodies_with_rewritten_http_variables() {
    let (prefix, config) = test_config(
        r#"
                worker_processes 1;
                events { worker_connections 64; }
                http {
                    server {
                        listen 127.0.0.1:18196;
                        location = /return.html {
                            ssi on;
                            set $http_foo "set-value";
                            return 200 'foo=<!--# echo var="http_foo" -->;connection=<!--# echo var="http_connection" -->';
                        }
                    }
                }
            "#,
        &[],
    );

    let response = request_bytes(
        &config,
        "GET /return.html HTTP/1.1\r\nHost: localhost\r\nFoo: incoming-value\r\nConnection: close\r\n\r\n",
    )
    .await;
    let (headers, body) = split_http_response(&response);
    assert!(headers.starts_with("HTTP/1.1 200 OK\r\n"), "{headers}");
    assert!(headers.contains("Content-Type: text/html"), "{headers}");
    assert_eq!(
        String::from_utf8_lossy(body),
        "foo=set-value;connection=close"
    );
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn distinguishes_empty_arguments_and_exposes_dates_and_regex_captures() {
    let (prefix, config) = test_config(
        r#"
                worker_processes 1;
                events { worker_connections 64; }
                http {
                    server {
                        listen 127.0.0.1:18197;
                        root html;
                        ssi on;
                        location = /empty.html { }
                        location = /capture.html { }
                        location = /date.html {
                            add_header X-Var x${date_gmt}x;
                            return 200;
                        }
                    }
                }
            "#,
        &[
            ("html/empty.html", "X<!--#echo var=\"arg_test\" -->X"),
            (
                "html/capture.html",
                "x<!--#if expr=\"$arg_v = /(CAP\\d).*(CAP\\d)/\" --><!--#echo var=\"1\" -->x<!--#echo var=\"2\" --><!--#endif -->x",
            ),
        ],
    );

    let empty = request_bytes(
        &config,
        "GET /empty.html?test= HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    let (_, body) = split_http_response(&empty);
    assert_eq!(String::from_utf8_lossy(body), "XX");

    let captures = request_bytes(
        &config,
        "GET /capture.html?v=hereCAP1andCAP2 HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    let (_, body) = split_http_response(&captures);
    assert_eq!(String::from_utf8_lossy(body), "xCAP1xCAP2x");

    let date = request_bytes(
        &config,
        "GET /date.html HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    let (headers, _) = split_http_response(&date);
    assert!(headers.contains("X-Var: x"), "{headers}");
    assert!(headers.contains(','), "{headers}");
    assert!(headers.contains(" GMTx"), "{headers}");
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn handles_errors_and_last_modified_policy() {
    let (prefix, config) = test_config(
        r#"
                worker_processes 1;
                events { worker_connections 64; }
                http {
                    server {
                        listen 127.0.0.1:18195;
                        root html;
                        ssi on;
                        location = /silent.html { ssi_silent_errors on; }
                        location = /weak.html { ssi_last_modified on; }
                    }
                }
            "#,
        &[
            ("html/error.html", "before<!--# unknown -->after"),
            ("html/silent.html", "before<!--# unknown -->after"),
            ("html/weak.html", "before<!--# echo var=\"host\" -->after"),
        ],
    );

    let error = request_bytes(
        &config,
        "GET /error.html HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    let (_, body) = split_http_response(&error);
    assert_eq!(
        String::from_utf8_lossy(body),
        "before[an error occurred while processing the directive]after"
    );

    let silent = request_bytes(
        &config,
        "GET /silent.html HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    let (_, body) = split_http_response(&silent);
    assert_eq!(String::from_utf8_lossy(body), "beforeafter");

    let weak = request_bytes(
        &config,
        "GET /weak.html HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    let (headers, body) = split_http_response(&weak);
    assert_eq!(String::from_utf8_lossy(body), "beforelocalhostafter");
    assert!(headers.contains("Last-Modified:"), "{headers}");
    assert!(headers.contains("ETag: W/\""), "{headers}");
    let _ = std::fs::remove_dir_all(prefix);
}

#[test]
fn compiles_ssi_inheritance_and_validation() {
    let (prefix, config) = test_config(
        r#"
                worker_processes 1;
                events { worker_connections 64; }
                http {
                    ssi on;
                    ssi_types text/html text/plain;
                    server {
                        listen 127.0.0.1:18194;
                        ssi_value_length 512;
                        location /inherit/ { }
                        location /off/ { ssi off; }
                    }
                }
            "#,
        &[],
    );
    let server = &config.http.as_ref().unwrap().servers[0];
    assert!(server.context.ssi.enabled);
    assert_eq!(server.context.ssi.types, ["text/html", "text/plain"]);
    assert_eq!(server.context.ssi.value_length, 512);
    assert!(server.locations[0].context.ssi.enabled);
    assert!(!server.locations[1].context.ssi.enabled);

    let duplicate = test_config_error(
        r#"
                events {}
                http {
                    server {
                        listen 127.0.0.1:18194;
                        ssi on;
                        ssi off;
                    }
                }
            "#,
    );
    assert!(
        duplicate.contains("\"ssi\" directive is duplicate"),
        "{duplicate}"
    );
    let _ = std::fs::remove_dir_all(prefix);
}
