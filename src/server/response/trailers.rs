use super::*;

pub(in crate::server) fn apply_response_trailers(
    response: &mut Response,
    context: &EffectiveConfig,
    request: &Request,
    variables: &mut Variables,
) {
    if context.add_trailers.is_empty()
        || request.method.eq_ignore_ascii_case("HEAD")
        || matches!(response.status, 204 | 304)
    {
        return;
    }
    response.expect_chunked();
    variables.status = response.status;
    variables.sent_trailers.clone_from(&response.trailers);
    let always_allowed = trailer_status_allowed(response.status);
    for trailer in &context.add_trailers {
        if !always_allowed && !trailer.always {
            continue;
        }
        let value = eval_variables(&trailer.value, variables);
        if !value.is_empty() {
            response.trailers.push(Header {
                name: trailer.name.clone(),
                value,
            });
            variables.sent_trailers.clone_from(&response.trailers);
        }
    }
}

fn trailer_status_allowed(status: u16) -> bool {
    matches!(status, 200 | 201 | 206 | 301 | 302 | 303 | 307 | 308)
}
