use super::*;

#[cfg(unix)]
#[tokio::test]
async fn evaluates_regex_alias_captures_for_files_and_request_filename() {
    use std::os::unix::fs::symlink;

    let (prefix, config) = test_config(
        r#"
                worker_processes 1;
                events { worker_connections 64; }
                http {
                    server {
                        listen 127.0.0.1:18157;
                        server_name localhost;
                        add_header X-File $request_filename always;
                        location ~ ^/files/(.+)$ {
                            disable_symlinks on from=$document_root;
                            alias data/$1;
                        }
                        location ~ ^/try-alias/files {
                            alias data;
                            try_files $uri =404;
                        }
                    }
                }
            "#,
        &[],
    );
    std::fs::create_dir_all(prefix.join("data")).unwrap();
    std::fs::write(prefix.join("data/file.txt"), b"aliased").unwrap();
    symlink("file.txt", prefix.join("data/link.txt")).unwrap();
    std::fs::create_dir_all(prefix.join("data/try-alias/files")).unwrap();
    std::fs::write(prefix.join("data/try-alias/files/file.txt"), b"tried").unwrap();

    let regular = request_body(
        &config,
        "GET /files/file.txt HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(regular.starts_with("HTTP/1.1 200 OK\r\n"), "{regular}");
    assert!(
        regular.contains(&format!(
            "X-File: {}\r\n",
            prefix.join("data/file.txt").display()
        )),
        "{regular}"
    );
    assert!(regular.ends_with("aliased"), "{regular}");

    let linked = request_body(
        &config,
        "GET /files/link.txt HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(linked.starts_with("HTTP/1.1 200 OK\r\n"), "{linked}");
    assert!(linked.ends_with("aliased"), "{linked}");

    let tried = request_body(
        &config,
        "GET /try-alias/files/file.txt HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(tried.starts_with("HTTP/1.1 200 OK\r\n"), "{tried}");
    assert!(tried.ends_with("tried"), "{tried}");
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn serves_an_exact_alias_file_without_appending_the_uri() {
    let (prefix, config) = test_config(
        r#"
                worker_processes 1;
                events { worker_connections 64; }
                http {
                    server {
                        listen 127.0.0.1:18158;
                        server_name localhost;
                        location /alias-file {
                            alias data/file.txt;
                        }
                    }
                }
            "#,
        &[],
    );
    std::fs::create_dir_all(prefix.join("data")).unwrap();
    std::fs::write(prefix.join("data/file.txt"), b"aliased").unwrap();

    let response = request_body(
        &config,
        "GET /alias-file HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(response.starts_with("HTTP/1.1 200 OK\r\n"), "{response}");
    assert!(response.ends_with("aliased"), "{response}");
    let _ = std::fs::remove_dir_all(prefix);
}
