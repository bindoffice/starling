use super::*;

fn compile_mail(source: &str) -> starling::config::RuntimeConfig {
    let prefix = temp_prefix();
    fs::create_dir_all(&prefix).unwrap();
    let config_path = prefix.join("nginx.conf");
    fs::write(&config_path, source).unwrap();
    let document = ConfigLoader::new(prefix.clone())
        .load(&config_path)
        .unwrap();
    let config =
        compile(document, prefix.clone(), config_path).expect("compile mail configuration");
    let _ = fs::remove_dir_all(prefix);
    config
}

fn mail_error(source: &str) -> String {
    let prefix = temp_prefix();
    fs::create_dir_all(&prefix).unwrap();
    let config_path = prefix.join("nginx.conf");
    fs::write(&config_path, source).unwrap();
    let document = ConfigLoader::new(prefix.clone())
        .load(&config_path)
        .unwrap();
    let error = compile(document, prefix.clone(), config_path)
        .expect_err("mail configuration should fail")
        .to_string();
    let _ = fs::remove_dir_all(prefix);
    error
}

#[test]
fn compiles_pop3_imap_and_smtp_servers() {
    let config = compile_mail(
        r#"
            events {}
            mail {
                proxy_pass_error_message on;
                proxy_timeout 15s;
                auth_http http://127.0.0.1:8080/mail/auth;
                xclient off;

                server {
                    listen 127.0.0.1:8025;
                    protocol smtp;
                    smtp_auth login plain none cram-md5 external;
                }

                server {
                    listen 127.0.0.1:8110;
                    protocol pop3;
                    pop3_auth plain apop cram-md5 external;
                }

                server {
                    listen 127.0.0.1:8143;
                    protocol imap;
                    imap_auth plain cram-md5 external;
                }
            }
        "#,
    );
    let mail = config.mail.expect("mail context");
    assert_eq!(mail.servers.len(), 3);
    assert_eq!(mail.servers[0].protocol.as_str(), "smtp");
    assert!(mail.servers[0].has_auth_method(starling::config::MailAuthMethod::None));
    assert_eq!(mail.servers[1].protocol.as_str(), "pop3");
    assert!(mail.servers[1].has_auth_method(starling::config::MailAuthMethod::Apop));
    assert_eq!(mail.servers[2].protocol.as_str(), "imap");
}

#[test]
fn inherits_mail_defaults_and_parses_tls_starttls() {
    let config = compile_mail(
        r#"
            events {}
            mail {
                auth_http http://localhost:8080/auth;
                auth_http_header X-Application mail;
                timeout 45s;
                max_errors 7;
                proxy_protocol v2;
                set_real_ip_from 127.0.0.1/32;

                server {
                    listen 127.0.0.1:8465 ssl;
                    protocol smtp;
                    server_name mail.example.test;
                    ssl_certificate cert.pem;
                    ssl_certificate_key key.pem;
                    ssl_certificate cert2.pem;
                    ssl_certificate_key key2.pem;
                    ssl_session_cache shared:MAIL:1m;
                    ssl_session_timeout 10m;
                    ssl_session_tickets off;
                    starttls only;
                    set_real_ip_from 192.0.2.0/24;
                }
            }
        "#,
    );
    let mail = config.mail.expect("mail context");
    let server = &mail.servers[0];
    assert_eq!(server.timeout, Duration::from_secs(45));
    assert_eq!(server.max_errors, 7);
    assert_eq!(server.server_name, "mail.example.test");
    assert_eq!(server.tls.certificates.len(), 2);
    assert_eq!(server.tls.certificate_keys.len(), 2);
    assert_eq!(
        server.tls.session_cache,
        starling::config::MailSessionCache::Shared {
            name: "MAIL".into(),
            size: 1024 * 1024,
        }
    );
    assert_eq!(server.tls.session_timeout, Duration::from_secs(10 * 60));
    assert!(!server.tls.session_tickets);
    assert_eq!(server.auth_http.headers.len(), 1);
    assert_eq!(server.real_ip_from.len(), 1);
    assert!(matches!(
        &server.auth_http.address,
        starling::config::MailAuthHttpAddress::Inet(_)
    ));
}

#[test]
fn parses_unix_auth_http_and_rejects_incomplete_servers() {
    let config = compile_mail(
        r#"
            events {}
            mail {
                auth_http unix:/tmp/starling-auth.sock:/mail/auth;
                server {
                    listen unix:/tmp/starling-mail.sock;
                    protocol pop3;
                }
            }
        "#,
    );
    assert_eq!(config.mail.unwrap().servers[0].auth_http.uri, "/mail/auth");

    let error = mail_error(
        r#"
            events {}
            mail {
                server {
                    listen 127.0.0.1:8123;
                    protocol imap;
                }
            }
        "#,
    );
    assert!(error.contains("no \"auth_http\" is defined"));
}
