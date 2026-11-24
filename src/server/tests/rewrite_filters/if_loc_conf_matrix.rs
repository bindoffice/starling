use super::*;
use crate::config::{AccessLogConfig, AddHeader, HeaderInheritance};

fn access_log_path(log: &AccessLogConfig) -> &Path {
    log.logs
        .first()
        .and_then(|target| target.destination.as_fixed_path())
        .expect("fixed access log path")
}

fn header_names(headers: &[AddHeader]) -> Vec<&str> {
    headers.iter().map(|header| header.name.as_str()).collect()
}

#[test]
fn if_snapshots_cover_core_headers_filters_and_logs() {
    let (prefix, config) = test_config(
        r#"
                worker_processes 1;
                events { worker_connections 64; }
                http {
                    server {
                        listen 127.0.0.1:18361;
                        location /all {
                            access_log logs/parent-access.log;
                            add_header X-Parent parent;
                            add_trailer X-Parent-Trailer parent;
                            add_header_inherit merge;
                            add_trailer_inherit merge;
                            expires 1h;
                            ssi on;
                            gzip on;
                            charset utf-8;
                            source_charset utf-8;
                            override_charset on;
                            root html/parent;
                            sendfile on;
                            limit_rate 1k;
                            limit_rate_after 2k;
                            error_page 404 /parent-error;
                            post_action /parent-post;
                            rewrite_log on;
                            uninitialized_variable_warn on;
                            if ($arg_branch = yes) {
                                access_log logs/branch-access.log;
                                add_header X-Branch branch;
                                add_trailer X-Branch-Trailer branch;
                                add_header_inherit off;
                                add_trailer_inherit off;
                                expires max;
                                ssi off;
                                gzip off;
                                charset koi8-r;
                                source_charset koi8-r;
                                override_charset off;
                                root html/branch;
                                sendfile off;
                                limit_rate 3k;
                                limit_rate_after 4k;
                                error_page 500 /branch-error;
                                post_action /branch-post;
                                rewrite_log off;
                                uninitialized_variable_warn off;
                            }
                            if ($arg_reset = yes) {
                            }
                        }
                    }
                }
            "#,
        &[],
    );
    let server = &config.http.as_ref().unwrap().servers[0];
    let location = match_location(&server.locations, "/all").expect("if location");
    let parent = &location.context;
    let branch = &parent.conditionals[0];
    let reset = &parent.conditionals[1];

    assert_eq!(branch.root, prefix.join("html/branch"));
    assert_eq!(reset.root, parent.root);
    assert!(!branch.sendfile);
    assert_eq!(reset.sendfile, parent.sendfile);
    assert_eq!(branch.limit_rate.as_deref(), Some("3k"));
    assert_eq!(reset.limit_rate, parent.limit_rate);
    assert_eq!(branch.limit_rate_after.as_deref(), Some("4k"));
    assert_eq!(reset.limit_rate_after, parent.limit_rate_after);
    assert_eq!(branch.error_pages[0].statuses, [500]);
    assert_eq!(
        reset.error_pages[0].statuses,
        parent.error_pages[0].statuses
    );
    assert_eq!(branch.post_action.as_deref(), Some("/branch-post"));
    assert_eq!(reset.post_action, parent.post_action);

    assert_eq!(
        access_log_path(&branch.access_log),
        prefix.join("logs/branch-access.log")
    );
    assert_eq!(
        access_log_path(&reset.access_log),
        prefix.join("logs/parent-access.log")
    );
    assert_eq!(header_names(&branch.add_headers), ["X-Branch"]);
    assert_eq!(header_names(&reset.add_headers), ["X-Parent"]);
    assert_eq!(header_names(&branch.add_trailers), ["X-Branch-Trailer"]);
    assert_eq!(header_names(&reset.add_trailers), ["X-Parent-Trailer"]);
    assert_eq!(branch.add_header_inherit, HeaderInheritance::Off);
    assert_eq!(reset.add_header_inherit, HeaderInheritance::Merge);
    assert_eq!(branch.add_trailer_inherit, HeaderInheritance::Off);
    assert_eq!(reset.add_trailer_inherit, HeaderInheritance::Merge);
    assert_ne!(
        format!("{:?}", branch.expires),
        format!("{:?}", parent.expires)
    );
    assert_eq!(
        format!("{:?}", reset.expires),
        format!("{:?}", parent.expires)
    );

    assert!(!branch.ssi.enabled);
    assert_eq!(reset.ssi, parent.ssi);
    assert!(!branch.gzip);
    assert_eq!(reset.gzip, parent.gzip);
    assert_eq!(branch.charset.charset.as_deref(), Some("koi8-r"));
    assert_eq!(branch.charset.source_charset.as_deref(), Some("koi8-r"));
    assert!(!branch.charset.override_charset);
    assert_eq!(reset.charset, parent.charset);
    assert!(!branch.rewrite_log);
    assert!(reset.rewrite_log);
    assert!(!branch.uninitialized_variable_warn);
    assert!(reset.uninitialized_variable_warn);
    let _ = std::fs::remove_dir_all(prefix);
}

