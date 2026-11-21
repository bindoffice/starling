use super::*;

pub(in crate::server) fn weaken_etag(response: &mut Response) {
    let Some(etag) = response.header("ETag").map(str::to_string) else {
        return;
    };
    if !etag.trim_start().starts_with("W/") {
        response.set_header("ETag", format!("W/{etag}"));
    }
}
