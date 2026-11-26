use super::*;

#[test]
fn automatic_https_is_enabled_by_default_and_configurable() {
    let prefix = temp_prefix();
    fs::create_dir_all(prefix.join("conf")).unwrap();
    let config_path = prefix.join("conf/nginx.conf");
    fs::write(
        &config_path,
        "events {}\nhttp { server { listen 8080; server_name example.test; } }\n",
    )
    .unwrap();
    let document = ConfigLoader::new(prefix.clone())
        .load(&config_path)
        .unwrap();
    let config = compile(document, prefix.clone(), config_path.clone()).unwrap();
    assert!(config.auto_https.enabled);
    assert_eq!(config.auto_https.http_port, 80);
    assert_eq!(config.auto_https.https_port, 443);
    assert_eq!(config.auto_https.cert_dir, prefix.join("auto_https"));
    assert_eq!(config.auto_https.challenge, AutoHttpsChallenge::Http01);
    assert_eq!(config.auto_https.dns_hook, None);
    assert_eq!(
        config.auto_https.dns_propagation_delay,
        Duration::from_secs(30)
    );

    fs::write(
        &config_path,
        r#"
            auto_https off;
            events {}
            http {
                server {
                    listen 127.0.0.1:18080;
                    server_name example.com;
                }
            }
        "#,
    )
    .unwrap();
    let document = ConfigLoader::new(prefix.clone())
        .load(&config_path)
        .unwrap();
    let config = compile(document, prefix.clone(), config_path).unwrap();
    assert!(!config.auto_https.enabled);
    let _ = fs::remove_dir_all(prefix);
}

#[test]
fn automatic_https_configures_dns01() {
    let prefix = temp_prefix();
    fs::create_dir_all(prefix.join("conf")).unwrap();
    let config_path = prefix.join("conf/nginx.conf");
    fs::write(
        &config_path,
        r#"
            auto_https on cert_dir=acme challenge=dns-01 dns_hook=hooks/publish
                dns_propagation_delay=45s;
            events {}
            http {
                server {
                    listen 127.0.0.1:18080;
                    server_name *.starling.dev;
                }
            }
        "#,
    )
    .unwrap();
    let document = ConfigLoader::new(prefix.clone())
        .load(&config_path)
        .unwrap();
    let config = compile(document, prefix.clone(), config_path).unwrap();
    assert_eq!(config.auto_https.challenge, AutoHttpsChallenge::Dns01);
    assert_eq!(
        config.auto_https.dns_hook,
        Some(prefix.join("hooks/publish"))
    );
    assert_eq!(
        config.auto_https.dns_propagation_delay,
        Duration::from_secs(45)
    );
    let _ = fs::remove_dir_all(prefix);
}

#[test]
fn automatic_https_dns01_accepts_individual_directives() {
    let prefix = temp_prefix();
    fs::create_dir_all(prefix.join("conf")).unwrap();
    let config_path = prefix.join("conf/nginx.conf");
    fs::write(
        &config_path,
        r#"
            auto_https_challenge dns-01;
            auto_https_dns_hook hooks/dns;
            auto_https_dns_propagation_delay 1m;
            events {}
        "#,
    )
    .unwrap();
    let document = ConfigLoader::new(prefix.clone())
        .load(&config_path)
        .unwrap();
    let config = compile(document, prefix.clone(), config_path).unwrap();
    assert_eq!(config.auto_https.challenge, AutoHttpsChallenge::Dns01);
    assert_eq!(config.auto_https.dns_hook, Some(prefix.join("hooks/dns")));
    assert_eq!(
        config.auto_https.dns_propagation_delay,
        Duration::from_secs(60)
    );
    let _ = fs::remove_dir_all(prefix);
}

#[test]
fn automatic_https_requires_a_dns_hook_for_dns01() {
    let prefix = temp_prefix();
    fs::create_dir_all(prefix.join("conf")).unwrap();
    let config_path = prefix.join("conf/nginx.conf");
    fs::write(
        &config_path,
        r#"
            auto_https on challenge=dns-01;
            events {}
        "#,
    )
    .unwrap();
    let document = ConfigLoader::new(prefix.clone())
        .load(&config_path)
        .unwrap();
    let error = compile(document, prefix.clone(), config_path)
        .expect_err("DNS-01 without a hook should fail");
    assert!(error.to_string().contains("requires a dns_hook"));
    let _ = fs::remove_dir_all(prefix);
}

#[test]
fn automatic_https_accepts_production_and_testing_options() {
    let prefix = temp_prefix();
    fs::create_dir_all(prefix.join("conf")).unwrap();
    let config_path = prefix.join("conf/nginx.conf");
    fs::write(
        &config_path,
        r#"
            auto_https on cert_dir=acme email=admin@example.com staging=on redirect=off
                acme_directory=http://127.0.0.1:14000/dir acme_ca_file=pebble-ca.crt
                http_port=8080 https_port=8443;
            events {}
        "#,
    )
    .unwrap();
    let document = ConfigLoader::new(prefix.clone())
        .load(&config_path)
        .unwrap();
    let config = compile(document, prefix.clone(), config_path).unwrap();
    assert!(config.auto_https.enabled);
    assert_eq!(config.auto_https.cert_dir, prefix.join("acme"));
    assert_eq!(
        config.auto_https.email.as_deref(),
        Some("admin@example.com")
    );
    assert!(config.auto_https.staging);
    assert!(!config.auto_https.redirect_http);
    assert_eq!(
        config.auto_https.acme_directory_url(),
        "http://127.0.0.1:14000/dir"
    );
    assert_eq!(
        config.auto_https.acme_ca_file,
        Some(prefix.join("pebble-ca.crt"))
    );
    assert_eq!(config.auto_https.http_port, 8080);
    assert_eq!(config.auto_https.https_port, 8443);
    let _ = fs::remove_dir_all(prefix);
}

#[test]
fn automatic_https_rejects_matching_http_and_https_ports() {
    let prefix = temp_prefix();
    fs::create_dir_all(prefix.join("conf")).unwrap();
    let config_path = prefix.join("conf/nginx.conf");
    fs::write(
        &config_path,
        r#"
            auto_https_http_port 8443;
            auto_https_https_port 8443;
            events {}
        "#,
    )
    .unwrap();
    let document = ConfigLoader::new(prefix.clone())
        .load(&config_path)
        .unwrap();
    let error = compile(document, prefix.clone(), config_path)
        .expect_err("matching automatic HTTPS ports should fail");
    assert!(
        error
            .to_string()
            .contains("http_port and https_port must be different")
    );
    let _ = fs::remove_dir_all(prefix);
}
