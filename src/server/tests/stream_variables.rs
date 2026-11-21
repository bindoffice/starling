use super::support::*;
use super::*;

#[test]
fn compiles_stream_map_geo_and_split_clients_variables() {
    let (prefix, config) = test_config(
        r#"
                worker_processes 1;
                events { worker_connections 64; }
                stream {
                    map $remote_addr $mapped {
                        hostnames;
                        default fallback;
                        *.example.com wildcard;
                        ~^203\.0\.113\.(\d+)$ "net-$1";
                    }
                    geo $remote_addr $zone {
                        default outside;
                        203.0.113.0/24 inside;
                    }
                    split_clients "$remote_addr" $bucket {
                        50% one;
                        * two;
                    }
                    server {
                        listen 127.0.0.1:13319;
                        return "$mapped|$zone|$bucket";
                    }
                }
            "#,
        &[],
    );
    let stream = config.stream.as_ref().unwrap();
    let map = stream.maps.get("mapped").unwrap();
    assert!(map.hostnames);
    assert_eq!(map.default.as_deref(), Some("fallback"));
    assert_eq!(map.exact.get("example.com"), None);
    assert_eq!(map.leading_wildcards[0].0, ".example.com");
    assert_eq!(map.regexes.len(), 1);
    assert_eq!(stream.geos["zone"].default.as_deref(), Some("outside"));
    assert_eq!(stream.geos["zone"].networks.len(), 1);
    assert_eq!(stream.split_clients["bucket"].parts.len(), 2);
    let _ = std::fs::remove_dir_all(prefix);
}

#[test]
fn rejects_conflicting_stream_variable_names() {
    let error = test_config_error(
        r#"
                worker_processes 1;
                events { worker_connections 64; }
                stream {
                    map $remote_addr $shared { default map; }
                    geo $remote_addr $shared { default geo; }
                    server {
                        listen 127.0.0.1:13320;
                        return "$shared";
                    }
                }
            "#,
    );
    assert!(error.contains("duplicate geo variable"));

    let error = test_config_error(
        r#"
                worker_processes 1;
                events { worker_connections 64; }
                stream {
                    split_clients "$remote_addr" $bucket { 101% all; }
                    server {
                        listen 127.0.0.1:13321;
                        return "$bucket";
                    }
                }
            "#,
    );
    assert!(error.contains("percent total is greater than 100%"));
}

#[tokio::test]
async fn evaluates_stream_map_geo_split_clients_and_set_variables() {
    let source = r#"
                worker_processes 1;
                events { worker_connections 64; }
                stream {
                    map $site_host $site {
                        hostnames;
                        default other;
                        *.example.com wildcard;
                    }
                    map $remote_addr $mapped {
                        default none;
                        ~^203\.0\.113\.(?<last>\d+)$ "net-$last";
                    }
                    geo $client $zone {
                        default outside;
                        203.0.113.0/24 inside;
                    }
                    geo $client $range_zone {
                        ranges;
                        default outside;
                        203.0.113.1-203.0.113.9 range;
                    }
                    split_clients "203.0.113.7" $bucket {
                        50% one;
                        * two;
                    }
                    server {
                        listen 127.0.0.1:13322;
                        set $site_host "www.example.com";
                        set $client "203.0.113.7";
                        return "$site|$mapped|$zone|$range_zone|$bucket";
                    }
                }
            "#;
    let (prefix, config) = test_config(source, &[]);
    let expected_bucket = if nginx_murmur_hash2(b"203.0.113.7") < u32::MAX / 2 {
        "one"
    } else {
        "two"
    };
    let route = build_stream_routes(&config)[0].clone();
    let state = test_state(&config);
    let remote: SocketAddr = "203.0.113.7:45679".parse().unwrap();
    let addresses = ProxyProtocolAddresses::without_header(&ListenAddress::Inet(remote));
    let (mut client, inbound) = tokio::io::duplex(1024);
    let task = tokio::spawn(async move {
        handle_stream_connection(inbound, &addresses, route, state).await;
    });
    let mut response = Vec::new();
    client.read_to_end(&mut response).await.unwrap();
    assert_eq!(
        std::str::from_utf8(&response).unwrap(),
        format!("wildcard|net-7|inside|range|{expected_bucket}")
    );
    task.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}
