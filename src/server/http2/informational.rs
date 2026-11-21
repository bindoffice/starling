use super::*;

pub(in crate::server) fn send_h2_early_hints(
    respond: &mut SendResponse<Bytes>,
    early_hints: &[Vec<Header>],
) -> io::Result<()> {
    for headers in early_hints {
        let mut fields = http::HeaderMap::new();
        for header in headers {
            if is_h2_connection_header(&header.name) {
                continue;
            }
            let Ok(name) = http::header::HeaderName::from_bytes(header.name.as_bytes()) else {
                continue;
            };
            let value = sanitize_header_value(&header.value);
            let Ok(value) = http::header::HeaderValue::from_str(&value) else {
                continue;
            };
            fields.append(name, value);
        }
        if fields.is_empty() {
            continue;
        }
        let mut response = HttpResponse::builder().status(StatusCode::EARLY_HINTS);
        for (name, value) in &fields {
            response = response.header(name, value);
        }
        let response = response.body(()).map_err(io::Error::other)?;
        respond
            .send_informational(response)
            .map_err(io::Error::other)?;
    }
    Ok(())
}
