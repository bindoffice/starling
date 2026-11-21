use super::*;

#[tokio::test]
async fn exchanges_uwsgi_packets_and_maps_cgi_headers() {
    let (prefix, config) = test_config(
        r#"
                worker_processes 1;
                events { worker_connections 64; }
                http {
                    server {
                        listen 127.0.0.1:18123;
                        location /u/ {
                            uwsgi_pass 127.0.0.1:9001;
                            uwsgi_modifier1 9;
                            uwsgi_modifier2 1;
                            uwsgi_param REQUEST_METHOD $request_method;
                            uwsgi_param PATH_INFO $document_uri;
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
        target: "/u/app?x=1".into(),
        version: "HTTP/1.1".into(),
        headers: vec![
            Header {
                name: "Host".into(),
                value: "localhost".into(),
            },
            Header {
                name: "Content-Length".into(),
                value: "5".into(),
            },
        ],
        body_stream: None,
        body: b"hello".to_vec(),
        remote_addr: "127.0.0.1:45678".into(),
        local_addr: "127.0.0.1:18123".into(),
        keep_alive: false,
    };
    let remote: SocketAddr = "127.0.0.1:45678".parse().unwrap();
    let local: SocketAddr = "127.0.0.1:18123".parse().unwrap();
    let mut variables = Variables::new(&request, "localhost", server, &local, &remote);
    let env = RequestEnv {
        uri: "/u/app".into(),
        args: Some("x=1".into()),
        unparsed_uri: Some("/u/app?x=1".into()),
        internal: false,
        bypass_try_files: false,
        limits_checked: false,
        auth_request_checked: false,
        mirror_checked: false,
    };
    refresh_variables(&env, Some(&server.locations[0]), &mut variables, context);
    let (client, mut backend) = tokio::io::duplex(64 * 1024);
    let server_task = tokio::spawn(async move {
        let mut header = [0_u8; 4];
        backend.read_exact(&mut header).await.unwrap();
        assert_eq!(header[0], 9);
        assert_eq!(header[3], 1);
        let parameter_length = u16::from_le_bytes([header[1], header[2]]) as usize;
        let mut parameters = vec![0_u8; parameter_length];
        backend.read_exact(&mut parameters).await.unwrap();
        let values = parse_null_separated_params(&parameters);
        assert_eq!(
            values.get("REQUEST_METHOD").map(String::as_str),
            Some("POST")
        );
        assert_eq!(values.get("PATH_INFO").map(String::as_str), Some("/u/app"));
        assert_eq!(
            values.get("HTTP_HOST").map(String::as_str),
            Some("localhost")
        );
        let mut body = [0_u8; 5];
        backend.read_exact(&mut body).await.unwrap();
        assert_eq!(&body, b"hello");
        backend
            .write_all(b"Status: 202 Accepted\r\nContent-Type: text/plain\r\n\r\nuwsgi-ok")
            .await
            .unwrap();
        backend.shutdown().await.unwrap();
    });
    let response = uwsgi_exchange(client, &request, context, &variables)
        .await
        .unwrap();
    assert_eq!(response.status, 202);
    assert_eq!(response.into_bytes(), b"uwsgi-ok");
    server_task.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn exchanges_scgi_netstrings_and_maps_cgi_headers() {
    let (prefix, config) = test_config(
        r#"
                worker_processes 1;
                events { worker_connections 64; }
                http {
                    server {
                        listen 127.0.0.1:18124;
                        location /s/ {
                            scgi_pass 127.0.0.1:9002;
                            scgi_param REQUEST_METHOD $request_method;
                            scgi_param REQUEST_URI $request_uri;
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
        target: "/s/app?x=1".into(),
        version: "HTTP/1.1".into(),
        headers: vec![
            Header {
                name: "Host".into(),
                value: "localhost".into(),
            },
            Header {
                name: "Content-Length".into(),
                value: "5".into(),
            },
        ],
        body_stream: None,
        body: b"hello".to_vec(),
        remote_addr: "127.0.0.1:45678".into(),
        local_addr: "127.0.0.1:18124".into(),
        keep_alive: false,
    };
    let remote: SocketAddr = "127.0.0.1:45678".parse().unwrap();
    let local: SocketAddr = "127.0.0.1:18124".parse().unwrap();
    let mut variables = Variables::new(&request, "localhost", server, &local, &remote);
    let env = RequestEnv {
        uri: "/s/app".into(),
        args: Some("x=1".into()),
        unparsed_uri: Some("/s/app?x=1".into()),
        internal: false,
        bypass_try_files: false,
        limits_checked: false,
        auth_request_checked: false,
        mirror_checked: false,
    };
    refresh_variables(&env, Some(&server.locations[0]), &mut variables, context);
    let (client, mut backend) = tokio::io::duplex(64 * 1024);
    let server_task = tokio::spawn(async move {
        let mut prefix = Vec::new();
        let mut byte = [0_u8; 1];
        loop {
            backend.read_exact(&mut byte).await.unwrap();
            if byte[0] == b':' {
                break;
            }
            prefix.push(byte[0]);
        }
        let length = std::str::from_utf8(&prefix)
            .unwrap()
            .parse::<usize>()
            .unwrap();
        let mut parameters = vec![0_u8; length];
        backend.read_exact(&mut parameters).await.unwrap();
        assert_eq!(parameters.last(), Some(&b','));
        let values = parse_null_separated_params(&parameters[..parameters.len() - 1]);
        assert_eq!(
            values.get("REQUEST_METHOD").map(String::as_str),
            Some("POST")
        );
        assert_eq!(
            values.get("REQUEST_URI").map(String::as_str),
            Some("/s/app?x=1")
        );
        assert_eq!(values.get("CONTENT_LENGTH").map(String::as_str), Some("5"));
        assert_eq!(
            values.get("HTTP_HOST").map(String::as_str),
            Some("localhost")
        );
        let mut body = [0_u8; 5];
        backend.read_exact(&mut body).await.unwrap();
        assert_eq!(&body, b"hello");
        backend
            .write_all(b"Status: 201 Created\r\nContent-Type: text/plain\r\n\r\nscgi-ok")
            .await
            .unwrap();
        backend.shutdown().await.unwrap();
    });
    let response = scgi_exchange(client, &request, context, &variables)
        .await
        .unwrap();
    assert_eq!(response.status, 201);
    assert_eq!(response.into_bytes(), b"scgi-ok");
    server_task.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}
