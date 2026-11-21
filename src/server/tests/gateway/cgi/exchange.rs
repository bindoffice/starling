use super::*;

#[tokio::test]
async fn exchanges_fastcgi_records_and_maps_cgi_headers() {
    let (prefix, config) = test_config(
        r#"
                worker_processes 1;
                events { worker_connections 64; }
                http {
                    server {
                        listen 127.0.0.1:18120;
                        location /php/ {
                            fastcgi_pass 127.0.0.1:9000;
                            fastcgi_split_path_info ^(.+?\.php)(/.*)$;
                            fastcgi_param SCRIPT_FILENAME /srv$fastcgi_script_name;
                            fastcgi_param PATH_INFO $fastcgi_path_info;
                            fastcgi_param QUERY_STRING $query_string;
                            fastcgi_param CONTENT_TYPE $content_type if_not_empty;
                            fastcgi_keep_conn on;
                        }
                    }
                }
            "#,
        &[],
    );
    let server = &config.http.as_ref().unwrap().servers[0];
    let context = &server.locations[0].context;
    let request = Request {
        method: "POST".into(),
        target: "/php/demo.php/extra/path?x=1".into(),
        version: "HTTP/1.1".into(),
        headers: vec![
            Header {
                name: "Host".into(),
                value: "localhost".into(),
            },
            Header {
                name: "Content-Type".into(),
                value: "text/plain".into(),
            },
            Header {
                name: "Content-Length".into(),
                value: "5".into(),
            },
            Header {
                name: "X-Custom".into(),
                value: "forwarded".into(),
            },
            Header {
                name: "X-Custom".into(),
                value: "again".into(),
            },
        ],
        body_stream: None,
        body: b"hello".to_vec(),
        remote_addr: "127.0.0.1:45678".into(),
        local_addr: "127.0.0.1:18120".into(),
        keep_alive: false,
    };
    let remote: SocketAddr = "127.0.0.1:45678".parse().unwrap();
    let local: SocketAddr = "127.0.0.1:18120".parse().unwrap();
    let mut variables = Variables::new(&request, "localhost", server, &local, &remote);
    let env = RequestEnv {
        uri: "/php/demo.php/extra/path".into(),
        args: Some("x=1".into()),
        unparsed_uri: Some("/php/demo.php/extra/path?x=1".into()),
        internal: false,
        bypass_try_files: false,
        limits_checked: false,
        auth_request_checked: false,
        mirror_checked: false,
    };
    refresh_variables(&env, Some(&server.locations[0]), &mut variables, context);
    let (client, mut fastcgi) = tokio::io::duplex(64 * 1024);
    let server_task = tokio::spawn(async move {
        async fn read_record<R: AsyncRead + Unpin>(reader: &mut R) -> (u8, Vec<u8>) {
            let mut header = [0_u8; 8];
            reader.read_exact(&mut header).await.unwrap();
            let length = u16::from_be_bytes([header[4], header[5]]) as usize;
            let padding = header[6] as usize;
            let mut content = vec![0_u8; length];
            reader.read_exact(&mut content).await.unwrap();
            if padding > 0 {
                let mut padding = vec![0_u8; padding];
                reader.read_exact(&mut padding).await.unwrap();
            }
            (header[1], content)
        }

        let mut parameters = Vec::new();
        let mut body = Vec::new();
        let mut begin_flags = None;
        loop {
            let (record_type, content) = read_record(&mut fastcgi).await;
            match record_type {
                FCGI_BEGIN_REQUEST => begin_flags = Some(content[2]),
                FCGI_PARAMS if !content.is_empty() => parameters.extend(content),
                FCGI_STDIN if !content.is_empty() => body.extend(content),
                FCGI_STDIN => break,
                _ => {}
            }
        }
        assert_eq!(begin_flags, Some(1));
        let mut position = 0;
        let mut values = HashMap::new();
        while position < parameters.len() {
            let name_length = read_fastcgi_test_length(&parameters, &mut position);
            let value_length = read_fastcgi_test_length(&parameters, &mut position);
            let name =
                String::from_utf8(parameters[position..position + name_length].to_vec()).unwrap();
            position += name_length;
            let value =
                String::from_utf8(parameters[position..position + value_length].to_vec()).unwrap();
            position += value_length;
            values.insert(name, value);
        }
        assert_eq!(
            values.get("REQUEST_METHOD").map(String::as_str),
            Some("POST")
        );
        assert_eq!(values.get("QUERY_STRING").map(String::as_str), Some("x=1"));
        assert_eq!(
            values.get("SCRIPT_FILENAME").map(String::as_str),
            Some("/srv/php/demo.php")
        );
        assert_eq!(
            values.get("PATH_INFO").map(String::as_str),
            Some("/extra/path")
        );
        assert_eq!(
            values.get("CONTENT_TYPE").map(String::as_str),
            Some("text/plain")
        );
        assert_eq!(
            values.get("HTTP_HOST").map(String::as_str),
            Some("localhost")
        );
        assert_eq!(
            values.get("HTTP_X_CUSTOM").map(String::as_str),
            Some("forwarded, again")
        );
        assert_eq!(body, b"hello");
        write_fastcgi_record(
            &mut fastcgi,
            FCGI_STDOUT,
            b"Status: 201 Created\r\nContent-Type: text/plain\r\nX-FastCGI: yes\r\n\r\nfastcgi-ok",
        )
        .await
        .unwrap();
        write_fastcgi_record(&mut fastcgi, FCGI_END_REQUEST, &[0; 8])
            .await
            .unwrap();
        fastcgi.flush().await.unwrap();
    });
    let response = fastcgi_exchange(
        client,
        &request,
        &env,
        Some(&server.locations[0]),
        context,
        &variables,
    )
    .await
    .unwrap();
    assert_eq!(response.status, 201);
    assert_eq!(response.header("X-FastCGI"), Some("yes"));
    assert_eq!(response.header("Content-Type"), Some("text/plain"));
    assert_eq!(response.into_bytes(), b"fastcgi-ok");
    server_task.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}
