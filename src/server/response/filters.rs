use super::*;

pub(in crate::server) async fn maybe_gzip(
    response: &mut Response,
    context: &EffectiveConfig,
    request: &Request,
) {
    if !context.gzip
        || response.status == 206
        || response.status == 304
        || response.header("Content-Encoding").is_some()
        || !request.header("accept-encoding").is_some_and(accepts_gzip)
        || !super::gzip_rules::request_allows_gzip(context, request)
        || request.header("via").is_some()
            && !gzip_proxied_allows(&context.gzip_proxied, request, response)
    {
        return;
    }
    let content_type = response
        .header("Content-Type")
        .unwrap_or_default()
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    let compressible = content_type == "text/html"
        || context
            .gzip_types
            .iter()
            .any(|allowed| allowed == "*" || allowed == &content_type);
    let body_length = response
        .header("Content-Length")
        .and_then(|value| value.parse().ok())
        .or_else(|| response.body.content_length());
    if !compressible || body_length.is_none_or(|length| length < context.gzip_min_length) {
        return;
    }
    if request.method.eq_ignore_ascii_case("HEAD") {
        response.disable_ranges();
        response.remove_header("Content-Length");
        response.remove_header("Accept-Ranges");
        response.set_header("Content-Encoding", "gzip");
        if context.gzip_vary {
            append_gzip_vary(response);
        }
        return;
    }
    let body = match std::mem::replace(&mut response.body, ResponseBody::Empty)
        .into_memory(16 * 1024 * 1024)
        .await
    {
        Ok(body) => body,
        Err(body) => {
            response.body = *body;
            return;
        }
    };
    use flate2::Compression;
    use flate2::write::GzEncoder;
    use std::io::Write;
    let mut encoder = GzEncoder::new(Vec::new(), Compression::new(context.gzip_comp_level));
    if encoder.write_all(&body).is_err() {
        response.body = ResponseBody::Bytes(body);
        return;
    }
    let Ok(compressed) = encoder.finish() else {
        response.body = ResponseBody::Bytes(body);
        return;
    };
    response.body = ResponseBody::Bytes(compressed);
    response.disable_ranges();
    response.remove_header("Content-Length");
    response.remove_header("Accept-Ranges");
    response.expect_chunked();
    response.set_header("Content-Encoding", "gzip");
    if context.gzip_vary {
        append_gzip_vary(response);
    }
}

fn append_gzip_vary(response: &mut Response) {
    let Some(vary) = response.header("Vary").map(str::to_string) else {
        response.set_header("Vary", "Accept-Encoding");
        return;
    };
    if vary
        .split(',')
        .any(|value| value.trim().eq_ignore_ascii_case("accept-encoding"))
    {
        return;
    }
    response.set_header("Vary", format!("{vary}, Accept-Encoding"));
}

pub(in crate::server) fn gzip_proxied_allows(
    config: &GzipProxiedConfig,
    request: &Request,
    response: &Response,
) -> bool {
    if config.any {
        return true;
    }
    if config.auth && request.header("authorization").is_some() {
        return true;
    }
    if config.expired
        && response
            .header("Expires")
            .and_then(|value| httpdate::parse_http_date(value).ok())
            .is_some_and(|expires| expires <= SystemTime::now())
    {
        return true;
    }
    let cache_control = response.header("Cache-Control").unwrap_or_default();
    let has_cache_token = |token: &str| {
        cache_control
            .split(',')
            .any(|value| value.trim().eq_ignore_ascii_case(token))
    };
    (config.no_cache && has_cache_token("no-cache"))
        || (config.no_store && has_cache_token("no-store"))
        || (config.private && has_cache_token("private"))
        || (config.no_last_modified && response.header("Last-Modified").is_none())
        || (config.no_etag && response.header("ETag").is_none())
}

pub(in crate::server) fn accepts_gzip(value: &str) -> bool {
    value.split(',').any(|encoding| {
        let mut parts = encoding.trim().split(';');
        parts
            .next()
            .is_some_and(|name| name.trim().eq_ignore_ascii_case("gzip"))
            && !parts.any(|parameter| {
                parameter
                    .trim()
                    .strip_prefix("q=")
                    .and_then(|quality| quality.parse::<f32>().ok())
                    .is_some_and(|quality| quality <= 0.0)
            })
    })
}
