use super::*;

#[test]
fn matches_nginx_murmur_hash2_vectors() {
    let vectors = [
        ("", 0),
        ("a", 2_456_313_694),
        ("abc", 324_500_635),
        ("example.test", 1_227_660_559),
        ("127.0.0.1AAA", 3_053_215_307),
        ("hello world", 1_151_865_881),
    ];
    for (value, expected) in vectors {
        assert_eq!(
            nginx_murmur_hash2(value.as_bytes()),
            expected,
            "unexpected hash for {value:?}"
        );
    }
}

#[tokio::test]
async fn assigns_split_clients_buckets_from_deterministic_hashes() {
    let (prefix, config) = test_config(
        r#"
                worker_processes 1;
                events { worker_connections 64; }
                http {
                    map $remote_addr $split_source {
                        default "${remote_addr}AAA";
                    }
                    split_clients "${remote_addr}AAA" $half {
                        50% low;
                        * high;
                    }
                    split_clients "${remote_addr}AAA" $eighty {
                        80% eighty;
                        * rest;
                    }
                    split_clients $remote_addr $all {
                        * only;
                    }
                    split_clients $split_source $chained {
                        50% low;
                        * high;
                    }
                    server {
                        listen 127.0.0.1:18107;
                        location / {
                            return 200 "$half|$eighty|$all|$chained\n";
                        }
                    }
                }
            "#,
        &[],
    );
    let response = request_body(
        &config,
        "GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(
        response.ends_with("high|eighty|only|high\n"),
        "unexpected split_clients response:\n{response}"
    );
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn returns_geo_and_split_client_values_without_recursive_expansion() {
    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            http {
                geo $remote_addr $site { default $literal_site; }
                split_clients $remote_addr $variant { * $literal_variant; }
                server {
                    listen 127.0.0.1:18136;
                    location / { return 200 "$site|$variant\n"; }
                }
            }
        "#,
        &[],
    );
    let response = request_body(
        &config,
        "GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(
        response.ends_with("$literal_site|$literal_variant\n"),
        "unexpected response:\n{response}"
    );
    let _ = std::fs::remove_dir_all(prefix);
}
