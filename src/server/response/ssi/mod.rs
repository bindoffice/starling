use super::*;

mod eval;
mod include;
mod parser;
mod state;
mod text;

use eval::Runtime;
use state::State;

pub(in crate::server) fn default_date(gmt: bool) -> String {
    text::format_timestamp(state::DEFAULT_TIME_FORMAT, gmt)
}

pub(in crate::server) async fn apply_ssi(
    http: &HttpConfig,
    server: &ServerConfig,
    request: &Request,
    response: &mut Response,
    context: &EffectiveConfig,
    variables: &mut Variables,
    depth: usize,
) {
    if !ssi_applies(context, request, response) {
        return;
    }
    let body = match std::mem::replace(&mut response.body, ResponseBody::Empty)
        .into_memory(MAX_BODY_BYTES as u64)
        .await
    {
        Ok(body) => body,
        Err(body) => {
            response.body = *body;
            return;
        }
    };
    let segments = parser::parse(&body, context.ssi.value_length);
    let mut state = State::new();
    let mut runtime = Runtime {
        http,
        server,
        request,
        context,
        variables,
        depth,
    };
    let rendered = match eval::render(&segments, &mut runtime, &mut state).await {
        Ok(rendered) => rendered,
        Err(error) if context.ssi.silent_errors => {
            let _ = error;
            body
        }
        Err(_) => body,
    };
    response.body = ResponseBody::Bytes(rendered);
    response.remove_header("Content-Length");
    response.remove_header("Accept-Ranges");
    if context.ssi.last_modified {
        weaken_etag(response);
    } else {
        response.remove_header("Last-Modified");
        response.remove_header("ETag");
    }
    response.set_content_length();
}

pub(super) fn ssi_applies(
    context: &EffectiveConfig,
    request: &Request,
    response: &Response,
) -> bool {
    if !context.ssi.enabled
        || request.method.eq_ignore_ascii_case("HEAD")
        || response.header("Content-Encoding").is_some()
        || response.body_len() == 0
    {
        return false;
    }
    let content_type = response
        .header("Content-Type")
        .unwrap_or_default()
        .split(';')
        .next()
        .unwrap_or_default()
        .trim();
    context
        .ssi
        .types
        .iter()
        .any(|allowed| allowed == "*" || allowed.eq_ignore_ascii_case(content_type))
}
