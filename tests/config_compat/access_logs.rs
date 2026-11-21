use super::*;

#[test]
fn compiles_multiple_access_logs_with_nginx_inheritance() {
    let prefix = temp_prefix();
    fs::create_dir_all(&prefix).unwrap();
    let config_path = prefix.join("nginx.conf");
    fs::write(
        &config_path,
        r#"
            events {}
            http {
                log_format compact '$status';
                access_log logs/parent.log combined;
                access_log logs/parent-second.log compact if=$arg_log;
                server {
                    listen 127.0.0.1:18093;
                    location / { return 204; }
                }
                server {
                    listen 127.0.0.1:18094;
                    access_log logs/first.log compact if=$arg_log;
                    access_log logs/second.log combined;
                    location /inherited { return 204; }
                    location /off {
                        access_log off;
                        return 204;
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
    let http = config.http.as_ref().unwrap();

    assert_eq!(http.servers[0].context.access_log.logs.len(), 2);
    assert_eq!(
        http.servers[0].context.access_log.logs[0]
            .destination
            .as_fixed_path(),
        Some(prefix.join("logs/parent.log").as_path())
    );
    assert_eq!(
        http.servers[0].context.access_log.logs[1]
            .condition
            .as_deref(),
        Some("$arg_log")
    );

    let server = &http.servers[1];
    assert_eq!(server.context.access_log.logs.len(), 2);
    assert_eq!(
        server.context.access_log.logs[0]
            .destination
            .as_fixed_path(),
        Some(prefix.join("logs/first.log").as_path())
    );
    assert_eq!(
        server.context.access_log.logs[0].condition.as_deref(),
        Some("$arg_log")
    );
    assert_eq!(
        server.context.access_log.logs[1]
            .destination
            .as_fixed_path(),
        Some(prefix.join("logs/second.log").as_path())
    );
    assert_eq!(server.locations[0].context.access_log.logs.len(), 2);
    assert!(server.locations[1].context.access_log.off);
    assert!(server.locations[1].context.access_log.logs.is_empty());
    let _ = fs::remove_dir_all(prefix);
}