#[test]
fn if_snapshots_cover_every_gateway_pass() {
    let (prefix, config) = test_config(
        r#"
                worker_processes 1;
                events { worker_connections 64; }
                http {
                    server {
                        listen 127.0.0.1:18362;
                        location /proxy {
                            proxy_pass http://127.0.0.1:19001;
                            if ($arg_branch = yes) {
                                proxy_pass http://127.0.0.1:19002;
                            }
                            if ($arg_reset = yes) {
                            }
                        }
                        location /grpc {
                            grpc_pass grpc://127.0.0.1:19003;
                            if ($arg_branch = yes) {
                                grpc_pass grpc://127.0.0.1:19004;
                            }
                            if ($arg_reset = yes) {
                            }
                        }
                        location /fastcgi {
                            fastcgi_pass 127.0.0.1:19005;
                            if ($arg_branch = yes) {
                                fastcgi_pass 127.0.0.1:19006;
                            }
                            if ($arg_reset = yes) {
                            }
                        }
                        location /uwsgi {
                            uwsgi_pass 127.0.0.1:19007;
                            if ($arg_branch = yes) {
                                uwsgi_pass 127.0.0.1:19008;
                            }
                            if ($arg_reset = yes) {
                            }
                        }
                        location /scgi {
                            scgi_pass 127.0.0.1:19009;
                            if ($arg_branch = yes) {
                                scgi_pass 127.0.0.1:19010;
                            }
                            if ($arg_reset = yes) {
                            }
                        }
                        location /memcached {
                            memcached_pass 127.0.0.1:19011;
                            if ($arg_branch = yes) {
                                memcached_pass 127.0.0.1:19012;
                            }
                            if ($arg_reset = yes) {
                            }
                        }
                        location /tunnel {
                            tunnel_pass 127.0.0.1:19013;
                            if ($arg_branch = yes) {
                                tunnel_pass 127.0.0.1:19014;
                            }
                            if ($arg_reset = yes) {
                            }
                        }
                    }
                }
            "#,
        &[],
    );
    let server = &config.http.as_ref().unwrap().servers[0];

    let proxy = match_location(&server.locations, "/proxy").expect("proxy location");
    assert_eq!(
        proxy
            .context
            .proxy_pass
            .as_ref()
            .map(|pass| pass.raw.as_str()),
        Some("http://127.0.0.1:19001")
    );
    assert_eq!(
        proxy.context.conditionals[0]
            .proxy_pass
            .as_ref()
            .map(|pass| pass.raw.as_str()),
        Some("http://127.0.0.1:19002")
    );
    assert_eq!(
        proxy.context.conditionals[1]
            .proxy_pass
            .as_ref()
            .map(|pass| pass.raw.as_str()),
        Some("http://127.0.0.1:19001")
    );

    let grpc = match_location(&server.locations, "/grpc").expect("grpc location");
    assert_eq!(
        grpc.context
            .grpc
            .pass
            .as_ref()
            .map(|pass| pass.raw.as_str()),
        Some("grpc://127.0.0.1:19003")
    );
    assert_eq!(
        grpc.context.conditionals[0]
            .grpc_pass
            .as_ref()
            .map(|pass| pass.raw.as_str()),
        Some("grpc://127.0.0.1:19004")
    );
    assert_eq!(
        grpc.context.conditionals[1]
            .grpc_pass
            .as_ref()
            .map(|pass| pass.raw.as_str()),
        Some("grpc://127.0.0.1:19003")
    );

    let fastcgi = match_location(&server.locations, "/fastcgi").expect("fastcgi location");
    assert_eq!(
        fastcgi
            .context
            .fastcgi_pass
            .as_ref()
            .map(|pass| pass.raw.as_str()),
        Some("127.0.0.1:19005")
    );
    assert_eq!(
        fastcgi.context.conditionals[0]
            .fastcgi_pass
            .as_ref()
            .map(|pass| pass.raw.as_str()),
        Some("127.0.0.1:19006")
    );
    assert_eq!(
        fastcgi.context.conditionals[1]
            .fastcgi_pass
            .as_ref()
            .map(|pass| pass.raw.as_str()),
        Some("127.0.0.1:19005")
    );

    let uwsgi = match_location(&server.locations, "/uwsgi").expect("uwsgi location");
    assert_eq!(
        uwsgi
            .context
            .uwsgi_pass
            .as_ref()
            .map(|pass| pass.raw.as_str()),
        Some("127.0.0.1:19007")
    );
    assert_eq!(
        uwsgi.context.conditionals[0]
            .uwsgi_pass
            .as_ref()
            .map(|pass| pass.raw.as_str()),
        Some("127.0.0.1:19008")
    );
    assert_eq!(
        uwsgi.context.conditionals[1]
            .uwsgi_pass
            .as_ref()
            .map(|pass| pass.raw.as_str()),
        Some("127.0.0.1:19007")
    );

    let scgi = match_location(&server.locations, "/scgi").expect("scgi location");
    assert_eq!(
        scgi.context
            .scgi_pass
            .as_ref()
            .map(|pass| pass.raw.as_str()),
        Some("127.0.0.1:19009")
    );
    assert_eq!(
        scgi.context.conditionals[0]
            .scgi_pass
            .as_ref()
            .map(|pass| pass.raw.as_str()),
        Some("127.0.0.1:19010")
    );
    assert_eq!(
        scgi.context.conditionals[1]
            .scgi_pass
            .as_ref()
            .map(|pass| pass.raw.as_str()),
        Some("127.0.0.1:19009")
    );

    let memcached = match_location(&server.locations, "/memcached").expect("memcached location");
    assert_eq!(
        memcached
            .context
            .memcached_pass
            .as_ref()
            .map(|pass| pass.raw.as_str()),
        Some("127.0.0.1:19011")
    );
    assert_eq!(
        memcached.context.conditionals[0]
            .memcached_pass
            .as_ref()
            .map(|pass| pass.raw.as_str()),
        Some("127.0.0.1:19012")
    );
    assert_eq!(
        memcached.context.conditionals[1]
            .memcached_pass
            .as_ref()
            .map(|pass| pass.raw.as_str()),
        Some("127.0.0.1:19011")
    );

    let tunnel = match_location(&server.locations, "/tunnel").expect("tunnel location");
    assert_eq!(
        tunnel
            .context
            .tunnel
            .pass
            .as_ref()
            .map(|pass| pass.raw.as_str()),
        Some("127.0.0.1:19013")
    );
    assert_eq!(
        tunnel.context.conditionals[0]
            .tunnel_pass
            .as_ref()
            .map(|pass| pass.raw.as_str()),
        Some("127.0.0.1:19014")
    );
    assert_eq!(
        tunnel.context.conditionals[1]
            .tunnel_pass
            .as_ref()
            .map(|pass| pass.raw.as_str()),
        Some("127.0.0.1:19013")
    );
    let _ = std::fs::remove_dir_all(prefix);
}
