use super::*;

pub(super) async fn finalize_proxy_response(
    mut upstream_response: UpstreamResponse,
    proxy: &ProxyPassConfig,
    env: &RequestEnv,
    location: Option<&LocationConfig>,
    context: &EffectiveConfig,
    upstream_variables: &Variables,
) -> Response {
    if upstream_response.body.is_stream() {
        store_proxy_response_streaming(
            &mut upstream_response,
            env,
            location,
            context,
            upstream_variables,
        );
    } else {
        let _ = store_proxy_response(
            &upstream_response,
            env,
            location,
            context,
            upstream_variables,
        )
        .await;
    }
    let early_hints = if should_send_early_hints(&context.early_hints, upstream_variables) {
        std::mem::take(&mut upstream_response.early_hints)
            .into_iter()
            .map(|headers| filter_proxy_response_headers(headers, context))
            .filter(|headers| !headers.is_empty())
            .collect()
    } else {
        Vec::new()
    };
    let body = std::mem::replace(&mut upstream_response.body, ResponseBody::Empty);
    let body_temp_file = upstream_response.body_temp_file.take();
    let mut response = Response {
        status: upstream_response.status,
        headers: upstream_response.headers,
        trailers: upstream_response.trailers,
        chunked_expected: false,
        body,
        body_temp_file,
        upstream: None,
        builtin_error: false,
        msie_padding: false,
        allow_ranges: false,
        single_range: false,
    };
    response.set_early_hints(early_hints);
    capture_upstream_control(&mut response, &context.proxy_ignore_headers);
    response.headers =
        filter_proxy_response_headers(std::mem::take(&mut response.headers), context);
    if context.proxy_pass_trailers {
        response.trailers =
            filter_proxy_response_headers(std::mem::take(&mut response.trailers), context);
    } else {
        response.trailers.clear();
    }
    if let ResponseBody::Stream(stream) = &mut response.body {
        let pass_trailers = context.proxy_pass_trailers;
        let pass_headers = context.proxy_pass_headers.clone();
        let hide_headers = context.proxy_hide_headers.clone();
        stream.set_trailer_filter(move |trailers| {
            if pass_trailers {
                filter_proxy_trailer_headers(trailers, &pass_headers, &hide_headers)
            } else {
                Vec::new()
            }
        });
    }
    apply_proxy_redirect(&mut response, proxy, location, context, upstream_variables);
    apply_proxy_cookie_rewrites(&mut response, context, upstream_variables);
    if !upstream_variables.method.eq_ignore_ascii_case("HEAD")
        || response.header("Content-Length").is_none()
    {
        response.set_content_length();
    }
    response
}
