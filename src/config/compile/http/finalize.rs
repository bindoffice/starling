use super::validation::validate_http_servers;
use super::*;

impl HttpCompileState<'_> {
    pub(super) fn finish(mut self, directive: &Directive) -> ConfigResult<HttpConfig> {
        finalize_scope_directives(&mut self.context, &mut self.seen_scope);
        validate_upstream_buffering(&self.context, &directive.location)?;
        finalize_secure_link(
            &mut self.context.secure_link,
            &self.seen_secure_link,
            &directive.location,
        )?;

        self.context.default_type = self.default_type.clone();
        self.context.mime_types = self.mime_types.clone();
        self.context.maps = Arc::new(self.maps.clone());
        self.context.geos = Arc::new(self.geos.clone());
        self.context.split_clients = Arc::new(self.split_clients.clone());
        self.context.limit_req_zones = Arc::new(self.limit_req_zones.clone());
        self.context.limit_conn_zones = Arc::new(self.limit_conn_zones.clone());
        self.context.charset_maps = Arc::new(std::mem::take(&mut self.charset_maps));

        for limit_directive in self.limit_req_directives.iter().copied() {
            let limit = compile_limit_req(limit_directive, &self.context.limit_req_zones)?;
            if self
                .context
                .limit_reqs
                .iter()
                .any(|existing| existing.zone == limit.zone)
            {
                return Err(ConfigError::at(
                    limit_directive.location.clone(),
                    "\"limit_req\" directive is duplicate",
                ));
            }
            self.context.limit_reqs.push(limit);
        }
        for limit_directive in self.limit_conn_directives.iter().copied() {
            let limit = compile_limit_conn(limit_directive, &self.context.limit_conn_zones)?;
            if self
                .context
                .limit_conns
                .iter()
                .any(|existing| existing.zone == limit.zone)
            {
                return Err(ConfigError::at(
                    limit_directive.location.clone(),
                    "\"limit_conn\" directive is duplicate",
                ));
            }
            self.context.limit_conns.push(limit);
        }
        if !self.access_log_seen {
            self.context.access_log = default_http_access_log(self.prefix);
        }

        let servers = self
            .server_directives
            .iter()
            .copied()
            .enumerate()
            .map(|(id, server)| compile_server(server, &self.context, self.prefix, id))
            .collect::<ConfigResult<Vec<_>>>()?;
        validate_all_listen_conflicts(
            servers.iter().flat_map(|server| &server.listens),
            &directive.location,
        )?;
        validate_http_servers(
            &servers,
            &self.cache_paths,
            &self.log_formats,
            &directive.location,
        )?;

        Ok(HttpConfig {
            servers,
            error_logs: self.context.error_logs,
            upstreams: self.upstreams,
            cache_paths: self.cache_paths,
            maps: self.maps,
            geos: self.geos,
            split_clients: self.split_clients,
            limit_req_zones: self.limit_req_zones,
            limit_conn_zones: self.limit_conn_zones,
            mime_types: self.mime_types,
            default_type: self.default_type,
            log_formats: self.log_formats,
        })
    }
}
