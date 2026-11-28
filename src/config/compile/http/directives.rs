use super::super::compat::reject_ssl_conf_command;
use super::*;

impl<'a> HttpCompileState<'a> {
    pub(super) fn apply(&mut self, child: &'a Directive) -> ConfigResult<()> {
        if try_apply_error_log_directive(
            child,
            &mut self.context,
            self.prefix,
            &mut self.seen_scope.error_log,
        )? {
            return Ok(());
        }
        match child.name.as_str() {
            "types" => {
                require_block(child)?;
                compile_types(child, &mut self.mime_types)?;
            }
            "default_type" => {
                self.default_type = one_arg(child)?.to_string();
                self.context.default_type = self.default_type.clone();
            }
            "http2" => self.context.http2 = parse_flag(child, "http2")?,
            "http3" => self.context.http3 = parse_flag(child, "http3")?,
            "rewrite_log" => self.context.rewrite_log = parse_flag(child, "rewrite_log")?,
            "uninitialized_variable_warn" => {
                self.context.uninitialized_variable_warn =
                    parse_flag(child, "uninitialized_variable_warn")?;
            }
            "ssl_protocols" => {
                apply_tls_protocols(
                    child,
                    &mut self.context.ssl_protocols,
                    &mut self.seen_ssl_protocols,
                )?;
            }
            "proxy_ssl_protocols" => {
                apply_tls_protocols(
                    child,
                    &mut self.context.proxy_ssl_protocols,
                    &mut self.seen_proxy_ssl_protocols,
                )?;
            }
            "ssl_prefer_server_ciphers" => {
                self.context.ssl_prefer_server_ciphers =
                    parse_flag(child, "ssl_prefer_server_ciphers")?;
            }
            "ssl_ciphers" => self.context.ssl_ciphers = parse_tls_cipher_config(child)?,
            "proxy_ssl_ciphers" => self.context.proxy_ssl_ciphers = parse_tls_cipher_config(child)?,
            "ssl_verify_client" => {
                self.context.ssl_verify_client = parse_ssl_verify_client(child)?;
            }
            "ssl_verify_depth" => {
                self.context.ssl_verify_depth = parse_ssl_verify_depth(child)?;
            }
            "ssl_client_certificate" => {
                self.context.ssl_client_certificate =
                    Some(resolve_path(self.prefix, one_arg(child)?));
            }
            "ssl_trusted_certificate" => {
                self.context.ssl_trusted_certificate =
                    Some(resolve_path(self.prefix, one_arg(child)?));
            }
            "ssl_stapling"
            | "ssl_stapling_verify"
            | "ssl_stapling_file"
            | "ssl_stapling_responder"
            | "ssl_ocsp"
            | "ssl_ocsp_responder"
            | "ssl_ocsp_cache"
            | "resolver"
            | "resolver_timeout" => {
                try_apply_stapling_directive(
                    child,
                    &mut self.context,
                    self.prefix,
                    Scope::Server,
                    &mut self.seen_scope.stapling,
                )?;
            }
            "ssl_conf_command" => {
                reject_ssl_conf_command(child, "ssl_conf_command")?;
            }
            "ssl_crl" => {
                self.context.ssl_crl = Some(resolve_path(self.prefix, one_arg(child)?));
            }
            "ssl_password_file" => {
                self.context.ssl_passwords = parse_password_file(self.prefix, child)?;
            }
            "server" => {
                require_block(child)?;
                self.server_directives.push(child);
            }
            "upstream" => {
                require_block(child)?;
                let upstream = compile_upstream(child, UpstreamContext::Http)?;
                let name = upstream.name.to_string();
                if self.upstreams.insert(name, upstream).is_some() {
                    return Err(ConfigError::at(
                        child.location.clone(),
                        "duplicate upstream",
                    ));
                }
            }
            "proxy_cache_path" | "fastcgi_cache_path" | "uwsgi_cache_path" | "scgi_cache_path" => {
                let cache = compile_cache_path(child, self.prefix)?;
                if self
                    .cache_paths
                    .insert(cache.keys_zone.clone(), Arc::new(cache))
                    .is_some()
                {
                    return Err(ConfigError::at(
                        child.location.clone(),
                        "duplicate cache zone",
                    ));
                }
            }
            "map" => {
                require_block(child)?;
                let map = compile_map(child)?;
                if self.geos.contains_key(&map.name)
                    || self.split_clients.contains_key(&map.name)
                    || self.maps.insert(map.name.clone(), Arc::new(map)).is_some()
                {
                    return Err(ConfigError::at(
                        child.location.clone(),
                        "duplicate map variable",
                    ));
                }
            }
            "geo" => {
                require_block(child)?;
                let geo = compile_geo(child)?;
                if self.maps.contains_key(&geo.name)
                    || self.split_clients.contains_key(&geo.name)
                    || self.geos.insert(geo.name.clone(), Arc::new(geo)).is_some()
                {
                    return Err(ConfigError::at(
                        child.location.clone(),
                        "duplicate geo variable",
                    ));
                }
            }
            "split_clients" => {
                require_block(child)?;
                let split = compile_split_clients(child)?;
                if self.maps.contains_key(&split.name)
                    || self.geos.contains_key(&split.name)
                    || self
                        .split_clients
                        .insert(split.name.clone(), Arc::new(split))
                        .is_some()
                {
                    return Err(ConfigError::at(
                        child.location.clone(),
                        "duplicate split_clients variable",
                    ));
                }
            }
            "limit_req_zone" => {
                validate_no_block(child)?;
                let zone = compile_limit_req_zone(child)?;
                if self
                    .limit_req_zones
                    .insert(zone.name.clone(), Arc::new(zone))
                    .is_some()
                {
                    return Err(ConfigError::at(
                        child.location.clone(),
                        "duplicate limit_req_zone",
                    ));
                }
            }
            "limit_conn_zone" => {
                validate_no_block(child)?;
                let zone = compile_limit_conn_zone(child)?;
                if self
                    .limit_conn_zones
                    .insert(zone.name.clone(), Arc::new(zone))
                    .is_some()
                {
                    return Err(ConfigError::at(
                        child.location.clone(),
                        "duplicate limit_conn_zone",
                    ));
                }
            }
            "charset_map" => {
                let charset_map = compile_charset_map(child)?;
                if self.charset_maps.iter().any(|existing: &CharsetMap| {
                    existing.connects(&charset_map.source, &charset_map.destination)
                }) {
                    return Err(ConfigError::at(
                        child.location.clone(),
                        format!(
                            "duplicate \"charset_map\" between \"{}\" and \"{}\"",
                            charset_map.source, charset_map.destination
                        ),
                    ));
                }
                self.charset_maps.push(charset_map);
            }
            "limit_req" => self.limit_req_directives.push(child),
            "limit_conn" => self.limit_conn_directives.push(child),
            "limit_req_status" => {
                self.context.limit_req_status = parse_limit_status(child, "limit_req_status")?;
            }
            "limit_conn_status" => {
                self.context.limit_conn_status = parse_limit_status(child, "limit_conn_status")?;
            }
            "limit_req_dry_run" => {
                self.context.limit_req_dry_run = parse_flag(child, "limit_req_dry_run")?;
            }
            "limit_conn_dry_run" => {
                self.context.limit_conn_dry_run = parse_flag(child, "limit_conn_dry_run")?;
            }
            "limit_req_log_level" => {
                self.context.limit_req_log_level = parse_limit_log_level(child, &child.name)?;
            }
            "limit_conn_log_level" => {
                self.context.limit_conn_log_level = parse_limit_log_level(child, &child.name)?;
            }
            "log_format" => compile_log_format(child, &mut self.log_formats, true)?,
            "open_log_file_cache" => {
                try_apply_open_log_file_cache_directive(
                    child,
                    &mut self.context.open_log_file_cache,
                    &mut self.seen_scope.open_log_file_cache,
                )?;
            }
            "access_log" => {
                apply_access_log_directive(
                    child,
                    self.prefix,
                    Some("combined"),
                    &mut self.context.access_log,
                    &mut self.access_log_seen,
                )?;
            }
            "add_header" => self.seen_scope.add_headers.push(parse_add_header(child)?),
            "add_trailer" => self.seen_scope.add_trailers.push(parse_add_header(child)?),
            "add_header_inherit" => {
                if self.seen_scope.add_header_inherit {
                    return Err(ConfigError::at(
                        child.location.clone(),
                        "\"add_header_inherit\" directive is duplicate",
                    ));
                }
                self.seen_scope.add_header_inherit = true;
                self.context.add_header_inherit = parse_header_inheritance(child)?;
            }
            "add_trailer_inherit" => {
                if self.seen_scope.add_trailer_inherit {
                    return Err(ConfigError::at(
                        child.location.clone(),
                        "\"add_trailer_inherit\" directive is duplicate",
                    ));
                }
                self.seen_scope.add_trailer_inherit = true;
                self.context.add_trailer_inherit = parse_header_inheritance(child)?;
            }
            "add_before_body" => apply_addition_part(
                child,
                &mut self.context,
                &mut self.seen_addition,
                AdditionPart::Before,
            )?,
            "add_after_body" => apply_addition_part(
                child,
                &mut self.context,
                &mut self.seen_addition,
                AdditionPart::After,
            )?,
            "addition_types" => {
                apply_addition_types(child, &mut self.context, &mut self.seen_addition)?
            }
            name if is_ssi_directive(name) => {
                apply_ssi_directive(child, &mut self.context, &mut self.seen_ssi)?
            }
            "expires" => {
                apply_expires_directive(child, &mut self.context, &mut self.seen_expires)?;
            }
            "if_modified_since" => {
                validate_no_block(child)?;
                if self.seen_if_modified_since {
                    return Err(ConfigError::at(
                        child.location.clone(),
                        "\"if_modified_since\" directive is duplicate",
                    ));
                }
                self.seen_if_modified_since = true;
                self.context.if_modified_since = parse_if_modified_since(child)?;
            }
            "sub_filter" => {
                validate_no_block(child)?;
                self.context.sub_filters.push(parse_sub_filter(child)?);
            }
            "sub_filter_types" => {
                validate_no_block(child)?;
                self.context.sub_filter_types = parse_sub_filter_types(child)?;
            }
            "sub_filter_once" => {
                self.context.sub_filter_once = parse_flag(child, "sub_filter_once")?;
            }
            "sub_filter_last_modified" => {
                self.context.sub_filter_last_modified =
                    parse_flag(child, "sub_filter_last_modified")?;
            }
            "proxy_set_header" => {
                validate_no_block(child)?;
                if child.args.len() != 2 {
                    return Err(ConfigError::at(
                        child.location.clone(),
                        "invalid number of arguments in \"proxy_set_header\" directive",
                    ));
                }
                self.context.proxy_headers.push(HeaderValue {
                    name: child.args[0].clone(),
                    value: child.args[1].clone(),
                });
            }
            "proxy_redirect" => {
                apply_proxy_redirect(child, &mut self.context, &mut self.seen_proxy_redirect)?;
            }
            "proxy_hide_header" => {
                validate_no_block(child)?;
                self.context
                    .proxy_hide_headers
                    .push(one_arg(child)?.to_string());
            }
            "proxy_pass_header" => {
                validate_no_block(child)?;
                self.context
                    .proxy_pass_headers
                    .push(one_arg(child)?.to_string());
            }
            "proxy_pass_request_headers" => {
                self.context.proxy_pass_request_headers =
                    parse_flag(child, "proxy_pass_request_headers")?;
            }
            "proxy_pass_request_body" => {
                self.context.proxy_pass_request_body =
                    parse_flag(child, "proxy_pass_request_body")?;
            }
            "proxy_method" => {
                validate_no_block(child)?;
                self.context.proxy_method = Some(one_arg(child)?.to_string());
            }
            "proxy_set_body" => {
                validate_no_block(child)?;
                self.context.proxy_set_body = Some(one_arg(child)?.to_string());
            }
            "proxy_intercept_errors" => {
                self.context.proxy_intercept_errors = parse_flag(child, "proxy_intercept_errors")?;
            }
            "proxy_cookie_domain" => apply_proxy_cookie(
                child,
                &mut self.context,
                ProxyCookieKind::Domain,
                &mut self.seen_proxy_cookie_domain,
            )?,
            "proxy_cookie_path" => apply_proxy_cookie(
                child,
                &mut self.context,
                ProxyCookieKind::Path,
                &mut self.seen_proxy_cookie_path,
            )?,
            "proxy_cookie_flags" => apply_proxy_cookie_flags(
                child,
                &mut self.context,
                &mut self.seen_proxy_cookie_flags,
            )?,
            "set_real_ip_from" => {
                apply_set_real_ip_from(child, &mut self.context, &mut self.seen_real_ip_from)?
            }
            "real_ip_header" => {
                validate_no_block(child)?;
                let header = one_arg(child)?;
                self.context.real_ip_header = header.to_string();
            }
            "real_ip_recursive" => {
                self.context.real_ip_recursive = parse_flag(child, "real_ip_recursive")?;
            }
            "allow" | "deny" => self.context.access_rules.push(parse_access_rule(child)?),
            "mirror" => apply_mirror_directive(child, &mut self.context, &mut self.seen_mirror)?,
            "mirror_request_body" => {
                apply_mirror_request_body(child, &mut self.context, &mut self.seen_mirror)?
            }
            name if is_userid_directive(name) => {
                apply_userid_directive(child, &mut self.context, &mut self.seen_userid)?
            }
            _ => apply_http_directive(
                child,
                &mut self.context,
                self.prefix,
                &mut self.seen_secure_link,
                &mut self.seen_browser,
                &mut self.seen_scope,
            )?,
        }
        Ok(())
    }
}
