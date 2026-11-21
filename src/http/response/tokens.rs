use super::*;
use crate::SERVER_SOFTWARE_BUILD;

const MSIE_PADDING: &[u8] =
    b"<!-- a padding to disable MSIE and Chrome friendly error page -->\r\n\
      <!-- a padding to disable MSIE and Chrome friendly error page -->\r\n\
      <!-- a padding to disable MSIE and Chrome friendly error page -->\r\n\
      <!-- a padding to disable MSIE and Chrome friendly error page -->\r\n\
      <!-- a padding to disable MSIE and Chrome friendly error page -->\r\n\
      <!-- a padding to disable MSIE and Chrome friendly error page -->\r\n";

pub(crate) fn prepare_response_headers(response: &mut Response, tokens: ServerTokens) {
    if response.builtin_error
        && let ResponseBody::Bytes(body) = &mut response.body
    {
        *body = error_page_body(response.status, tokens).into_bytes();
        if response.msie_padding && response.status >= 400 {
            body.extend_from_slice(MSIE_PADDING);
        }
        response.set_content_length();
    }
    if response.header("Date").is_none() {
        response.set_header(
            "Date",
            httpdate::fmt_http_date(std::time::SystemTime::now()),
        );
    }
    if response.header("Server").is_none() {
        response.set_header("Server", server_software(tokens));
    }
    if response.header("Content-Length").is_none() && !matches!(response.body, ResponseBody::Empty)
    {
        response.set_content_length();
    }
}

pub(crate) fn error_page_body(status: u16, tokens: ServerTokens) -> String {
    let reason = reason_phrase(status);
    format!(
        "<html>\r\n<head><title>{status} {reason}</title></head>\r\n\
         <body>\r\n<center><h1>{status} {reason}</h1></center>\r\n\
         <hr><center>{}</center>\r\n</body>\r\n</html>\r\n",
        server_software(tokens)
    )
}

fn server_software(tokens: ServerTokens) -> &'static str {
    match tokens {
        ServerTokens::Off => "nginx",
        ServerTokens::On => SERVER_SOFTWARE,
        ServerTokens::Build => SERVER_SOFTWARE_BUILD,
    }
}
