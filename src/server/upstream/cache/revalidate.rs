use super::*;

const CACHE_CONDITIONAL_HEADERS: &[&str] = &[
    "if-modified-since",
    "if-unmodified-since",
    "if-none-match",
    "if-match",
];

pub(in crate::server) struct CacheRevalidation {
    last_modified: Option<String>,
    etag: Option<String>,
}

impl CacheRevalidation {
    pub(in crate::server) fn from_cached(cached: &CachedResponse) -> Self {
        let last_modified = cached
            .last_modified
            .and_then(|timestamp| UNIX_EPOCH.checked_add(Duration::from_secs(timestamp)))
            .map(httpdate::fmt_http_date);
        Self {
            last_modified,
            etag: cached.etag.clone(),
        }
    }

    pub(in crate::server) fn apply(&self, outgoing: &mut ProxyRequestMessage) {
        outgoing.headers.retain(|(name, _)| {
            !CACHE_CONDITIONAL_HEADERS
                .iter()
                .any(|header| name.eq_ignore_ascii_case(header))
        });
        if let Some(last_modified) = &self.last_modified {
            outgoing
                .headers
                .push(("If-Modified-Since".into(), last_modified.clone()));
        }
        if let Some(etag) = &self.etag {
            outgoing
                .headers
                .push(("If-None-Match".into(), etag.clone()));
        }
    }

    pub(in crate::server) fn apply_to_request(&self, request: &mut Request) {
        request.headers.retain(|header| {
            !CACHE_CONDITIONAL_HEADERS
                .iter()
                .any(|name| header.name.eq_ignore_ascii_case(name))
        });
        if let Some(last_modified) = &self.last_modified {
            request.headers.push(Header {
                name: "If-Modified-Since".into(),
                value: last_modified.clone(),
            });
        }
        if let Some(etag) = &self.etag {
            request.headers.push(Header {
                name: "If-None-Match".into(),
                value: etag.clone(),
            });
        }
    }
}

pub(in crate::server) fn revalidated_validity(
    upstream: &Response,
    cached: &CachedResponse,
    context: &EffectiveConfig,
) -> Option<CacheValidity> {
    revalidated_upstream_validity(
        upstream,
        cached,
        &context.proxy_ignore_headers,
        &context.proxy_cache_valid,
    )
}

pub(in crate::server) fn revalidated_upstream_validity(
    upstream: &Response,
    cached: &CachedResponse,
    ignore_headers: &[String],
    valid_rules: &[crate::config::ProxyCacheValid],
) -> Option<CacheValidity> {
    upstream_cache_validity(upstream, ignore_headers, valid_rules).or_else(|| {
        cached_upstream_cache_validity(cached.status, &cached.headers, ignore_headers, valid_rules)
            .map(|mut validity| {
                validity.stale = cached.stale_windows;
                validity
            })
    })
}
