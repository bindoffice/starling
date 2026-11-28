use super::*;

#[test]
fn accepts_image_filter_module_configuration() {
    let prefix = temp_prefix();
    fs::create_dir_all(&prefix).unwrap();
    let config_path = prefix.join("nginx.conf");
    fs::write(
        &config_path,
        r#"
            load_module modules/ngx_http_image_filter_module.so;
            events {}
            http {
                image_filter_buffer 3m;
                image_filter_jpeg_quality 70;
                server {
                    listen 127.0.0.1:18231;
                    image_filter_webp_quality 65;
                    location /images/ {
                        image_filter resize 320 200;
                        image_filter rotate 90;
                        image_filter_buffer 2m;
                        image_filter_interlace on;
                        image_filter_jpeg_quality 80;
                        image_filter_sharpen 10;
                        image_filter_transparency off;
                        image_filter_webp_quality 75;
                    }
                }
            }
        "#,
    )
    .unwrap();

    let document = ConfigLoader::new(prefix.clone())
        .load(&config_path)
        .unwrap();
    let config = compile(document, prefix.clone(), config_path).expect("compatible configuration");

    assert!(
        config.warnings.iter().any(|warning| {
            warning.message.contains("load_module")
                && warning.message.contains("ngx_http_image_filter_module.so")
                && warning.message.contains("ignored")
        }),
        "{:#?}",
        config.warnings
    );
    assert!(
        !config
            .warnings
            .iter()
            .any(|warning| warning.message.contains("\"image_filter")),
        "image filter directives should be implemented: {:#?}",
        config.warnings
    );

    let server = &config.http.as_ref().unwrap().servers[0];
    let image_filter = &server.locations[0].context.image_filter;
    assert_eq!(image_filter.mode, ImageFilterMode::Resize);
    assert_eq!(image_filter.width.as_deref(), Some("320"));
    assert_eq!(image_filter.height.as_deref(), Some("200"));
    assert_eq!(image_filter.angle.as_deref(), Some("90"));
    assert_eq!(image_filter.buffer_size, 2 * 1024 * 1024);
    assert_eq!(image_filter.jpeg_quality.as_deref(), Some("80"));
    assert_eq!(image_filter.webp_quality.as_deref(), Some("75"));
    assert_eq!(image_filter.sharpen.as_deref(), Some("10"));
    assert!(image_filter.interlace);
    assert!(!image_filter.transparency);
    let _ = fs::remove_dir_all(prefix);
}

#[test]
fn child_image_filter_clears_inherited_filter_arguments() {
    let prefix = temp_prefix();
    fs::create_dir_all(&prefix).unwrap();
    let config_path = prefix.join("nginx.conf");
    fs::write(
        &config_path,
        r#"
            events {}
            http {
                server {
                    listen 127.0.0.1:18232;
                    location /parent/ {
                        image_filter resize 100 120;
                        image_filter_jpeg_quality 60;
                        location /parent/rotate/ {
                            image_filter rotate 90;
                        }
                        location /parent/inherit/ { }
                    }
                }
            }
        "#,
    )
    .unwrap();

    let document = ConfigLoader::new(prefix.clone())
        .load(&config_path)
        .unwrap();
    let config = compile(document, prefix.clone(), config_path).unwrap();
    let parent = &config.http.as_ref().unwrap().servers[0].locations[0];

    let rotated = &parent.nested[0].context.image_filter;
    assert_eq!(rotated.mode, ImageFilterMode::Rotate);
    assert_eq!(rotated.width, None);
    assert_eq!(rotated.height, None);
    assert_eq!(rotated.angle.as_deref(), Some("90"));
    assert_eq!(rotated.jpeg_quality.as_deref(), Some("60"));

    let inherited = &parent.nested[1].context.image_filter;
    assert_eq!(inherited.mode, ImageFilterMode::Resize);
    assert_eq!(inherited.width.as_deref(), Some("100"));
    assert_eq!(inherited.height.as_deref(), Some("120"));
    let _ = fs::remove_dir_all(prefix);
}

#[test]
fn still_rejects_invalid_image_filter_syntax() {
    let cases = [
        (
            r#"
                events {}
                http {
                    server {
                        listen 127.0.0.1:18233;
                        location /images/ {
                            image_filter resize 320;
                        }
                    }
                }
            "#,
            "invalid arguments in \"image_filter\" directive",
        ),
        (
            r#"
                events {}
                http {
                    image_filter test;
                }
            "#,
            "\"image_filter\" directive is not allowed here",
        ),
        (
            r#"
                events {}
                http {
                    image_filter_jpeg_quality 70;
                    image_filter_jpeg_quality 80;
                }
            "#,
            "\"image_filter_jpeg_quality\" directive is duplicate",
        ),
        (
            r#"
                events {}
                http {
                    image_filter_webp_quality 101;
                }
            "#,
            "invalid value \"101\"",
        ),
        (
            r#"
                events {}
                http {
                    image_filter_sharpen 101;
                }
            "#,
            "invalid value \"101\"",
        ),
    ];

    for (source, expected) in cases {
        let prefix = temp_prefix();
        fs::create_dir_all(&prefix).unwrap();
        let config_path = prefix.join("nginx.conf");
        fs::write(&config_path, source).unwrap();

        let document = ConfigLoader::new(prefix.clone())
            .load(&config_path)
            .unwrap();
        let error = compile(document, prefix.clone(), config_path).unwrap_err();
        assert!(error.message.contains(expected), "{}", error.message);
        let _ = fs::remove_dir_all(prefix);
    }
}
