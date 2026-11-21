use super::support::*;
use super::*;

const FLV_HEADER: &[u8] = b"FLV\x01\x05\0\0\0\x09\0\0\0\0";

async fn request(config: &Arc<RuntimeConfig>, target: &str, headers: &str) -> Vec<u8> {
    let request =
        format!("GET {target} HTTP/1.1\r\nHost: localhost\r\n{headers}Connection: close\r\n\r\n");
    request_bytes(config, &request).await
}

#[tokio::test]
async fn serves_flv_ranges_from_the_start_argument() {
    let media: Vec<u8> = (0_u8..32).collect();
    let (prefix, config) = test_config(
        r#"
                worker_processes 1;
                events { worker_connections 64; }
                http {
                    server {
                        listen 127.0.0.1:18169;
                        root html;
                        location /video/ {
                            flv;
                            location /video/plain/ { }
                        }
                    }
                }
            "#,
        &[],
    );
    std::fs::create_dir_all(prefix.join("html/video")).unwrap();
    std::fs::create_dir_all(prefix.join("html/video/plain")).unwrap();
    std::fs::write(prefix.join("html/video/sample.flv"), &media).unwrap();
    std::fs::write(prefix.join("html/video/plain/sample.flv"), &media).unwrap();

    let full = request(&config, "/video/sample.flv", "").await;
    let (headers, body) = split_http_response(&full);
    assert!(headers.starts_with("HTTP/1.1 200 OK\r\n"), "{headers}");
    assert!(
        headers.contains("Content-Type: video/x-flv\r\n"),
        "{headers}"
    );
    assert_eq!(body, media);

    let from_offset = request(&config, "/video/sample.flv?start=20", "").await;
    let (headers, body) = split_http_response(&from_offset);
    assert!(headers.starts_with("HTTP/1.1 200 OK\r\n"), "{headers}");
    assert!(headers.contains("Content-Length: 25\r\n"), "{headers}");
    let mut expected = FLV_HEADER.to_vec();
    expected.extend_from_slice(&media[20..]);
    assert_eq!(body, expected);

    let invalid = request(&config, "/video/sample.flv?start=999", "").await;
    let (_, body) = split_http_response(&invalid);
    assert_eq!(body, media);

    let ranged = request(
        &config,
        "/video/sample.flv?start=20",
        "Range: bytes=10-15\r\n",
    )
    .await;
    let (headers, body) = split_http_response(&ranged);
    assert!(
        headers.starts_with("HTTP/1.1 206 Partial Content\r\n"),
        "{headers}"
    );
    assert!(
        headers.contains("Content-Range: bytes 10-15/25\r\n"),
        "{headers}"
    );
    let mut expected = FLV_HEADER[10..].to_vec();
    expected.extend_from_slice(&media[20..23]);
    assert_eq!(body, expected);

    let nested = request(&config, "/video/plain/sample.flv?start=20", "").await;
    let (_, body) = split_http_response(&nested);
    assert_eq!(body, media);

    let head = request_bytes(
        &config,
        "HEAD /video/sample.flv?start=20 HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    let (headers, body) = split_http_response(&head);
    assert!(headers.contains("Content-Length: 25\r\n"), "{headers}");
    assert!(body.is_empty());

    let _ = std::fs::remove_dir_all(prefix);
}

#[test]
fn restricts_flv_to_locations_without_arguments() {
    let server_scope =
        test_config_error("events {} http { server { listen 127.0.0.1:18170; flv; } }\n");
    assert!(
        server_scope.contains("\"flv\" directive is not allowed here"),
        "{server_scope}"
    );

    let with_argument = test_config_error(
        "events {} http { server { listen 127.0.0.1:18170; location / { flv on; } } }\n",
    );
    assert!(
        with_argument.contains("invalid number of arguments in \"flv\" directive"),
        "{with_argument}"
    );
}
