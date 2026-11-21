use super::*;
use flate2::read::MultiGzDecoder;
use std::io::Read;

pub(in crate::server) async fn maybe_gunzip(
    response: &mut Response,
    context: &EffectiveConfig,
    request: &Request,
) {
    if !context.gunzip
        || !response
            .header("Content-Encoding")
            .is_some_and(|encoding| encoding.eq_ignore_ascii_case("gzip"))
        || response.body_len() == 0
    {
        return;
    }
    if context.gzip_vary {
        add_vary_accept_encoding(response);
    }
    if request.header("accept-encoding").is_some_and(accepts_gzip) {
        return;
    }

    let compressed = match std::mem::replace(&mut response.body, ResponseBody::Empty)
        .into_memory(MAX_BODY_BYTES as u64)
        .await
    {
        Ok(body) => body,
        Err(_) => {
            *response = error_response(500);
            return;
        }
    };
    let mut decoded = Vec::new();
    if MultiGzDecoder::new(compressed.as_slice())
        .read_to_end(&mut decoded)
        .is_err()
    {
        *response = error_response(500);
        return;
    }

    response.remove_header("Content-Encoding");
    response.remove_header("Accept-Ranges");
    weaken_etag(response);
    response.body = ResponseBody::Bytes(decoded);
    response.set_content_length();
}
