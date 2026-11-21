use super::*;

#[test]
fn compiles_map_variables_and_rejects_duplicate_defaults() {
    let prefix = temp_prefix();
    fs::create_dir_all(prefix.join("conf")).unwrap();
    let config_path = prefix.join("conf/nginx.conf");
    fs::write(
        &config_path,
        r#"
            events {}
            http {
                map $http_host $site {
                    hostnames;
                    default other;
                    example.com exact;
                    *.example.org wildcard;
                }
                server {
                    listen 127.0.0.1:18091;
                    location / { return 200 "$site\n"; }
                }
            }
        "#,
    )
    .unwrap();
    let document = ConfigLoader::new(prefix.clone())
        .load(&config_path)
        .unwrap();
    let config = compile(document, prefix.clone(), config_path.clone()).unwrap();
    let map = config.http.as_ref().unwrap().maps.get("site").unwrap();
    assert!(map.hostnames);
    assert_eq!(map.default.as_deref(), Some("other"));
    assert_eq!(
        map.exact.get("example.com").map(String::as_str),
        Some("exact")
    );
    assert_eq!(map.leading_wildcards[0].0, ".example.org");

    fs::write(
        &config_path,
        "events {} http { map $host $x { default one; default two; } }\n",
    )
    .unwrap();
    let document = ConfigLoader::new(prefix.clone())
        .load(&config_path)
        .unwrap();
    let error = compile(document, prefix.clone(), config_path.clone()).unwrap_err();
    assert!(error.message.contains("duplicate default map parameter"));
    let _ = fs::remove_dir_all(prefix);
}

#[test]
fn compiles_geo_networks_and_ranges() {
    let prefix = temp_prefix();
    fs::create_dir_all(&prefix).unwrap();
    let config_path = prefix.join("nginx.conf");
    fs::write(
        &config_path,
        r#"
            events {}
            http {
                geo $http_x_forwarded_for $site {
                    default other;
                    10.0.0.0/8 internal;
                    192.0.2.1 exact;
                }
                server {
                    listen 127.0.0.1:18104;
                    location / { return 200 "$site\n"; }
                }
            }
        "#,
    )
    .unwrap();
    let document = ConfigLoader::new(prefix.clone())
        .load(&config_path)
        .unwrap();
    let config = compile(document, prefix.clone(), config_path.clone()).unwrap();
    let geo = config.http.as_ref().unwrap().geos.get("site").unwrap();
    assert_eq!(geo.source, "$http_x_forwarded_for");
    assert_eq!(geo.default.as_deref(), Some("other"));
    assert_eq!(geo.networks[0].0.prefix, 32);
    assert_eq!(geo.networks[1].0.prefix, 8);

    fs::write(
        &config_path,
        "events {} http { geo $remote_addr $site { ranges; default local; 10.0.0.1-10.0.0.9 internal; } }\n",
    )
    .unwrap();
    let document = ConfigLoader::new(prefix.clone())
        .load(&config_path)
        .unwrap();
    let config = compile(document, prefix.clone(), config_path.clone()).unwrap();
    let geo = config.http.as_ref().unwrap().geos.get("site").unwrap();
    assert!(geo.ranges);
    assert_eq!(geo.range_entries.len(), 1);
    assert_eq!(geo.range_entries[0].1, "internal");

    fs::write(
        &config_path,
        "events {} http { geo $remote_addr $site { proxy 10.0.0.0/8; default local; } }\n",
    )
    .unwrap();
    let document = ConfigLoader::new(prefix.clone())
        .load(&config_path)
        .unwrap();
    let error = compile(document, prefix.clone(), config_path.clone()).unwrap_err();
    assert!(error.message.contains("not implemented in \"geo\" context"));
    let _ = fs::remove_dir_all(prefix);
}

