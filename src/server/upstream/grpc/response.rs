use super::*;

pub(in crate::server) const GRPC_DEFAULT_HIDE_HEADERS: &[&str] = &[
    "date",
    "server",
    "x-accel-expires",
    "x-accel-redirect",
    "x-accel-limit-rate",
    "x-accel-buffering",
    "x-accel-charset",
];

pub(in crate::server) fn finalize_grpc_response(
    mut upstream: UpstreamResponse,
    context: &EffectiveConfig,
) -> Response {
    let body = std::mem::replace(&mut upstream.body, ResponseBody::Empty);
    let mut response = Response {
        status: upstream.status,
        headers: upstream.headers,
        trailers: upstream.trailers,
        body,
        body_temp_file: upstream.body_temp_file.take(),
        upstream: None,
        builtin_error: false,
        msie_padding: false,
    };
    filter_gateway_response_headers(
        &mut response,
        GRPC_DEFAULT_HIDE_HEADERS,
        &context.grpc.hide_headers,
        &context.grpc.pass_headers,
        &context.grpc.ignore_headers,
    );
    response.trailers = filter_grpc_headers(
        std::mem::take(&mut response.trailers),
        &context.grpc.hide_headers,
        &context.grpc.pass_headers,
    );
    if let ResponseBody::Stream(stream) = &mut response.body {
        let hide_headers = context.grpc.hide_headers.clone();
        let pass_headers = context.grpc.pass_headers.clone();
        stream.set_trailer_filter(move |trailers| {
            filter_grpc_headers(trailers, &hide_headers, &pass_headers)
        });
    }
    response.set_content_length();
    response
}

fn filter_grpc_headers(headers: Vec<Header>, hide: &[String], pass: &[String]) -> Vec<Header> {
    headers
        .into_iter()
        .filter(|header| {
            pass.iter()
                .any(|name| name.eq_ignore_ascii_case(&header.name))
                || (!GRPC_DEFAULT_HIDE_HEADERS
                    .iter()
                    .any(|name| name.eq_ignore_ascii_case(&header.name))
                    && !hide
                        .iter()
                        .any(|name| name.eq_ignore_ascii_case(&header.name)))
        })
        .collect()
}
