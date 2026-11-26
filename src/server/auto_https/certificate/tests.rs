use super::*;

#[test]
fn creates_reusable_local_certificate() {
    let directory = std::env::temp_dir().join(format!(
        "starling-auto-https-{}-{}",
        std::process::id(),
        AUTO_HTTPS_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let names = vec!["localhost".to_string(), "dev.local".to_string()];
    let first = ensure_local_certificate(&directory, &names, Duration::from_secs(1)).unwrap();
    let second = ensure_local_certificate(&directory, &names, Duration::from_secs(1)).unwrap();
    assert_eq!(first.cert, second.cert);
    assert!(first.is_fresh(&names, Duration::from_secs(1)));
    assert_eq!(first.source(), Some(CertificateSource::LocalCa));
    assert!(directory.join("local-ca.crt").exists());
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn rejects_missing_san_or_expiring_certificate() {
    let directory = std::env::temp_dir().join(format!(
        "starling-auto-https-san-{}-{}",
        std::process::id(),
        AUTO_HTTPS_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let names = vec!["localhost".to_string()];
    let paths = ensure_local_certificate(&directory, &names, Duration::from_secs(1)).unwrap();
    assert!(paths.is_fresh(&names, Duration::from_secs(1)));
    assert!(!paths.is_fresh(&["other.test".to_string()], Duration::from_secs(1)));
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn tracks_ip_subject_alternative_names() {
    let directory = std::env::temp_dir().join(format!(
        "starling-auto-https-ip-san-{}-{}",
        std::process::id(),
        AUTO_HTTPS_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let names = vec!["127.0.0.1".to_string(), "::1".to_string()];
    let paths = ensure_local_certificate(&directory, &names, Duration::from_secs(1)).unwrap();
    assert!(paths.is_fresh(&names, Duration::from_secs(1)));
    assert!(!paths.is_fresh(&["127.0.0.2".to_string()], Duration::from_secs(1)));
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn recreates_local_leaf_when_the_ca_disappears() {
    let directory = std::env::temp_dir().join(format!(
        "starling-auto-https-ca-recovery-{}-{}",
        std::process::id(),
        AUTO_HTTPS_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let names = vec!["localhost".to_string()];
    let paths = ensure_local_certificate(&directory, &names, Duration::from_secs(1)).unwrap();
    let original_certificate = std::fs::read(&paths.cert).unwrap();
    std::fs::remove_file(directory.join("local-ca.crt")).unwrap();
    std::fs::remove_file(directory.join("local-ca.key")).unwrap();

    let paths = ensure_local_certificate(&directory, &names, Duration::from_secs(1)).unwrap();
    assert!(directory.join("local-ca.crt").exists());
    assert_ne!(original_certificate, std::fs::read(paths.cert).unwrap());
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn recreates_local_leaf_when_the_private_key_is_missing() {
    let directory = std::env::temp_dir().join(format!(
        "starling-auto-https-key-recovery-{}-{}",
        std::process::id(),
        AUTO_HTTPS_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let names = vec!["localhost".to_string()];
    let paths = ensure_local_certificate(&directory, &names, Duration::from_secs(1)).unwrap();
    std::fs::remove_file(&paths.key).unwrap();
    assert!(!paths.is_usable(&names, Duration::from_secs(1)));

    let paths = ensure_local_certificate(&directory, &names, Duration::from_secs(1)).unwrap();
    assert!(paths.is_usable(&names, Duration::from_secs(1)));
    assert!(paths.key.exists());
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn recreates_local_leaf_when_the_private_key_does_not_match() {
    let directory = std::env::temp_dir().join(format!(
        "starling-auto-https-key-mismatch-{}-{}",
        std::process::id(),
        AUTO_HTTPS_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let names = vec!["localhost".to_string()];
    let paths = ensure_local_certificate(&directory, &names, Duration::from_secs(1)).unwrap();
    std::fs::write(
        &paths.key,
        generate_ec_key()
            .unwrap()
            .private_key_to_pem_pkcs8()
            .unwrap(),
    )
    .unwrap();
    assert!(!paths.is_usable(&names, Duration::from_secs(1)));

    let paths = ensure_local_certificate(&directory, &names, Duration::from_secs(1)).unwrap();
    assert!(paths.is_usable(&names, Duration::from_secs(1)));
    let _ = std::fs::remove_dir_all(directory);
}
