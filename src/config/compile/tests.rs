use super::*;

#[test]
fn normalizes_uri_and_blocks_traversal() {
    assert_eq!(normalize_uri("/a/../b//c"), Some("/b/c".into()));
    assert_eq!(normalize_uri("/a/../"), Some("/".into()));
    assert_eq!(normalize_uri("/foo/bar/."), Some("/foo/bar/".into()));
    assert_eq!(normalize_uri("/foo/bar/.."), Some("/foo/".into()));
    assert_eq!(normalize_uri("/%2e%2e/secret"), Some("/secret".into()));
    assert_eq!(normalize_uri("/%00"), None);
    assert_eq!(normalize_uri("/%02"), Some("/\u{2}".into()));
    assert_eq!(normalize_uri("/bad%"), None);
    assert_eq!(normalize_uri("/bad%1"), None);
    assert_eq!(normalize_uri("/bad\u{2}"), None);
    assert_eq!(
        normalize_uri_with_merge("/a//b/./c/", false),
        Some("/a//b/c/".into())
    );
    assert_eq!(
        normalize_uri_with_merge("/a//b/./c/", true),
        Some("/a/b/c/".into())
    );
}

#[test]
fn parses_sizes_and_times() {
    let location = SourceLocation::new("test", 1, 1);
    assert_eq!(parse_size("10m", &location).unwrap(), 10 * 1024 * 1024);
    assert_eq!(parse_time_seconds("1h", &location).unwrap(), 3600.0);
}

#[test]
fn cidr_matching() {
    let network = IpNetwork {
        address: "192.0.2.0".parse().unwrap(),
        prefix: 24,
    };
    assert!(network.contains("192.0.2.55".parse().unwrap()));
    assert!(!network.contains("192.0.3.1".parse().unwrap()));
}

#[test]
fn parses_openssl_cipher_lists() {
    let default = TlsCipherConfig::parse("HIGH:!aNULL:!MD5").unwrap();
    assert_eq!(default.suites, TlsCipherSuite::TLS12_DEFAULT);

    let ecdsa = TlsCipherConfig::parse("DEFAULT:!RSA").unwrap();
    assert_eq!(
        ecdsa.suites,
        vec![
            TlsCipherSuite::EcdheEcdsaAes256GcmSha384,
            TlsCipherSuite::EcdheEcdsaAes128GcmSha256,
            TlsCipherSuite::EcdheEcdsaChaCha20Poly1305Sha256,
        ]
    );

    let aes128 = TlsCipherConfig::parse("AES128+AESGCM").unwrap();
    assert_eq!(
        aes128.suites,
        vec![
            TlsCipherSuite::EcdheEcdsaAes128GcmSha256,
            TlsCipherSuite::EcdheRsaAes128GcmSha256,
        ]
    );

    let ordered =
        TlsCipherConfig::parse("ECDHE-RSA-AES128-GCM-SHA256:ECDHE-RSA-AES256-GCM-SHA384").unwrap();
    assert_eq!(
        ordered.suites,
        vec![
            TlsCipherSuite::EcdheRsaAes128GcmSha256,
            TlsCipherSuite::EcdheRsaAes256GcmSha384,
        ]
    );

    let moved = TlsCipherConfig::parse("HIGH:+AES128").unwrap();
    assert_eq!(
        moved.suites,
        vec![
            TlsCipherSuite::EcdheEcdsaAes256GcmSha384,
            TlsCipherSuite::EcdheEcdsaChaCha20Poly1305Sha256,
            TlsCipherSuite::EcdheRsaAes256GcmSha384,
            TlsCipherSuite::EcdheRsaChaCha20Poly1305Sha256,
            TlsCipherSuite::EcdheEcdsaAes128GcmSha256,
            TlsCipherSuite::EcdheRsaAes128GcmSha256,
        ]
    );

    let move_only = TlsCipherConfig::parse("ECDHE-RSA-AES128-GCM-SHA256:+ECDHE").unwrap();
    assert_eq!(
        move_only.suites,
        vec![TlsCipherSuite::EcdheRsaAes128GcmSha256]
    );

    let aead_only = TlsCipherConfig::parse("AESGCM:!SHA256:!SHA384").unwrap();
    assert_eq!(
        aead_only.suites,
        vec![
            TlsCipherSuite::EcdheEcdsaAes256GcmSha384,
            TlsCipherSuite::EcdheEcdsaAes128GcmSha256,
            TlsCipherSuite::EcdheRsaAes256GcmSha384,
            TlsCipherSuite::EcdheRsaAes128GcmSha256,
        ]
    );

    let removed = TlsCipherConfig::parse("DEFAULT:!ECDSA:ECDSA").unwrap();
    assert!(removed.suites.iter().all(|suite| !suite.uses_ecdsa()));
    assert!(TlsCipherConfig::parse("SHA256").is_err());
    assert!(TlsCipherConfig::parse("MEDIUM").is_err());
    assert!(TlsCipherConfig::parse("DEFAULT:!ALL").is_err());
    assert!(TlsCipherConfig::parse("NOT_A_CIPHER").is_err());
}
