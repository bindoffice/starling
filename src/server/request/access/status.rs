use super::*;

pub(in crate::server) fn should_intercept_error_pages(
    proxied: bool,
    context: &EffectiveConfig,
) -> bool {
    !proxied || context.proxy_intercept_errors
}

pub(in crate::server) fn stub_status_response(
    request: &Request,
    variables: &Variables,
) -> Response {
    if !request.method.eq_ignore_ascii_case("GET") && !request.method.eq_ignore_ascii_case("HEAD") {
        return error_response(405);
    }
    let body = format!(
        "Active connections: {} \nserver accepts handled requests\n {} {} {} \nReading: {} Writing: {} Waiting: {} \n",
        variables.stats.active.load(Ordering::Relaxed),
        variables.stats.accepted.load(Ordering::Relaxed),
        variables.stats.handled.load(Ordering::Relaxed),
        variables.stats.requests.load(Ordering::Relaxed),
        variables.stats.reading.load(Ordering::Relaxed),
        variables.stats.writing.load(Ordering::Relaxed),
        variables.stats.waiting.load(Ordering::Relaxed),
    );
    Response::bytes(200, "text/plain", body)
}
