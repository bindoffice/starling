use super::*;

pub(in crate::server) fn enable_upstream_ranges(
    response: &mut Response,
    force_ranges: bool,
    upstream: bool,
    cached: bool,
) {
    if !upstream || response.status != 200 {
        return;
    }
    if force_ranges {
        response.enable_ranges(!cached);
    } else if cached && response.header("Accept-Ranges").is_some() {
        response.enable_ranges(false);
    }
}
