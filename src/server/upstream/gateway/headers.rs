use super::*;

pub(in crate::server) const FASTCGI_DEFAULT_HIDE_HEADERS: &[&str] = &[
    "status",
    "x-accel-expires",
    "x-accel-redirect",
    "x-accel-limit-rate",
    "x-accel-buffering",
    "x-accel-charset",
];

pub(in crate::server) const UWSGI_DEFAULT_HIDE_HEADERS: &[&str] = &[
    "x-accel-expires",
    "x-accel-redirect",
    "x-accel-limit-rate",
    "x-accel-buffering",
    "x-accel-charset",
];

pub(in crate::server) const SCGI_DEFAULT_HIDE_HEADERS: &[&str] = &[
    "status",
    "x-accel-expires",
    "x-accel-redirect",
    "x-accel-limit-rate",
    "x-accel-buffering",
    "x-accel-charset",
];

pub(in crate::server) fn filter_gateway_response_headers(
    response: &mut Response,
    default_hide: &[&str],
    hide: &[String],
    pass: &[String],
    ignore: &[String],
) {
    capture_upstream_control(response, ignore);
    response.headers.retain(|header| {
        if pass
            .iter()
            .any(|name| name.eq_ignore_ascii_case(&header.name))
        {
            return true;
        }
        let name = header.name.to_ascii_lowercase();
        !default_hide.contains(&name.as_str())
            && !hide
                .iter()
                .any(|hidden| hidden.eq_ignore_ascii_case(&header.name))
    });
}