#[test]
fn compiles_real_ip_trust_and_header_configuration() {
    let prefix = temp_prefix();
    fs::create_dir_all(&prefix).unwrap();
    let config_path = prefix.join("nginx.conf");
    fs::write(
        &config_path,
        r#"
            events {}
            http {
                set_real_ip_from 10.0.0.0/8;
                real_ip_header X-Forwarded-For;
                real_ip_recursive on;
                server {
                    listen 127.0.0.1:18106;
                    location / {
                        set_real_ip_from 192.0.2.0/24;
                        return 204;
                    }
                }
            }
        "#,
    )
    .unwrap();
    let document = ConfigLoader::new(prefix.clone())
        .load(&config_path)
        .unwrap();
    let config = compile(document, prefix.clone(), config_path.clone()).unwrap();
    let server = &config.http.as_ref().unwrap().servers[0];
    assert_eq!(server.context.real_ip_from.len(), 1);
    assert_eq!(server.context.real_ip_from[0].network().unwrap().prefix, 8);
    assert_eq!(server.context.real_ip_header, "X-Forwarded-For");
    assert!(server.context.real_ip_recursive);
    let location = &server.locations[0];
    assert_eq!(location.context.real_ip_from.len(), 1);
    assert_eq!(
        location.context.real_ip_from[0].network().unwrap().prefix,
        24
    );

    fs::write(
        &config_path,
        r#"
            events {}
            http {
                set_real_ip_from 10.0.0.0/8;
                real_ip_header proxy_protocol;
                server {
                    listen 127.0.0.1:18106;
                }
            }
        "#,
    )
    .unwrap();
    let document = ConfigLoader::new(prefix.clone())
        .load(&config_path)
        .unwrap();
    let config = compile(document, prefix.clone(), config_path.clone()).unwrap();
    let server = &config.http.as_ref().unwrap().servers[0];
    assert_eq!(server.context.real_ip_header, "proxy_protocol");

    fs::write(
        &config_path,
        r#"
            events {}
            http {
                set_real_ip_from unix:;
                server {
                    listen 127.0.0.1:18106;
                }
            }
        "#,
    )
    .unwrap();
    let document = ConfigLoader::new(prefix.clone())
        .load(&config_path)
        .unwrap();
    let config = compile(document, prefix.clone(), config_path).unwrap();
    let server = &config.http.as_ref().unwrap().servers[0];
    assert!(server.context.real_ip_from[0].is_unix());
    let _ = fs::remove_dir_all(prefix);
}

#[test]
fn compiles_split_clients_and_rejects_invalid_percentages() {
    let prefix = temp_prefix();
    fs::create_dir_all(&prefix).unwrap();
    let config_path = prefix.join("nginx.conf");
    fs::write(
        &config_path,
        r#"
            events {}
            http {
                split_clients "$remote_addr" $variant {
                    50% one;
                    * two;
                }
                server {
                    listen 127.0.0.1:18108;
                    location / { return 200 "$variant\n"; }
                }
            }
        "#,
    )
    .unwrap();
    let document = ConfigLoader::new(prefix.clone())
        .load(&config_path)
        .unwrap();
    let config = compile(document, prefix.clone(), config_path.clone()).unwrap();
    let split = config
        .http
        .as_ref()
        .unwrap()
        .split_clients
        .get("variant")
        .unwrap();
    assert_eq!(split.parts[0].percent, 5_000);
    assert_eq!(split.parts[0].threshold, 2_147_483_647);
    assert_eq!(split.parts[1].percent, 0);

    let cases = [
        (
            "events {} http { split_clients $remote_addr $x { 0% none; * all; } }\n",
            "invalid percent value",
        ),
        (
            "events {} http { split_clients $remote_addr $x { 60% one; 60% two; } }\n",
            "percent total is greater than 100%",
        ),
        (
            "events {} http { split_clients $remote_addr $x { 10.555% one; * two; } }\n",
            "invalid percent value",
        ),
    ];
    for (source, expected) in cases {
        fs::write(&config_path, source).unwrap();
        let document = ConfigLoader::new(prefix.clone())
            .load(&config_path)
            .unwrap();
        let error = compile(document, prefix.clone(), config_path.clone()).unwrap_err();
        assert!(
            error.message.contains(expected),
            "unexpected error: {}",
            error.message
        );
    }
    let _ = fs::remove_dir_all(prefix);
}
