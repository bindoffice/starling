use super::*;

#[test]
fn single_primary_upstream_keeps_trying_after_failures() {
    let (prefix, config) = test_config(
        &upstream_config("server 127.0.0.1:19151 max_fails=1 fail_timeout=10s;"),
        &[],
    );
    let upstream = &config.http.as_ref().unwrap().upstreams["backend"];
    let mut failed = upstream.servers[0].try_acquire().unwrap();
    failed.mark_failed();
    drop(failed);
    assert_eq!(
        upstream.select_with(None, None).unwrap().address,
        "127.0.0.1:19151"
    );
    let _ = std::fs::remove_dir_all(prefix);
}

#[test]
fn parses_next_upstream_retry_controls() {
    let (prefix, config) = test_config(
        &location_config(
            "proxy_next_upstream http_500 non_idempotent; \
                 proxy_next_upstream_tries 2; \
                 proxy_next_upstream_timeout 3s; \
                 fastcgi_next_upstream error invalid_header; \
                 uwsgi_next_upstream timeout http_503; \
                 scgi_next_upstream off;",
        ),
        &[],
    );
    let context = &config.http.as_ref().unwrap().servers[0].context;
    assert!(!context.proxy_next_upstream.retry_error());
    assert!(!context.proxy_next_upstream.retry_timeout());
    assert!(context.proxy_next_upstream.retry_non_idempotent());
    assert!(context.proxy_next_upstream.retry_status(500));
    assert_eq!(context.proxy_next_upstream.tries, 2);
    assert_eq!(context.proxy_next_upstream.timeout, Duration::from_secs(3));
    assert!(context.fastcgi_next_upstream.retry_error());
    assert!(context.fastcgi_next_upstream.retry_invalid_header());
    assert!(context.uwsgi_next_upstream.retry_timeout());
    assert!(context.uwsgi_next_upstream.retry_status(503));
    assert!(!context.scgi_next_upstream.retry_error());
    assert!(!context.scgi_next_upstream.retry_timeout());
    assert!(!context.scgi_next_upstream.retry_status(500));
    let _ = std::fs::remove_dir_all(prefix);
}

#[test]
fn next_upstream_state_enforces_defaults_methods_and_tries() {
    let mut request = Request {
        method: "GET".into(),
        target: "/".into(),
        version: "HTTP/1.1".into(),
        headers: (Vec::new()).into(),
        body_stream: None,
        body: Vec::new(),
        remote_addr: "127.0.0.1:1234".into(),
        local_addr: "127.0.0.1:80".into(),
        keep_alive: true,
    };
    let defaults = NextUpstreamConfig::default();
    let state = NextUpstreamState::new(&defaults);
    assert!(state.can_retry_failure(&request, &ProxyFailure::Failure));
    assert!(state.can_retry_failure(&request, &ProxyFailure::Timeout));
    assert!(!state.can_retry_failure(&request, &ProxyFailure::InvalidHeader));
    assert!(!state.can_retry_status(&request, 500));

    request.method = "POST".into();
    assert!(!state.can_retry_failure(&request, &ProxyFailure::Failure));

    let (prefix, config) = test_config(
        &location_config("proxy_next_upstream error non_idempotent; proxy_next_upstream_tries 2;"),
        &[],
    );
    let retry = &config.http.as_ref().unwrap().servers[0]
        .context
        .proxy_next_upstream;
    let mut state = NextUpstreamState::new(retry);
    assert!(state.can_retry_failure(&request, &ProxyFailure::Failure));
    assert!(state.start_attempt());
    assert!(state.can_retry_failure(&request, &ProxyFailure::Failure));
    assert!(state.start_attempt());
    assert!(!state.can_retry_failure(&request, &ProxyFailure::Failure));
    assert!(!state.start_attempt());
    let _ = std::fs::remove_dir_all(prefix);
}

#[test]
fn rejects_invalid_next_upstream_controls() {
    assert!(
        test_config_error(&location_config("proxy_next_upstream bogus;"))
            .contains("invalid parameter \"bogus\"")
    );
    assert!(
        test_config_error(&location_config("proxy_next_upstream;"))
            .contains("invalid number of arguments")
    );
    assert!(
        test_config_error(&location_config("proxy_next_upstream_tries nope;"))
            .contains("invalid value \"nope\"")
    );
    assert!(
        test_config_error(&location_config("proxy_next_upstream_timeout nope;"))
            .contains("invalid value \"nope\"")
    );
}
