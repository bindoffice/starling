use super::*;

#[test]
fn compiles_recursive_error_pages_with_scope_inheritance() {
    let prefix = temp_prefix();
    fs::create_dir_all(&prefix).unwrap();
    let config_path = prefix.join("nginx.conf");
    fs::write(
        &config_path,
        r#"
            events {}
            http {
                recursive_error_pages on;
                server {
                    listen 127.0.0.1:18518;
                    recursive_error_pages off;
                    location /inherited {}
                    location /enabled {
                        recursive_error_pages on;
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
    let server = &config.http.as_ref().unwrap().servers[0];
    assert!(!server.context.recursive_error_pages);
    assert!(!server.locations[0].context.recursive_error_pages);
    assert!(server.locations[1].context.recursive_error_pages);
    let _ = fs::remove_dir_all(prefix);
}

#[test]
fn defaults_recursive_error_pages_to_off_and_rejects_duplicates() {
    let prefix = temp_prefix();
    fs::create_dir_all(&prefix).unwrap();
    let config_path = prefix.join("nginx.conf");
    fs::write(
        &config_path,
        "events {} http { server { listen 127.0.0.1:18519; } }\n",
    )
    .unwrap();
    let document = ConfigLoader::new(prefix.clone())
        .load(&config_path)
        .unwrap();
    let config = compile(document, prefix.clone(), config_path.clone()).unwrap();
    assert!(
        !config.http.as_ref().unwrap().servers[0]
            .context
            .recursive_error_pages
    );

    fs::write(
        &config_path,
        r#"
            events {}
            http {
                server {
                    listen 127.0.0.1:18519;
                    recursive_error_pages on;
                    recursive_error_pages off;
                }
            }
        "#,
    )
    .unwrap();
    let document = ConfigLoader::new(prefix.clone())
        .load(&config_path)
        .unwrap();
    let error = compile(document, prefix.clone(), config_path).unwrap_err();
    assert!(
        error
            .message
            .contains("\"recursive_error_pages\" directive is duplicate"),
        "{error}"
    );
    let _ = fs::remove_dir_all(prefix);
}

#[test]
fn error_page_declarations_replace_inherited_lists_per_scope() {
    let prefix = temp_prefix();
    fs::create_dir_all(&prefix).unwrap();
    let config_path = prefix.join("nginx.conf");
    fs::write(
        &config_path,
        r#"
            events {}
            http {
                error_page 404 /http-404;
                error_page 500 /http-500;
                server {
                    listen 127.0.0.1:18521;
                    error_page 502 /server-502;
                    location /inherited {}
                    location /replaced {
                        error_page 503 /location-503;
                        error_page 504 /location-504;
                    }
                }
                server {
                    listen 127.0.0.1:18522;
                    location /http-inherited {}
                }
            }
        "#,
    )
    .unwrap();
    let document = ConfigLoader::new(prefix.clone())
        .load(&config_path)
        .unwrap();
    let config = compile(document, prefix.clone(), config_path).unwrap();
    let http = config.http.as_ref().unwrap();

    let first = &http.servers[0];
    assert_eq!(first.context.error_pages.len(), 1);
    assert_eq!(first.context.error_pages[0].statuses, vec![502]);
    assert_eq!(first.locations[0].context.error_pages.len(), 1);
    assert_eq!(
        first.locations[0].context.error_pages[0].statuses,
        vec![502]
    );
    assert_eq!(
        first.locations[1]
            .context
            .error_pages
            .iter()
            .map(|page| page.statuses.clone())
            .collect::<Vec<_>>(),
        vec![vec![503], vec![504]]
    );

    let second = &http.servers[1];
    assert_eq!(
        second
            .context
            .error_pages
            .iter()
            .map(|page| page.statuses.clone())
            .collect::<Vec<_>>(),
        vec![vec![404], vec![500]],
        "HTTP-level error_page directives should both be inherited"
    );
    assert_eq!(second.locations[0].context.error_pages.len(), 2);
    let _ = fs::remove_dir_all(prefix);
}
