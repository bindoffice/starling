use super::*;

#[tokio::test]
async fn appends_uwsgi_string_after_parameters_and_before_body() {
    let (prefix, config) = test_config(
        r#"
                worker_processes 1;
                events { worker_connections 64; }
                http {
                    server {
                        listen 127.0.0.1:18521;
                        location /u/ {
                            uwsgi_pass 127.0.0.1:9001;
                            uwsgi_param REQUEST_METHOD $request_method;
                            uwsgi_string "literal=$host";
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
        target: "/u/app".into(),
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
        local_addr: "127.0.0.1:18521".into(),
        keep_alive: false,
    };
    let remote: SocketAddr = "127.0.0.1:45678".parse().unwrap();
    let local: SocketAddr = "127.0.0.1:18521".parse().unwrap();
    let mut variables = Variables::new(&request, "localhost", server, &local, &remote);
    let env = RequestEnv {
        uri: "/u/app".into(),
        args: None,
        unparsed_uri: Some("/u/app".into()),
        valid_location: true,
        internal: false,
        bypass_try_files: false,
        add_uri_to_alias: false,
        limits_checked: false,
        auth_request_checked: false,
        mirror_checked: false,
    };
    refresh_variables(&env, Some(&server.locations[0]), &mut variables, context);

    let (client, mut backend) = tokio::io::duplex(64 * 1024);
    let server_task = tokio::spawn(async move {
        let mut header = [0_u8; 4];
        backend.read_exact(&mut header).await.unwrap();
        let parameter_length = u16::from_le_bytes([header[1], header[2]]) as usize;
        let mut parameters = vec![0_u8; parameter_length];
        backend.read_exact(&mut parameters).await.unwrap();
        assert!(parameters.ends_with(b"literal=$host"));

        let mut body = [0_u8; 5];
        backend.read_exact(&mut body).await.unwrap();
        assert_eq!(&body, b"hello");
        backend
            .write_all(b"Status: 200 OK\r\nContent-Type: text/plain\r\n\r\nuwsgi-string-ok")
            .await
            .unwrap();
        backend.shutdown().await.unwrap();
    });

    let response = uwsgi_exchange(client, &request, context, &variables)
        .await
        .unwrap();
    assert_eq!(response.into_bytes(), b"uwsgi-string-ok");
    server_task.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}
