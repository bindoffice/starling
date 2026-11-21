use super::support::{RunningServer, TestDirectory, assert_status, reserve_port};

#[test]
#[ignore = "requires local TCP bind; run with --ignored"]
fn selects_default_alias_and_regex_virtual_hosts() {
    let port = reserve_port();
    let directory = TestDirectory::new("virtual-hosts");
    directory.write("html/default/index.html", "default-site");
    directory.write("html/one/index.html", "one-site");
    directory.write("html/api/index.html", "api-site");
    let config = virtual_host_config(port);
    let server = RunningServer::start(directory, port, &config);

    let default = server.get_with_host("/", "unknown.test");
    assert_status(&default, 200);
    assert!(
        default.contains("default-site"),
        "default virtual host was not selected:\n{default}"
    );

    let exact = server.get_with_host("/", "one.test");
    assert_status(&exact, 200);
    assert!(
        exact.contains("one-site"),
        "exact server_name was not selected:\n{exact}"
    );

    let alias = server.get_with_host("/", &format!("www.one.test:{port}"));
    assert_status(&alias, 200);
    assert!(
        alias.contains("one-site"),
        "server_name alias or Host port stripping failed:\n{alias}"
    );

    let regex = server.get_with_host("/", "api42.test");
    assert_status(&regex, 200);
    assert!(
        regex.contains("api-site"),
        "regex server_name was not selected:\n{regex}"
    );

    let selected_name = server.get_with_host("/whoami", "www.one.test");
    assert_status(&selected_name, 200);
    assert!(
        selected_name.contains("one.test|www.one.test"),
        "selected server_name variable is incorrect:\n{selected_name}"
    );
}

fn virtual_host_config(port: u16) -> String {
    format!(
        r#"
            daemon off;
            worker_processes 1;
            pid logs/nginx.pid;
            error_log logs/error.log notice;

            events {{
                worker_connections 128;
            }}

            http {{
                default_type text/plain;
                access_log logs/access.log;

                server {{
                    listen 127.0.0.1:{port} default_server;
                    server_name default.test;
                    root html/default;
                    index index.html;

                    location / {{
                        try_files $uri $uri/ =404;
                    }}
                }}

                server {{
                    listen 127.0.0.1:{port};
                    server_name one.test www.one.test;
                    root html/one;
                    index index.html;

                    location / {{
                        try_files $uri $uri/ =404;
                    }}

                    location = /whoami {{
                        return 200 "$server_name|$host";
                    }}
                }}

                server {{
                    listen 127.0.0.1:{port};
                    server_name ~^api[0-9]+\.test$;
                    root html/api;
                    index index.html;

                    location / {{
                        try_files $uri $uri/ =404;
                    }}
                }}
            }}
        "#
    )
}
