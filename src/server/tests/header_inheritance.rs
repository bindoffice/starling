use super::support::*;
use crate::config::HeaderInheritance;

fn inheritance_config(port: u16) -> String {
    format!(
        r#"
                worker_processes 1;
                events {{ worker_connections 64; }}
                http {{
                    add_header X-Parent parent;
                    add_header_inherit merge;
                    add_trailer X-Parent parent;
                    add_trailer_inherit merge;
                    server {{
                        listen 127.0.0.1:{port};
                        add_header X-Server server;
                        add_trailer X-Server server;
                        location /merge {{
                            add_header X-Location location;
                            add_trailer X-Location location;
                            return 200 "merge\n";
                        }}
                        location /off {{
                            add_header_inherit off;
                            add_trailer_inherit off;
                            add_header X-Location location;
                            add_trailer X-Location location;
                            return 200 "off\n";
                        }}
                        location /on {{
                            add_header_inherit on;
                            add_trailer_inherit on;
                            add_header X-Location location;
                            add_trailer X-Location location;
                            return 200 "on\n";
                        }}
                    }}
                }}
            "#
    )
}

#[test]
fn compiles_header_and_trailer_inheritance_modes() {
    let (prefix, config) = test_config(&inheritance_config(18177), &[]);
    let server = &config.http.as_ref().unwrap().servers[0];
    assert_eq!(server.context.add_header_inherit, HeaderInheritance::Merge);
    assert_eq!(server.context.add_trailer_inherit, HeaderInheritance::Merge);
    assert_eq!(
        server
            .context
            .add_headers
            .iter()
            .map(|header| header.name.as_str())
            .collect::<Vec<_>>(),
        ["X-Server", "X-Parent"]
    );
    assert_eq!(
        server.locations[0].context.add_header_inherit,
        HeaderInheritance::Merge
    );
    assert_eq!(
        server.locations[0]
            .context
            .add_headers
            .iter()
            .map(|header| header.name.as_str())
            .collect::<Vec<_>>(),
        ["X-Location", "X-Server", "X-Parent"]
    );
    assert_eq!(
        server.locations[1].context.add_header_inherit,
        HeaderInheritance::Off
    );
    assert_eq!(server.locations[1].context.add_headers.len(), 1);
    assert_eq!(
        server.locations[2].context.add_header_inherit,
        HeaderInheritance::On
    );
    assert_eq!(server.locations[2].context.add_headers.len(), 1);
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn applies_header_and_trailer_inheritance_modes() {
    let (prefix, config) = test_config(&inheritance_config(18178), &[]);
    let merged = request_body(
        &config,
        "GET /merge HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(merged.contains("X-Location: location\r\n"), "{merged}");
    assert!(merged.contains("X-Server: server\r\n"), "{merged}");
    assert!(merged.contains("X-Parent: parent\r\n"), "{merged}");
    assert!(
        merged.contains("X-Location: location\r\nX-Server: server\r\n"),
        "{merged}"
    );
    assert!(
        merged.contains("X-Server: server\r\nX-Parent: parent\r\n"),
        "{merged}"
    );
    let merged_trailers = merged.split_once("\r\n0\r\n").unwrap().1;
    assert!(
        merged_trailers
            .starts_with("X-Location: location\r\nX-Server: server\r\nX-Parent: parent\r\n"),
        "{merged_trailers}"
    );

    let off = request_body(
        &config,
        "GET /off HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(off.contains("X-Location: location\r\n"), "{off}");
    assert!(!off.contains("X-Server: server\r\n"), "{off}");
    assert!(!off.contains("X-Parent: parent\r\n"), "{off}");
    let off_trailers = off.split_once("\r\n0\r\n").unwrap().1;
    assert_eq!(off_trailers, "X-Location: location\r\n\r\n");

    let on = request_body(
        &config,
        "GET /on HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(on.contains("X-Location: location\r\n"), "{on}");
    assert!(!on.contains("X-Server: server\r\n"), "{on}");
    assert!(!on.contains("X-Parent: parent\r\n"), "{on}");
    let on_trailers = on.split_once("\r\n0\r\n").unwrap().1;
    assert_eq!(on_trailers, "X-Location: location\r\n\r\n");
    let _ = std::fs::remove_dir_all(prefix);
}

#[test]
fn rejects_invalid_or_duplicate_inheritance_directives() {
    let invalid = test_config_error(
        r#"
                events { worker_connections 64; }
                http {
                    add_header_inherit inherit;
                }
            "#,
    );
    assert!(
        invalid.contains("invalid value \"inherit\" in \"add_header_inherit\" directive"),
        "{invalid}"
    );

    let duplicate = test_config_error(
        r#"
                events { worker_connections 64; }
                http {
                    add_trailer_inherit on;
                    add_trailer_inherit merge;
                }
            "#,
    );
    assert!(
        duplicate.contains("\"add_trailer_inherit\" directive is duplicate"),
        "{duplicate}"
    );

    let server_if = test_config_error(
        r#"
                events { worker_connections 64; }
                http {
                    server {
                        listen 127.0.0.1:18183;
                        if ($arg_condition = yes) {
                            add_header X-Test value;
                        }
                    }
                }
            "#,
    );
    assert!(
        server_if.contains("\"add_header\" directive is not allowed here"),
        "{server_if}"
    );

    let duplicate_if = test_config_error(
        r#"
                events { worker_connections 64; }
                http {
                    server {
                        listen 127.0.0.1:18184;
                        location / {
                            if ($arg_condition = yes) {
                                add_header_inherit on;
                                add_header_inherit merge;
                            }
                        }
                    }
                }
            "#,
    );
    assert!(
        duplicate_if.contains("\"add_header_inherit\" directive is duplicate"),
        "{duplicate_if}"
    );
}

#[test]
fn compiles_if_header_and_trailer_inheritance() {
    let (prefix, config) = test_config(
        r#"
                events { worker_connections 64; }
                http {
                    add_header X-Parent parent;
                    add_header_inherit merge;
                    add_trailer X-Parent parent;
                    add_trailer_inherit merge;
                    server {
                        listen 127.0.0.1:18181;
                        add_header X-Server server;
                        add_trailer X-Server server;
                        location /if {
                            add_header X-Location location;
                            add_trailer X-Location location;
                            if ($arg_mode = merge) {
                                add_header_inherit merge;
                                add_trailer_inherit merge;
                                add_header X-Conditional conditional;
                                add_trailer X-Conditional conditional;
                            }
                            if ($arg_mode = off) {
                                add_header_inherit off;
                                add_trailer_inherit off;
                            }
                        }
                    }
                }
            "#,
        &[],
    );
    let conditionals = &config.http.as_ref().unwrap().servers[0].locations[0]
        .context
        .conditionals;
    assert_eq!(
        conditionals[0]
            .add_headers
            .iter()
            .map(|header| header.name.as_str())
            .collect::<Vec<_>>(),
        ["X-Conditional", "X-Location", "X-Server", "X-Parent"]
    );
    assert_eq!(conditionals[0].add_header_inherit, HeaderInheritance::Merge);
    assert_eq!(
        conditionals[0]
            .add_trailers
            .iter()
            .map(|trailer| trailer.name.as_str())
            .collect::<Vec<_>>(),
        ["X-Conditional", "X-Location", "X-Server", "X-Parent"]
    );
    assert_eq!(
        conditionals[0].add_trailer_inherit,
        HeaderInheritance::Merge
    );
    assert!(conditionals[1].add_headers.is_empty());
    assert_eq!(conditionals[1].add_header_inherit, HeaderInheritance::Off);
    assert!(conditionals[1].add_trailers.is_empty());
    assert_eq!(conditionals[1].add_trailer_inherit, HeaderInheritance::Off);
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn applies_if_header_and_trailer_inheritance() {
    let (prefix, config) = test_config(
        r#"
                events { worker_connections 64; }
                http {
                    add_header X-Parent parent;
                    add_header_inherit merge;
                    add_trailer X-Parent parent;
                    add_trailer_inherit merge;
                    server {
                        listen 127.0.0.1:18182;
                        add_header X-Server server;
                        add_trailer X-Server server;
                        location /on {
                            add_header_inherit on;
                            add_trailer_inherit on;
                            add_header X-Location location;
                            add_trailer X-Location location;
                            if ($arg_branch = on) {
                                add_header X-Conditional conditional;
                                add_trailer X-Conditional conditional;
                            }
                            return 200 "on\n";
                        }
                        location /merge {
                            add_header_inherit merge;
                            add_trailer_inherit merge;
                            add_header X-Location location;
                            add_trailer X-Location location;
                            if ($arg_branch = merge) {
                                add_header_inherit merge;
                                add_trailer_inherit merge;
                                add_header X-Conditional conditional;
                                add_trailer X-Conditional conditional;
                            }
                            return 200 "merge\n";
                        }
                        location /off {
                            add_header X-Location location;
                            add_trailer X-Location location;
                            if ($arg_branch = off) {
                                add_header_inherit off;
                                add_trailer_inherit off;
                            }
                            return 200 "off\n";
                        }
                    }
                }
            "#,
        &[],
    );

    let on = request_body(
        &config,
        "GET /on?branch=on HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(on.contains("X-Conditional: conditional\r\n"), "{on}");
    assert!(!on.contains("X-Location: location\r\n"), "{on}");
    assert!(!on.contains("X-Server: server\r\n"), "{on}");
    let on_trailers = on.split_once("\r\n0\r\n").unwrap().1;
    assert_eq!(on_trailers, "X-Conditional: conditional\r\n\r\n");

    let merged = request_body(
        &config,
        "GET /merge?branch=merge HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(
        merged.contains(
            "X-Conditional: conditional\r\nX-Location: location\r\nX-Server: server\r\nX-Parent: parent\r\n"
        ),
        "{merged}"
    );
    let merged_trailers = merged.split_once("\r\n0\r\n").unwrap().1;
    assert_eq!(
        merged_trailers,
        "X-Conditional: conditional\r\nX-Location: location\r\nX-Server: server\r\nX-Parent: parent\r\n\r\n"
    );

    let off = request_body(
        &config,
        "GET /off?branch=off HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(!off.contains("X-Conditional:"), "{off}");
    assert!(!off.contains("X-Location:"), "{off}");
    assert!(!off.contains("X-Server:"), "{off}");
    assert!(!off.contains("X-Parent:"), "{off}");
    let _ = std::fs::remove_dir_all(prefix);
}
