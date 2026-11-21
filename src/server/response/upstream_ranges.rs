use super::*;

pub(in crate::server) fn apply_upstream_ranges(
    response: &mut Response,
    request: &Request,
    force_ranges: bool,
    upstream: bool,
    max_ranges: u64,
) -> Option<Response> {
    if !upstream || !force_ranges || response.status != 200 {
        return None;
    }
    let length = response.body_len();
    apply_range(response, request, length, max_ranges)
}
