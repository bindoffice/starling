use super::*;

#[tokio::test]
async fn evaluates_map_variables() {
    let (prefix, config) = test_config(
        r#"
                worker_processes 1;
                events { worker_connections 64; }
                http {
                    map $http_x_mode $mode {
                        volatile;
                        default basic;
                        SPECIAL mapped;
                    }
                    map $mode $derived {
                        default inactive;
                        mapped active;
                    }
                    map $http_host $site {
                        hostnames;
                        example.com exact;
                        *.example.org wildcard;
                        .dot.example.org special-wildcard;
                        default other;
                    }
                    map $arg_code $label {
                        default none;
                        "~^a(\d+)$" "code-$1";
                    }
                    server {
                        listen 127.0.0.1:18089;
                        location / {
                            return 200 "$mode|$derived|$site|$label\n";
                        }
                    }
                }
            "#,
        &[],
    );
    let mapped = request_body(
            &config,
            "GET /?code=a42 HTTP/1.1\r\nHost: www.example.org.\r\nX-Mode: special\r\nConnection: close\r\n\r\n",
        )
        .await;
    assert!(
        mapped.ends_with("mapped|active|wildcard|code-42\n"),
        "unexpected response:\n{mapped}"
    );

    let special_wildcard = request_body(
        &config,
        "GET /?code=a42 HTTP/1.1\r\nHost: www.dot.example.org.\r\nX-Mode: special\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(
        special_wildcard.ends_with("mapped|active|special-wildcard|code-42\n"),
        "unexpected response:\n{special_wildcard}"
    );

    let defaults = request_body(
        &config,
        "GET / HTTP/1.1\r\nHost: example.com\r\nX-Mode: unknown\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(
        defaults.ends_with("basic|inactive|exact|none\n"),
        "unexpected response:\n{defaults}"
    );
    let _ = std::fs::remove_dir_all(prefix);
}
