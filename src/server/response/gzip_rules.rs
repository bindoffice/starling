use super::*;

pub(super) fn request_allows_gzip(context: &EffectiveConfig, request: &Request) -> bool {
    context.gzip_http_version.allows(&request.version)
        && !gzip_disabled_for_user_agent(&context.gzip_disable, request.header("user-agent"))
}

fn gzip_disabled_for_user_agent(config: &GzipDisableConfig, user_agent: Option<&str>) -> bool {
    let Some(user_agent) = user_agent else {
        return false;
    };
    (config.msie6 && super::super::user_agent::is_msie6(user_agent))
        || config
            .regexes
            .iter()
            .any(|regex| regex.is_match(user_agent))
}
