use super::*;
use starling::config::LocationMatcher;

fn compile_site(
    name: &str,
    source: &str,
    files: &[(&str, &str)],
) -> (PathBuf, starling::config::RuntimeConfig) {
    let prefix = temp_prefix();
    fs::create_dir_all(prefix.join("conf")).unwrap();
    for (path, contents) in files {
        let path = prefix.join("conf").join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
    }
    let config_path = prefix.join("conf").join(name);
    fs::write(&config_path, source).unwrap();
    let document = ConfigLoader::new(prefix.clone())
        .load(&config_path)
        .expect("load real-world configuration");
    let config =
        compile(document, prefix.clone(), config_path).expect("compile real-world configuration");
    (prefix, config)
}

#[test]
fn compiles_a_wordpress_style_php_site() {
    let (prefix, config) = compile_site(
        "wordpress.conf",
        r#"
            events { worker_connections 1024; }
            http {
                include mime.types;
                client_max_body_size 64m;
                server {
                    listen 8080;
                    server_name example.com;
                    root html;
                    index index.php index.html;
                    location / { try_files $uri $uri/ /index.php?$args; }
                    location ~ \.php$ {
                        fastcgi_split_path_info ^(.+\.php)(/.+)$;
                        try_files $fastcgi_script_name =404;
                        include fastcgi_params;
                        fastcgi_param SCRIPT_FILENAME $document_root$fastcgi_script_name;
                        fastcgi_param PATH_INFO $fastcgi_path_info;
                        fastcgi_pass unix:run/php-fpm.sock;
                    }
                    location ~* \.(js|css|png|svg|woff2)$ {
                        expires max;
                        access_log off;
                    }
                }
            }
        "#,
        &[
            ("mime.types", "types { text/html html; image/png png; }"),
            (
                "fastcgi_params",
                "fastcgi_param QUERY_STRING $query_string;\n\
                 fastcgi_param REQUEST_METHOD $request_method;\n\
                 fastcgi_param SCRIPT_NAME $fastcgi_script_name;\n",
            ),
        ],
    );
    let server = &config.http.as_ref().unwrap().servers[0];
    assert_eq!(server.locations.len(), 3);
    assert_eq!(server.locations[1].context.fastcgi_params.len(), 5);
    let _ = fs::remove_dir_all(prefix);
}

#[test]
fn supports_laravel_negative_lookahead_location_regexes() {
    let (prefix, config) = compile_site(
        "laravel.conf",
        r#"
            events { worker_connections 512; }
            http {
                server {
                    listen 8081;
                    root html/public;
                    location / { try_files $uri $uri/ /index.php?$query_string; }
                    location ~ /\.(?!well-known).* { deny all; }
                }
            }
        "#,
        &[],
    );
    let server = &config.http.as_ref().unwrap().servers[0];
    let LocationMatcher::Regex(regex) = &server.locations[1].matcher else {
        panic!("expected a regular-expression location");
    };
    assert!(regex.is_match("/.env"));
    assert!(regex.is_match("/.git/config"));
    assert!(!regex.is_match("/.well-known/acme-challenge/token"));
    let _ = fs::remove_dir_all(prefix);
}

#[test]
fn compiles_an_spa_with_api_proxy_and_websocket_upgrade_map() {
    let (prefix, config) = compile_site(
        "spa.conf",
        r#"
            events { worker_connections 512; }
            http {
                map $http_upgrade $connection_upgrade { default upgrade; '' close; }
                upstream api { server 127.0.0.1:9000; keepalive 32; }
                server {
                    listen 8082;
                    root html;
                    location / { try_files $uri $uri/ /index.html; }
                    location /assets/ {
                        expires 1y;
                        add_header Cache-Control "public, immutable";
                    }
                    location /api/ {
                        proxy_pass http://api;
                        proxy_http_version 1.1;
                        proxy_set_header Upgrade $http_upgrade;
                        proxy_set_header Connection $connection_upgrade;
                        proxy_buffering off;
                    }
                }
            }
        "#,
        &[],
    );
    let http = config.http.as_ref().unwrap();
    assert!(http.maps.contains_key("connection_upgrade"));
    assert!(http.upstreams.contains_key("api"));
    assert_eq!(http.servers[0].locations.len(), 3);
    let _ = fs::remove_dir_all(prefix);
}

