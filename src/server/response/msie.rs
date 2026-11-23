use super::*;

const MSIE_REFRESH_HEAD: &str = "<html><head><meta http-equiv=\"Refresh\" content=\"0; URL=";
const MSIE_REFRESH_TAIL: &str = "\"></head><body></body></html>\r\n";

pub(in crate::server) fn apply_msie_compat(
    context: &EffectiveConfig,
    request: &Request,
    response: &mut Response,
) {
    let user_agent = request.header("user-agent").unwrap_or_default();
    if context.msie_refresh
        && super::super::user_agent::is_msie(user_agent)
        && matches!(response.status, 301 | 302)
        && let Some(location) = response.header("Location").map(str::to_string)
    {
        apply_msie_refresh(response, &location);
    }

    response.msie_padding = context.msie_padding
        && response.builtin_error
        && response.status >= 400
        && matches!(
            request.version.as_str(),
            "HTTP/1.0" | "HTTP/1.1" | "HTTP/2.0"
        )
        && (super::super::user_agent::is_msie(user_agent)
            || super::super::user_agent::is_chrome(user_agent));
}

fn apply_msie_refresh(response: &mut Response, location: &str) {
    let body = format!(
        "{MSIE_REFRESH_HEAD}{}{MSIE_REFRESH_TAIL}",
        escape_refresh_uri(location)
    );
    response.status = 200;
    response.body = ResponseBody::Bytes(body.into_bytes());
    response.builtin_error = false;
    response.msie_padding = false;
    response.remove_header("Location");
    response.remove_header("Accept-Ranges");
    response.disable_ranges();
    response.remove_header("Last-Modified");
    response.remove_header("ETag");
    response.set_header("Content-Type", "text/html");
    response.set_content_length();
}

fn escape_refresh_uri(value: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut escaped = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte <= b' '
            || byte >= 0x7f
            || matches!(
                byte,
                b'"' | b'\'' | b'<' | b'>' | b'@' | b'[' | b'\\' | b']' | b'^' | b'_' | b'`'
            )
        {
            escaped.push('%');
            escaped.push(HEX[(byte >> 4) as usize] as char);
            escaped.push(HEX[(byte & 0x0f) as usize] as char);
        } else {
            escaped.push(byte as char);
        }
    }
    escaped
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_refresh_uri_like_nginx() {
        assert_eq!(
            escape_refresh_uri("/a b?x=\"<>'@[\\]^_`&y=%"),
            "/a%20b?x=%22%3C%3E%27%40%5B%5C%5D%5E%5F%60&y=%"
        );
        assert_eq!(escape_refresh_uri("/caf\u{e9}"), "/caf%C3%A9");
    }
}
