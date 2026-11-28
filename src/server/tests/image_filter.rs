use std::io::Cursor;

use super::support::*;
use image::{DynamicImage, GenericImageView, ImageFormat, Rgba, RgbaImage};

#[tokio::test]
async fn reports_image_size_and_validates_images_without_changing_test_bodies() {
    let (prefix, config) = test_config(
        r#"
            events { worker_connections 64; }
            http {
                server {
                    listen 127.0.0.1:18241;
                    root html;
                    location /size/ {
                        image_filter size;
                    }
                    location /test/ {
                        image_filter test;
                    }
                    location /missing/ {
                        image_filter test;
                    }
                }
            }
        "#,
        &[],
    );
    let png = image_bytes(37, 23, ImageFormat::Png);
    write_test_file(&prefix, "html/size/source.png", &png);
    write_test_file(
        &prefix,
        "html/size/source.jpg",
        &image_bytes(37, 23, ImageFormat::Jpeg),
    );
    write_test_file(
        &prefix,
        "html/size/source.gif",
        &image_bytes(37, 23, ImageFormat::Gif),
    );
    write_test_file(
        &prefix,
        "html/size/source.webp",
        &image_bytes(37, 23, ImageFormat::WebP),
    );
    write_test_file(&prefix, "html/size/source.txt", b"not an image");
    write_test_file(
        &prefix,
        "html/size/broken.png",
        b"\x89PNG\r\n\x1a\nnot an image!!!",
    );
    write_test_file(&prefix, "html/test/source.png", &png);
    write_test_file(&prefix, "html/test/source.txt", b"not an image");
    write_test_file(
        &prefix,
        "html/test/broken.png",
        b"\x89PNG\r\n\x1a\nnot an image!!!",
    );

    let size = request_bytes(
        &config,
        "GET /size/source.png HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    let (headers, body) = split_http_response(&size);
    assert!(headers.starts_with("HTTP/1.1 200 OK\r\n"), "{headers}");
    assert!(
        headers.contains("Content-Type: application/json\r\n"),
        "{headers}"
    );
    assert_eq!(
        body,
        b"{ \"img\" : { \"width\": 37, \"height\": 23, \"type\": \"png\" } }\r\n"
    );
    for (path, kind) in [("jpg", "jpeg"), ("gif", "gif"), ("webp", "webp")] {
        let response = request_bytes(
            &config,
            &format!(
                "GET /size/source.{path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"
            ),
        )
        .await;
        let (_, body) = split_http_response(&response);
        assert_eq!(
            body,
            format!(
                "{{ \"img\" : {{ \"width\": 37, \"height\": 23, \"type\": \"{kind}\" }} }}\r\n"
            )
            .as_bytes()
        );
    }

    let invalid_size = request_bytes(
        &config,
        "GET /size/source.txt HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    let (headers, body) = split_http_response(&invalid_size);
    assert!(headers.starts_with("HTTP/1.1 200 OK\r\n"), "{headers}");
    assert_eq!(body, b"{}\r\n");

    let broken_size = request_bytes(
        &config,
        "GET /size/broken.png HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    let (_, body) = split_http_response(&broken_size);
    assert_eq!(body, b"{}\r\n");

    let tested = request_bytes(
        &config,
        "GET /test/source.png HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    let (headers, body) = split_http_response(&tested);
    assert!(headers.starts_with("HTTP/1.1 200 OK\r\n"), "{headers}");
    assert!(headers.contains("Content-Type: image/png\r\n"), "{headers}");
    assert!(!headers.contains("Accept-Ranges:"), "{headers}");
    assert_eq!(body, png);

    let tested_head = request_bytes(
        &config,
        "HEAD /test/source.png HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    let (headers, body) = split_http_response(&tested_head);
    assert!(headers.starts_with("HTTP/1.1 200 OK\r\n"), "{headers}");
    assert!(headers.contains("Content-Type: image/png\r\n"), "{headers}");
    assert!(body.is_empty(), "HEAD response must not contain a body");

    let invalid = request_bytes(
        &config,
        "GET /test/source.txt HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    let (headers, _) = split_http_response(&invalid);
    assert!(
        headers.starts_with("HTTP/1.1 415 Unsupported Media Type\r\n"),
        "{headers}"
    );

    let broken = request_bytes(
        &config,
        "GET /test/broken.png HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    let (headers, _) = split_http_response(&broken);
    assert!(
        headers.starts_with("HTTP/1.1 415 Unsupported Media Type\r\n"),
        "{headers}"
    );

    let missing = request_bytes(
        &config,
        "GET /missing/source.png HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    let (headers, _) = split_http_response(&missing);
    assert!(
        headers.starts_with("HTTP/1.1 404 Not Found\r\n"),
        "{headers}"
    );
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn resizes_crops_rotates_and_reencodes_supported_image_formats() {
    let (prefix, config) = test_config(
        r#"
            events { worker_connections 64; }
            http {
                server {
                    listen 127.0.0.1:18242;
                    root html;
                    error_page 415 /fallback;
                    location /resize/ {
                        image_filter resize $arg_w $arg_h;
                    }
                    location /rotate/ {
                        image_filter rotate $arg_r;
                    }
                    location /crop-rotate/ {
                        image_filter crop 40 50;
                        image_filter rotate 90;
                    }
                    location /jpeg/ {
                        image_filter resize 10 12;
                        image_filter_jpeg_quality $arg_q;
                    }
                    location /gif/ {
                        image_filter resize 10 12;
                        image_filter_interlace on;
                    }
                    location /webp/ {
                        image_filter rotate 90;
                        image_filter_webp_quality $arg_q;
                    }
                    location /png/ {
                        image_filter resize 100 120;
                        image_filter_interlace on;
                    }
                    location /buffer/ {
                        image_filter test;
                        image_filter_buffer 16;
                    }
                    location = /fallback {
                        internal;
                        return 200 "fallback\n";
                    }
                }
            }
        "#,
        &[],
    );
    let png = image_bytes(100, 120, ImageFormat::Png);
    let jpeg = image_bytes(100, 120, ImageFormat::Jpeg);
    let gif = image_bytes(100, 120, ImageFormat::Gif);
    let webp = image_bytes(100, 120, ImageFormat::WebP);
    for directory in ["resize", "rotate", "crop-rotate", "png", "buffer"] {
        write_test_file(&prefix, &format!("html/{directory}/source.png"), &png);
    }
    write_test_file(&prefix, "html/jpeg/source.jpg", &jpeg);
    write_test_file(&prefix, "html/gif/source.gif", &gif);
    write_test_file(&prefix, "html/webp/source.webp", &webp);

    let resized = request_bytes(
        &config,
        "GET /resize/source.png?w=10&h=- HTTP/1.1\r\nHost: localhost\r\n\
         Range: bytes=0-0\r\nConnection: close\r\n\r\n",
    )
    .await;
    let (headers, body) = split_http_response(&resized);
    assert!(headers.starts_with("HTTP/1.1 200 OK\r\n"), "{headers}");
    assert_eq!(image_dimensions(body), (10, 12));
    assert!(!headers.contains("Accept-Ranges:"), "{headers}");
    assert!(
        headers.contains("ETag: W/\"") || !headers.contains("ETag:"),
        "{headers}"
    );
    let content_length = body.len().to_string();
    assert_eq!(
        header_value(headers, "Content-Length"),
        Some(content_length.as_str())
    );

    let rotated = request_bytes(
        &config,
        "GET /rotate/source.png?r=90 HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    let (_, body) = split_http_response(&rotated);
    assert_eq!(image_dimensions(body), (120, 100));

    let crop_rotated = request_bytes(
        &config,
        "GET /crop-rotate/source.png HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    let (_, body) = split_http_response(&crop_rotated);
    assert_eq!(image_dimensions(body), (40, 41));

    let low_quality = request_bytes(
        &config,
        "GET /jpeg/source.jpg?q=10 HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    let high_quality = request_bytes(
        &config,
        "GET /jpeg/source.jpg?q=90 HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    let (headers, low_body) = split_http_response(&low_quality);
    let (_, high_body) = split_http_response(&high_quality);
    assert!(
        headers.contains("Content-Type: image/jpeg\r\n"),
        "{headers}"
    );
    assert_eq!(image_dimensions(low_body), (10, 12));
    assert_ne!(low_body, high_body);

    let gif_response = request_bytes(
        &config,
        "GET /gif/source.gif HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    let (headers, body) = split_http_response(&gif_response);
    assert!(headers.contains("Content-Type: image/gif\r\n"), "{headers}");
    assert_eq!(image_dimensions(body), (10, 12));
    assert!(gif_is_interlaced(body));

    let webp_response = request_bytes(
        &config,
        "GET /webp/source.webp?q=80 HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    let (headers, body) = split_http_response(&webp_response);
    assert!(
        headers.contains("Content-Type: image/webp\r\n"),
        "{headers}"
    );
    assert_eq!(image_dimensions(body), (120, 100));

    let png_response = request_bytes(
        &config,
        "GET /png/source.png HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    let (headers, body) = split_http_response(&png_response);
    assert!(headers.contains("Content-Type: image/png\r\n"), "{headers}");
    assert_eq!(image_dimensions(body), (100, 120));
    assert!(
        png_is_interlaced(body),
        "image_filter_interlace must reencode"
    );

    let invalid_dimension = request_bytes(
        &config,
        "GET /resize/source.png?w=0&h=12 HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    let (headers, body) = split_http_response(&invalid_dimension);
    assert!(
        headers.starts_with("HTTP/1.1 415 Unsupported Media Type\r\n"),
        "{headers}"
    );
    assert_eq!(body, b"fallback\n");

    let invalid_quality = request_bytes(
        &config,
        "GET /jpeg/source.jpg?q=101 HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    let (headers, body) = split_http_response(&invalid_quality);
    assert!(
        headers.starts_with("HTTP/1.1 415 Unsupported Media Type\r\n"),
        "{headers}"
    );
    assert_eq!(body, b"fallback\n");

    let oversized = request_bytes(
        &config,
        "GET /buffer/source.png HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    let (headers, body) = split_http_response(&oversized);
    assert!(
        headers.starts_with("HTTP/1.1 415 Unsupported Media Type\r\n"),
        "{headers}"
    );
    assert_eq!(body, b"fallback\n");
    let _ = std::fs::remove_dir_all(prefix);
}

fn image_bytes(width: u32, height: u32, format: ImageFormat) -> Vec<u8> {
    let image = RgbaImage::from_fn(width, height, |x, y| {
        Rgba([
            (x * 255 / width.max(1)) as u8,
            (y * 255 / height.max(1)) as u8,
            ((x + y) % 256) as u8,
            if x < width / 4 { 0 } else { 255 },
        ])
    });
    let mut output = Cursor::new(Vec::new());
    DynamicImage::ImageRgba8(image)
        .write_to(&mut output, format)
        .unwrap();
    output.into_inner()
}

fn write_test_file(prefix: &std::path::Path, path: &str, contents: &[u8]) {
    let path = prefix.join(path);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
}

fn header_value<'a>(headers: &'a str, name: &str) -> Option<&'a str> {
    headers.lines().find_map(|line| {
        let (header_name, value) = line.split_once(':')?;
        header_name
            .eq_ignore_ascii_case(name)
            .then_some(value.trim())
    })
}

fn image_dimensions(body: &[u8]) -> (u32, u32) {
    image::load_from_memory(body).unwrap().dimensions()
}

fn png_is_interlaced(body: &[u8]) -> bool {
    body.get(28) == Some(&1)
}

fn gif_is_interlaced(body: &[u8]) -> bool {
    let mut offset = 13;
    if body.get(10).is_some_and(|flags| flags & 0x80 != 0) {
        offset += 3 * (1 << ((body[10] & 0x07) + 1));
    }
    while offset + 10 <= body.len() {
        match body[offset] {
            0x2c => {
                let flags = body[offset + 9];
                if flags & 0x80 != 0 {
                    return flags & 0x40 != 0;
                }
                return false;
            }
            0x21 => {
                offset += 2;
                while offset < body.len() {
                    let size = body[offset] as usize;
                    offset += 1;
                    if size == 0 {
                        break;
                    }
                    offset += size;
                }
            }
            _ => return false,
        }
    }
    false
}