#[test]
fn compiles_a_production_style_reverse_proxy_and_php_config() {
    let (prefix, config) = compile_site(
        "nginx.conf",
        r#"
            user www-data;
            worker_processes auto;
            worker_rlimit_nofile 65535;
            pid logs/nginx.pid;
            error_log logs/error.log warn;

            events {
                worker_connections 8192;
                multi_accept on;
                use epoll;
            }

            http {
                include mime.types;
                default_type application/octet-stream;
                log_format main '$remote_addr "$request" $status $body_bytes_sent';
                access_log logs/access.log main buffer=32k flush=5s;
                sendfile on;
                tcp_nopush on;
                tcp_nodelay on;
                keepalive_timeout 65;
                keepalive_requests 1000;
                types_hash_max_size 2048;
                server_tokens off;
                client_max_body_size 64m;
                client_body_timeout 60s;
                client_header_timeout 60s;
                send_timeout 60s;
                large_client_header_buffers 4 16k;
                open_file_cache max=10000 inactive=20s;
                open_file_cache_valid 30s;
                open_file_cache_min_uses 2;
                gzip on;
                gzip_comp_level 5;
                gzip_min_length 1024;
                gzip_types text/css application/javascript application/json;
                gzip_vary on;
                gzip_window 32k;
                gzip_hash 4096;
                gzip_no_buffer on;
                postpone_gzipping 1k;

                limit_req_zone $binary_remote_addr zone=api:10m rate=20r/s;
                limit_conn_zone $binary_remote_addr zone=addr:10m;

                map $http_upgrade $connection_upgrade {
                    default upgrade;
                    '' close;
                }

                proxy_cache_path cache levels=1:2 keys_zone=proxy_cache:16m
                                 max_size=1g inactive=60m use_temp_path=off;
                fastcgi_cache_path cache/fastcgi levels=1:2
                                   keys_zone=fastcgi_cache:16m inactive=60m;

                upstream app_backend {
                    server 127.0.0.1:9000 max_fails=3 fail_timeout=30s;
                    keepalive 32;
                }

                server {
                    listen 8080 default_server;
                    listen [::]:8080 default_server;
                    server_name example.com www.example.com;
                    root html;
                    index index.html index.php;
                    charset utf-8;
                    error_page 404 /404.html;
                    location = /404.html { internal; }
                    location / { try_files $uri $uri/ /index.php?$query_string; }
                    location /api/ {
                        limit_req zone=api burst=40 nodelay;
                        limit_conn addr 20;
                        proxy_pass http://app_backend;
                        proxy_http_version 1.1;
                        proxy_set_header Host $host;
                        proxy_set_header X-Real-IP $remote_addr;
                        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
                        proxy_set_header X-Forwarded-Proto $scheme;
                        proxy_set_header Upgrade $http_upgrade;
                        proxy_set_header Connection $connection_upgrade;
                        proxy_send_lowat 12000;
                        proxy_cache proxy_cache;
                        proxy_cache_valid 200 302 10m;
                        proxy_buffering on;
                        proxy_buffers 16 16k;
                        proxy_buffer_size 16k;
                        proxy_busy_buffers_size 32k;
                    }
                    location ~ \.php$ {
                        fastcgi_split_path_info ^(.+\.php)(/.+)$;
                        try_files $fastcgi_script_name =404;
                        include fastcgi_params;
                        fastcgi_param SCRIPT_FILENAME $document_root$fastcgi_script_name;
                        fastcgi_send_lowat 12000;
                        fastcgi_cache fastcgi_cache;
                        fastcgi_cache_valid 200 60m;
                        fastcgi_pass unix:run/php-fpm.sock;
                    }
                    location ~* \.(?:css|js|png|svg|woff2)$ {
                        expires 30d;
                        add_header Cache-Control "public, immutable";
                        access_log off;
                    }
                }
            }
        "#,
        &[
            (
                "mime.types",
                "types { text/html html htm; text/css css; application/javascript js; }",
            ),
            (
                "fastcgi_params",
                "fastcgi_param QUERY_STRING $query_string;\n\
                 fastcgi_param REQUEST_METHOD $request_method;\n\
                 fastcgi_param SCRIPT_FILENAME $document_root$fastcgi_script_name;\n",
            ),
        ],
    );

    assert!(matches!(
        config.core.worker_processes,
        starling::config::WorkerProcesses::Auto
    ));
    let http = config.http.as_ref().unwrap();
    assert!(http.limit_req_zones.contains_key("api"));
    assert!(http.limit_conn_zones.contains_key("addr"));
    assert!(http.maps.contains_key("connection_upgrade"));
    assert!(http.upstreams.contains_key("app_backend"));
    assert_eq!(http.servers.len(), 1);
    assert_eq!(http.servers[0].locations.len(), 5);
    let api = http.servers[0]
        .locations
        .iter()
        .find(|location| {
            matches!(
                &location.matcher,
                starling::config::LocationMatcher::Prefix(path) if path == "/api/"
            )
        })
        .expect("API location");
    assert_eq!(api.context.limit_conns[0].zone, "addr");
    assert_eq!(api.context.limit_conns[0].connections, 20);
    assert_eq!(api.context.limit_reqs[0].burst, 40);
    let _ = fs::remove_dir_all(prefix);
}
