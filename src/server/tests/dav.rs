use super::support::*;

#[tokio::test]
async fn put_mkcol_and_delete_update_the_filesystem() {
    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            http {
                server {
                    listen 127.0.0.1:18527;
                    root data;
                    dav_methods put mkcol delete;
                    create_full_put_path on;
                    dav_access user:rw group:r all:r;
                }
            }
        "#,
        &[],
    );

    let created = request_body(
        &config,
        "PUT /uploads/a/b.txt HTTP/1.1\r\n\
         Host: localhost\r\n\
         Content-Length: 5\r\n\
         Connection: close\r\n\r\n\
         hello",
    )
    .await;
    assert!(created.starts_with("HTTP/1.1 201 Created\r\n"), "{created}");
    assert!(
        created.contains("Location: http://localhost:18527/uploads/a/b.txt\r\n"),
        "{created}"
    );
    let path = prefix.join("data/uploads/a/b.txt");
    assert_eq!(std::fs::read(&path).unwrap(), b"hello");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o644
        );
    }

    let replaced = request_body(
        &config,
        "PUT /uploads/a/b.txt HTTP/1.1\r\n\
         Host: localhost\r\n\
         Content-Length: 3\r\n\
         Connection: close\r\n\r\n\
         new",
    )
    .await;
    assert!(
        replaced.starts_with("HTTP/1.1 204 No Content\r\n"),
        "{replaced}"
    );
    assert_eq!(std::fs::read(&path).unwrap(), b"new");

    let collection = request_body(
        &config,
        "MKCOL /uploads/collection/ HTTP/1.1\r\n\
         Host: localhost\r\n\
         Connection: close\r\n\r\n",
    )
    .await;
    assert!(
        collection.starts_with("HTTP/1.1 201 Created\r\n"),
        "{collection}"
    );
    assert!(prefix.join("data/uploads/collection").is_dir());

    let deleted_file = request_body(
        &config,
        "DELETE /uploads/a/b.txt HTTP/1.1\r\n\
         Host: localhost\r\n\
         Connection: close\r\n\r\n",
    )
    .await;
    assert!(
        deleted_file.starts_with("HTTP/1.1 204 No Content\r\n"),
        "{deleted_file}"
    );
    assert!(!path.exists());

    let deleted_collection = request_body(
        &config,
        "DELETE /uploads/collection/ HTTP/1.1\r\n\
         Host: localhost\r\n\
         Connection: close\r\n\r\n",
    )
    .await;
    assert!(
        deleted_collection.starts_with("HTTP/1.1 204 No Content\r\n"),
        "{deleted_collection}"
    );
    assert!(!prefix.join("data/uploads/collection").exists());
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn copy_and_move_files_and_collections() {
    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            http {
                server {
                    listen 127.0.0.1:18528;
                    root data;
                    dav_methods copy move;
                }
            }
        "#,
        &[],
    );
    std::fs::create_dir_all(prefix.join("data/tree/nested")).unwrap();
    std::fs::write(prefix.join("data/source.txt"), b"source").unwrap();
    std::fs::write(prefix.join("data/tree/a.txt"), b"a").unwrap();
    std::fs::write(prefix.join("data/tree/nested/b.txt"), b"b").unwrap();

    let copied = request_body(
        &config,
        "COPY /source.txt HTTP/1.1\r\n\
         Host: localhost\r\n\
         Destination: /copy.txt\r\n\
         Connection: close\r\n\r\n",
    )
    .await;
    assert!(
        copied.starts_with("HTTP/1.1 204 No Content\r\n"),
        "{copied}"
    );
    assert_eq!(
        std::fs::read(prefix.join("data/copy.txt")).unwrap(),
        b"source"
    );

    let moved = request_body(
        &config,
        "MOVE /copy.txt HTTP/1.1\r\n\
         Host: localhost\r\n\
         Destination: http://localhost:18528/moved.txt\r\n\
         Connection: close\r\n\r\n",
    )
    .await;
    assert!(moved.starts_with("HTTP/1.1 204 No Content\r\n"), "{moved}");
    assert!(!prefix.join("data/copy.txt").exists());
    assert_eq!(
        std::fs::read(prefix.join("data/moved.txt")).unwrap(),
        b"source"
    );

    let copied_tree = request_body(
        &config,
        "COPY /tree/ HTTP/1.1\r\n\
         Host: localhost\r\n\
         Destination: /copied/\r\n\
         Depth: infinity\r\n\
         Connection: close\r\n\r\n",
    )
    .await;
    assert!(
        copied_tree.starts_with("HTTP/1.1 201 Created\r\n"),
        "{copied_tree}"
    );
    assert_eq!(
        std::fs::read(prefix.join("data/copied/nested/b.txt")).unwrap(),
        b"b"
    );
    assert!(prefix.join("data/tree/nested/b.txt").exists());

    let moved_tree = request_body(
        &config,
        "MOVE /tree/ HTTP/1.1\r\n\
         Host: localhost\r\n\
         Destination: /moved-tree/\r\n\
         Depth: infinity\r\n\
         Connection: close\r\n\r\n",
    )
    .await;
    assert!(
        moved_tree.starts_with("HTTP/1.1 201 Created\r\n"),
        "{moved_tree}"
    );
    assert!(!prefix.join("data/tree").exists());
    assert_eq!(
        std::fs::read(prefix.join("data/moved-tree/nested/b.txt")).unwrap(),
        b"b"
    );
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn dav_validates_methods_depths_and_destination_headers() {
    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            http {
                server {
                    listen 127.0.0.1:18529;
                    root data;
                    dav_methods put mkcol delete copy move;
                    min_delete_depth 2;
                }
            }
        "#,
        &[],
    );
    std::fs::create_dir_all(prefix.join("data/collection")).unwrap();
    std::fs::write(prefix.join("data/source.txt"), b"source").unwrap();
    std::fs::write(prefix.join("data/existing.txt"), b"existing").unwrap();

    let put_collection = request_body(
        &config,
        "PUT /collection/ HTTP/1.1\r\nHost: localhost\r\nContent-Length: 1\r\nConnection: close\r\n\r\nx",
    )
    .await;
    assert!(
        put_collection.starts_with("HTTP/1.1 409 Conflict\r\n"),
        "{put_collection}"
    );

    let mkcol_without_slash = request_body(
        &config,
        "MKCOL /new HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(
        mkcol_without_slash.starts_with("HTTP/1.1 409 Conflict\r\n"),
        "{mkcol_without_slash}"
    );

    let delete_with_body = request_body(
        &config,
        "DELETE /source.txt HTTP/1.1\r\nHost: localhost\r\nContent-Length: 1\r\nConnection: close\r\n\r\nx",
    )
    .await;
    assert!(
        delete_with_body.starts_with("HTTP/1.1 415 Unsupported Media Type\r\n"),
        "{delete_with_body}"
    );
    assert!(prefix.join("data/source.txt").exists());

    let shallow_delete = request_body(
        &config,
        "DELETE /source.txt HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(
        shallow_delete.starts_with("HTTP/1.1 409 Conflict\r\n"),
        "{shallow_delete}"
    );

    let missing_destination = request_body(
        &config,
        "COPY /source.txt HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(
        missing_destination.starts_with("HTTP/1.1 400 Bad Request\r\n"),
        "{missing_destination}"
    );

    let mismatched_collection = request_body(
        &config,
        "COPY /source.txt HTTP/1.1\r\nHost: localhost\r\nDestination: /target/\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(
        mismatched_collection.starts_with("HTTP/1.1 409 Conflict\r\n"),
        "{mismatched_collection}"
    );

    let invalid_depth = request_body(
        &config,
        "COPY /collection/ HTTP/1.1\r\nHost: localhost\r\nDestination: /other/\r\nDepth: 1\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(
        invalid_depth.starts_with("HTTP/1.1 400 Bad Request\r\n"),
        "{invalid_depth}"
    );

    let no_overwrite = request_body(
        &config,
        "COPY /source.txt HTTP/1.1\r\nHost: localhost\r\nDestination: /existing.txt\r\nOverwrite: F\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(
        no_overwrite.starts_with("HTTP/1.1 412 Precondition Failed\r\n"),
        "{no_overwrite}"
    );
    assert_eq!(
        std::fs::read(prefix.join("data/existing.txt")).unwrap(),
        b"existing"
    );
    let _ = std::fs::remove_dir_all(prefix);
}
