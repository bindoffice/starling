use super::*;

#[test]
fn applies_gateway_header_defaults_hide_and_pass_rules() {
    let headers = || {
        [
            ("X-Accel-Redirect", "/internal"),
            ("X-Accel-Expires", "60"),
            ("X-Custom-Hide", "hidden"),
            ("X-Custom-Pass", "visible"),
            ("X-Keep", "keep"),
        ]
        .into_iter()
        .map(|(name, value)| Header {
            name: name.into(),
            value: value.into(),
        })
        .collect::<Vec<_>>()
    };
    let hide = vec!["X-Custom-Hide".to_string(), "X-Custom-Pass".to_string()];
    let pass = vec!["X-Custom-Pass".to_string(), "X-Accel-Expires".to_string()];

    let mut response = Response {
        status: 200,
        headers: headers(),
        trailers: Vec::new(),
        body: ResponseBody::Empty,
        body_temp_file: None,
        upstream: None,
        builtin_error: false,
        msie_padding: false,
    };
    filter_gateway_response_headers(
        &mut response,
        FASTCGI_DEFAULT_HIDE_HEADERS,
        &hide,
        &pass,
        &[],
    );
    assert_eq!(
        header_names(&response),
        vec!["X-Accel-Expires", "X-Custom-Pass", "X-Keep"]
    );

    let mut response = Response {
        status: 200,
        headers: headers(),
        trailers: Vec::new(),
        body: ResponseBody::Empty,
        body_temp_file: None,
        upstream: None,
        builtin_error: false,
        msie_padding: false,
    };
    filter_gateway_response_headers(&mut response, UWSGI_DEFAULT_HIDE_HEADERS, &[], &[], &[]);
    assert_eq!(
        header_names(&response),
        vec!["X-Custom-Hide", "X-Custom-Pass", "X-Keep"]
    );

    let mut response = Response {
        status: 200,
        headers: headers(),
        trailers: Vec::new(),
        body: ResponseBody::Empty,
        body_temp_file: None,
        upstream: None,
        builtin_error: false,
        msie_padding: false,
    };
    filter_gateway_response_headers(
        &mut response,
        FASTCGI_DEFAULT_HIDE_HEADERS,
        &[],
        &["X-Accel-Redirect".to_string()],
        &["X-Accel-Redirect".to_string()],
    );
    assert_eq!(response.header("X-Accel-Redirect"), Some("/internal"));
    assert!(
        response
            .upstream_control()
            .is_some_and(|control| control.redirect.is_none())
    );
}

#[tokio::test]
async fn filters_fastcgi_response_headers_in_exchange() {
    let (prefix, config) = test_config(
        r#"
            events { worker_connections 64; }
            http {
                server {
                    listen 127.0.0.1:18141;
                    location / {
                        fastcgi_pass 127.0.0.1:9000;
                        fastcgi_hide_header X-Custom-Hide;
                        fastcgi_hide_header X-Custom-Pass;
                        fastcgi_pass_header X-Custom-Pass;
                        fastcgi_pass_header X-Accel-Expires;
                    }
                }
            }
        "#,
        &[],
    );
    let server = &config.http.as_ref().unwrap().servers[0];
    let context = &server.locations[0].context;
    let request = Request {
        method: "GET".into(),
        target: "/".into(),
        version: "HTTP/1.1".into(),
        headers: vec![Header {
            name: "Host".into(),
            value: "localhost".into(),
        }],
        body_stream: None,
        body: Vec::new(),
        remote_addr: "127.0.0.1:45678".into(),
        local_addr: "127.0.0.1:18141".into(),
        keep_alive: false,
    };
    let remote: SocketAddr = "127.0.0.1:45678".parse().unwrap();
    let local: SocketAddr = "127.0.0.1:18141".parse().unwrap();
    let variables = Variables::new(&request, "localhost", server, &local, &remote);
    let env = RequestEnv {
        uri: "/".into(),
        args: None,
        unparsed_uri: Some("/".into()),
        internal: false,
        bypass_try_files: false,
        limits_checked: false,
        auth_request_checked: false,
        mirror_checked: false,
    };
    let (client, mut fastcgi) = tokio::io::duplex(64 * 1024);
    let backend = tokio::spawn(async move {
        loop {
            let mut header = [0_u8; 8];
            fastcgi.read_exact(&mut header).await.unwrap();
            let length = u16::from_be_bytes([header[4], header[5]]) as usize;
            let padding = header[6] as usize;
            let mut content = vec![0_u8; length];
            fastcgi.read_exact(&mut content).await.unwrap();
            if padding > 0 {
                let mut padding = vec![0_u8; padding];
                fastcgi.read_exact(&mut padding).await.unwrap();
            }
            if header[1] == FCGI_STDIN && content.is_empty() {
                break;
            }
        }
        write_fastcgi_record(
            &mut fastcgi,
            FCGI_STDOUT,
            b"Status: 200 OK\r\n\
              Content-Type: text/plain\r\n\
              X-Accel-Redirect: /internal\r\n\
              X-Accel-Expires: 60\r\n\
              X-Custom-Hide: hidden\r\n\
              X-Custom-Pass: visible\r\n\
              X-Keep: keep\r\n\r\nbody",
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
    assert_eq!(
        header_names(&response),
        vec![
            "Content-Type",
            "X-Accel-Expires",
            "X-Custom-Pass",
            "X-Keep",
            "Content-Length"
        ]
    );
    assert_eq!(response.into_bytes(), b"body");
    backend.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

fn header_names(response: &Response) -> Vec<&str> {
    response
        .headers
        .iter()
        .map(|header| header.name.as_str())
        .collect()
}
