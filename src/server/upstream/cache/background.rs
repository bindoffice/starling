use super::*;

pub(in crate::server) struct BackgroundUpdateContext<'a> {
    pub(in crate::server) proxy: &'a ProxyPassConfig,
    pub(in crate::server) http: &'a HttpConfig,
    pub(in crate::server) request: &'a Request,
    pub(in crate::server) env: &'a RequestEnv,
    pub(in crate::server) location: Option<&'a LocationConfig>,
    pub(in crate::server) config: &'a EffectiveConfig,
    pub(in crate::server) variables: &'a Variables,
}

pub(in crate::server) fn start_background_update(
    update: BackgroundUpdateContext<'_>,
    guard: CacheLockGuard,
) {
    let proxy = update.proxy.clone();
    let http = update.http.clone();
    let request = update.request.clone();
    let env = update.env.clone();
    let location = update.location.cloned();
    let config = update.config.clone();
    let mut variables = update.variables.clone();
    variables.proxy_cache_background = true;
    tokio::spawn(async move {
        let _guard = guard;
        let _ = super::super::proxy_request(
            &proxy,
            &http,
            &request,
            &env,
            location.as_ref(),
            &config,
            &mut variables,
        )
        .await;
    });
}
