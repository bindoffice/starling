use super::*;

#[test]
fn merges_forwarded_cgi_headers_without_overriding_explicit_params() {
    let headers = vec![
        Header {
            name: "X-Custom".into(),
            value: "first".into(),
        },
        Header {
            name: "X-Custom".into(),
            value: "second".into(),
        },
        Header {
            name: "Cookie".into(),
            value: "a=1".into(),
        },
        Header {
            name: "Cookie".into(),
            value: "b=2".into(),
        },
    ];
    let mut params = Vec::new();
    append_cgi_request_headers(&mut params, &headers);
    assert_eq!(
        params,
        vec![
            ("HTTP_X_CUSTOM".into(), "first, second".into()),
            ("HTTP_COOKIE".into(), "a=1; b=2".into())
        ]
    );

    let mut params = vec![("HTTP_X_CUSTOM".into(), "configured".into())];
    append_cgi_request_headers(&mut params, &headers);
    assert_eq!(
        params,
        vec![
            ("HTTP_X_CUSTOM".into(), "configured".into()),
            ("HTTP_COOKIE".into(), "a=1; b=2".into())
        ]
    );
}
