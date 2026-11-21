use super::support::*;

#[cfg(unix)]
#[tokio::test]
async fn rejects_symlinks_and_allows_location_override() {
    use std::os::unix::fs::symlink;

    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            http {
                server {
                    listen 127.0.0.1:18532;
                    root data;
                    disable_symlinks on;
                    location /open/ {
                        disable_symlinks off;
                    }
                }
            }
        "#,
        &[],
    );
    std::fs::create_dir_all(prefix.join("data/open")).unwrap();
    std::fs::write(prefix.join("data/real.txt"), b"real").unwrap();
    symlink("../real.txt", prefix.join("data/link.txt")).unwrap();
    symlink("../real.txt", prefix.join("data/open/link.txt")).unwrap();
    symlink("real.txt", prefix.join("data/link-dir")).unwrap();

    let normal = request_body(
        &config,
        "GET /real.txt HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(normal.starts_with("HTTP/1.1 200 OK\r\n"), "{normal}");

    let linked = request_body(
        &config,
        "GET /link.txt HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(linked.starts_with("HTTP/1.1 403 Forbidden\r\n"), "{linked}");

    let linked_directory = request_body(
        &config,
        "GET /link-dir HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(
        linked_directory.starts_with("HTTP/1.1 403 Forbidden\r\n"),
        "{linked_directory}"
    );

    let overridden = request_body(
        &config,
        "GET /open/link.txt HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(
        overridden.starts_with("HTTP/1.1 200 OK\r\n"),
        "{overridden}"
    );
    assert!(overridden.ends_with("real"), "{overridden}");
    let _ = std::fs::remove_dir_all(prefix);
}

#[cfg(unix)]
#[tokio::test]
async fn from_prefix_permits_symlinked_document_root() {
    use std::os::unix::fs::symlink;

    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            http {
                server {
                    listen 127.0.0.1:18533;
                    root data;
                    disable_symlinks on from=$document_root;
                }
            }
        "#,
        &[],
    );
    std::fs::create_dir_all(prefix.join("real-data")).unwrap();
    std::fs::write(prefix.join("real-data/file.txt"), b"file").unwrap();
    std::fs::write(prefix.join("target.txt"), b"target").unwrap();
    symlink("real-data", prefix.join("data")).unwrap();
    symlink("../target.txt", prefix.join("real-data/link.txt")).unwrap();

    let document_root_symlink = request_body(
        &config,
        "GET /file.txt HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(
        document_root_symlink.starts_with("HTTP/1.1 200 OK\r\n"),
        "{document_root_symlink}"
    );

    let nested_symlink = request_body(
        &config,
        "GET /link.txt HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(
        nested_symlink.starts_with("HTTP/1.1 403 Forbidden\r\n"),
        "{nested_symlink}"
    );
    let _ = std::fs::remove_dir_all(prefix);
}

#[cfg(unix)]
#[tokio::test]
async fn if_not_owner_allows_same_owner_symlinks() {
    use std::os::unix::fs::symlink;

    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            http {
                server {
                    listen 127.0.0.1:18534;
                    root data;
                    disable_symlinks if_not_owner;
                }
            }
        "#,
        &[],
    );
    std::fs::create_dir_all(prefix.join("data")).unwrap();
    std::fs::write(prefix.join("data/target.txt"), b"same-owner").unwrap();
    symlink("target.txt", prefix.join("data/link.txt")).unwrap();

    let response = request_body(
        &config,
        "GET /link.txt HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(response.starts_with("HTTP/1.1 200 OK\r\n"), "{response}");
    assert!(response.ends_with("same-owner"), "{response}");
    let _ = std::fs::remove_dir_all(prefix);
}

#[cfg(unix)]
#[tokio::test]
async fn applies_to_try_files_and_index_candidates() {
    use std::os::unix::fs::symlink;

    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            http {
                server {
                    listen 127.0.0.1:18535;
                    root data;
                    disable_symlinks on;
                    index index.html;
                    location / {
                        try_files $uri =404;
                    }
                }
            }
        "#,
        &[],
    );
    std::fs::create_dir_all(prefix.join("data/dir")).unwrap();
    std::fs::write(prefix.join("data/target.txt"), b"target").unwrap();
    std::fs::write(prefix.join("data/index-target.html"), b"index").unwrap();
    symlink("target.txt", prefix.join("data/link.txt")).unwrap();
    symlink("../index-target.html", prefix.join("data/dir/index.html")).unwrap();

    let try_files = request_body(
        &config,
        "GET /link.txt HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(
        try_files.starts_with("HTTP/1.1 404 Not Found\r\n"),
        "{try_files}"
    );

    let index = request_body(
        &config,
        "GET /dir/ HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(index.starts_with("HTTP/1.1 403 Forbidden\r\n"), "{index}");
    let _ = std::fs::remove_dir_all(prefix);
}

#[cfg(unix)]
#[tokio::test]
async fn applies_to_gzip_static_candidates() {
    use std::os::unix::fs::symlink;

    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            http {
                server {
                    listen 127.0.0.1:18536;
                    root data;
                    disable_symlinks on;
                    gzip_static on;
                }
            }
        "#,
        &[],
    );
    std::fs::create_dir_all(prefix.join("data")).unwrap();
    std::fs::write(prefix.join("data/file.txt"), b"plain").unwrap();
    std::fs::write(prefix.join("data/compressed.gz"), b"compressed").unwrap();
    symlink("compressed.gz", prefix.join("data/file.txt.gz")).unwrap();

    let response = request_body(
        &config,
        "GET /file.txt HTTP/1.1\r\n\
         Host: localhost\r\n\
         Accept-Encoding: gzip\r\n\
         Connection: close\r\n\r\n",
    )
    .await;
    assert!(
        response.starts_with("HTTP/1.1 403 Forbidden\r\n"),
        "{response}"
    );
    let _ = std::fs::remove_dir_all(prefix);
}
