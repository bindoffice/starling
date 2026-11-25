use super::*;

pub(in crate::server) fn connection_server_context<'a>(
    config: &'a RuntimeConfig,
    group: &ListenerGroup,
) -> Option<&'a ServerConfig> {
    let http = config.http.as_ref()?;
    http.servers
        .get(group.default_server_id)
        .or_else(|| http.servers.first())
}

pub(in crate::server) fn response_rate(
    context: &EffectiveConfig,
    variables: &Variables,
) -> ResponseRate {
    let rate = context
        .limit_rate
        .as_deref()
        .map(|value| eval_variables(value, variables))
        .and_then(|value| parse_size_value(&value))
        .unwrap_or(0);
    let after = context
        .limit_rate_after
        .as_deref()
        .map(|value| eval_variables(value, variables))
        .and_then(|value| parse_size_value(&value))
        .unwrap_or(0);
    ResponseRate { rate, after }
}

pub(in crate::server) fn complex_size(value: Option<&str>, variables: &Variables) -> u64 {
    value
        .map(|value| eval_variables(value, variables))
        .and_then(|value| parse_size_value(&value))
        .unwrap_or(0)
}

pub(in crate::server) fn request_server_context<'a>(
    config: &'a RuntimeConfig,
    listener_groups: &[ListenerGroup],
    group: &ListenerGroup,
    host: &str,
) -> Option<&'a ServerConfig> {
    let http = config.http.as_ref()?;
    let active_group = current_listener_group(listener_groups, group);
    Some(select_server(http, active_group, host))
}

pub(in crate::server) async fn timeout_io<T>(
    duration: Duration,
    future: impl std::future::Future<Output = io::Result<T>>,
) -> io::Result<T> {
    if duration == Duration::ZERO {
        return future.await;
    }
    match tokio::time::timeout(duration, future).await {
        Ok(result) => result,
        Err(_) => Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "I/O operation timed out",
        )),
    }
}

pub(in crate::server) struct TimeoutState {
    pub(in crate::server) timeout: Duration,
    pub(in crate::server) sleep: Option<Pin<Box<tokio::time::Sleep>>>,
    pub(in crate::server) expired: bool,
}

impl TimeoutState {
    pub(in crate::server) fn new(timeout: Duration) -> Self {
        Self {
            timeout,
            sleep: None,
            expired: false,
        }
    }

    pub(in crate::server) fn reset(&mut self) {
        if let Some(sleep) = self.sleep.as_mut() {
            sleep
                .as_mut()
                .reset(tokio::time::Instant::now() + self.timeout);
        }
    }

    pub(in crate::server) fn poll_expired(&mut self, cx: &mut Context<'_>) -> bool {
        if self.timeout == Duration::ZERO {
            return false;
        }
        let sleep = self
            .sleep
            .get_or_insert_with(|| Box::pin(tokio::time::sleep(self.timeout)));
        if std::future::Future::poll(sleep.as_mut(), cx).is_ready() {
            self.expired = true;
        }
        self.expired
    }
}

pub(in crate::server) struct TimedReader<'a, R> {
    pub(in crate::server) inner: &'a mut R,
    pub(in crate::server) timeout: TimeoutState,
}

impl<'a, R> TimedReader<'a, R> {
    pub(in crate::server) fn new(inner: &'a mut R, timeout: Duration) -> Self {
        Self {
            inner,
            timeout: TimeoutState::new(timeout),
        }
    }
}

impl<R: AsyncRead + Unpin> AsyncRead for TimedReader<'_, R> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        if this.timeout.expired {
            return Poll::Ready(Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "read timed out",
            )));
        }
        let before = buffer.filled().len();
        match Pin::new(&mut *this.inner).poll_read(cx, buffer) {
            Poll::Ready(Ok(())) => {
                if buffer.filled().len() > before {
                    this.timeout.reset();
                }
                Poll::Ready(Ok(()))
            }
            Poll::Ready(Err(err)) => Poll::Ready(Err(err)),
            Poll::Pending => {
                if this.timeout.poll_expired(cx) {
                    Poll::Ready(Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "read timed out",
                    )))
                } else {
                    Poll::Pending
                }
            }
        }
    }
}

impl<R: AsyncBufRead + Unpin> AsyncBufRead for TimedReader<'_, R> {
    fn poll_fill_buf(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<&[u8]>> {
        let this = self.get_mut();
        if this.timeout.expired {
            return Poll::Ready(Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "read timed out",
            )));
        }
        match Pin::new(&mut *this.inner).poll_fill_buf(cx) {
            Poll::Ready(Ok(buffer)) => {
                if !buffer.is_empty() {
                    this.timeout.reset();
                }
                Poll::Ready(Ok(buffer))
            }
            Poll::Ready(Err(err)) => Poll::Ready(Err(err)),
            Poll::Pending => {
                if this.timeout.poll_expired(cx) {
                    Poll::Ready(Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "read timed out",
                    )))
                } else {
                    Poll::Pending
                }
            }
        }
    }

    fn consume(self: Pin<&mut Self>, amount: usize) {
        let this = self.get_mut();
        Pin::new(&mut *this.inner).consume(amount);
    }
}

pub(in crate::server) struct TimedWriter<'a, W> {
    pub(in crate::server) inner: &'a mut W,
    pub(in crate::server) timeout: TimeoutState,
}

impl<'a, W> TimedWriter<'a, W> {
    pub(in crate::server) fn new(inner: &'a mut W, timeout: Duration) -> Self {
        Self {
            inner,
            timeout: TimeoutState::new(timeout),
        }
    }
}

impl<W: AsyncWrite + Unpin> AsyncWrite for TimedWriter<'_, W> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buffer: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        if this.timeout.expired {
            return Poll::Ready(Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "write timed out",
            )));
        }
        match Pin::new(&mut *this.inner).poll_write(cx, buffer) {
            Poll::Ready(Ok(written)) => {
                if written > 0 {
                    this.timeout.reset();
                }
                Poll::Ready(Ok(written))
            }
            Poll::Ready(Err(err)) => Poll::Ready(Err(err)),
            Poll::Pending => {
                if this.timeout.poll_expired(cx) {
                    Poll::Ready(Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "write timed out",
                    )))
                } else {
                    Poll::Pending
                }
            }
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        if this.timeout.expired {
            return Poll::Ready(Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "write timed out",
            )));
        }
        match Pin::new(&mut *this.inner).poll_flush(cx) {
            Poll::Ready(result) => {
                this.timeout.reset();
                Poll::Ready(result)
            }
            Poll::Pending => {
                if this.timeout.poll_expired(cx) {
                    Poll::Ready(Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "write timed out",
                    )))
                } else {
                    Poll::Pending
                }
            }
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut *self.get_mut().inner).poll_shutdown(cx)
    }
}

pub(in crate::server) async fn write_all_with_timeout<W: AsyncWrite + Unpin>(
    writer: &mut W,
    data: &[u8],
    timeout: Duration,
) -> io::Result<()> {
    // Nginx send_timeout measures inactivity, not the total lifetime of one
    // write. TimedWriter resets on every successful partial write.
    let mut writer = TimedWriter::new(writer, timeout);
    writer.write_all(data).await
}

pub(in crate::server) async fn flush_with_timeout<W: AsyncWrite + Unpin>(
    writer: &mut W,
    timeout: Duration,
) -> io::Result<()> {
    let mut writer = TimedWriter::new(writer, timeout);
    writer.flush().await
}

pub(in crate::server) fn select_server<'a>(
    http: &'a HttpConfig,
    group: &ListenerGroup,
    host: &str,
) -> &'a ServerConfig {
    let host = host.trim_end_matches('.');
    for server_id in &group.server_ids {
        let server = &http.servers[*server_id];
        if server.names.iter().any(
            |name| matches!(name, ServerName::Exact(value) if value.eq_ignore_ascii_case(host)),
        ) {
            return server;
        }
    }
    let mut wildcard_match: Option<(&ServerConfig, usize)> = None;
    for server_id in &group.server_ids {
        let server = &http.servers[*server_id];
        for name in server.names.iter() {
            let specificity = name.wildcard_specificity(host).unwrap_or(0);
            if specificity > 0 && wildcard_match.is_none_or(|(_, previous)| specificity > previous)
            {
                wildcard_match = Some((server, specificity));
            }
        }
    }
    if let Some((server, _)) = wildcard_match {
        return server;
    }
    for server_id in &group.server_ids {
        let server = &http.servers[*server_id];
        if server
            .names
            .iter()
            .any(|name| name.regex().is_some_and(|regex| regex.is_match(host)))
        {
            return server;
        }
    }
    &http.servers[group.default_server_id]
}

#[derive(Debug, Clone)]
pub(in crate::server) struct RequestEnv {
    pub(in crate::server) uri: String,
    pub(in crate::server) args: Option<String>,
    pub(in crate::server) unparsed_uri: Option<String>,
    pub(in crate::server) valid_location: bool,
    pub(in crate::server) internal: bool,
    pub(in crate::server) bypass_try_files: bool,
    pub(in crate::server) add_uri_to_alias: bool,
    pub(in crate::server) limits_checked: bool,
    pub(in crate::server) auth_request_checked: bool,
    pub(in crate::server) mirror_checked: bool,
}

pub(in crate::server) fn content_handler_for_context(
    context: &EffectiveConfig,
) -> Option<ContentHandlerKind> {
    if context.fastcgi_pass.is_some() {
        Some(ContentHandlerKind::FastCgi)
    } else if context.uwsgi_pass.is_some() {
        Some(ContentHandlerKind::Uwsgi)
    } else if context.scgi_pass.is_some() {
        Some(ContentHandlerKind::Scgi)
    } else if context.grpc.pass.is_some() {
        Some(ContentHandlerKind::Grpc)
    } else if context.memcached_pass.is_some() {
        Some(ContentHandlerKind::Memcached)
    } else if context.proxy_pass.is_some() {
        Some(ContentHandlerKind::Proxy)
    } else {
        context
            .tunnel
            .pass
            .is_some()
            .then_some(ContentHandlerKind::Tunnel)
    }
}

pub(in crate::server) struct Processed {
    pub(in crate::server) response: Response,
    pub(in crate::server) context: Arc<EffectiveConfig>,
    pub(in crate::server) tunnel: bool,
    pub(in crate::server) upgrade: Option<Box<ProxyUpgrade>>,
}

pub(in crate::server) fn refresh_variables(
    env: &RequestEnv,
    location: Option<&LocationConfig>,
    variables: &mut Variables,
    context: &EffectiveConfig,
) {
    variables.uri = env.uri.clone();
    let (fastcgi_script_name, fastcgi_path_info) = fastcgi_names(&env.uri, context);
    variables.fastcgi_script_name = fastcgi_script_name;
    variables.fastcgi_path_info = fastcgi_path_info;
    variables.uwsgi_script_name = env.uri.clone();
    variables.scgi_script_name = env.uri.clone();
    variables.args = env.args.clone().unwrap_or_default();
    variables.document_root = document_root_path(context, variables);
    variables.request_filename =
        map_request_path(context, location, &env.uri, env.add_uri_to_alias, variables);
    let referer = variables
        .headers
        .iter()
        .find(|header| header.name.eq_ignore_ascii_case("referer"))
        .map(|header| header.value.as_str());
    let invalid_referer =
        invalid_referer_value(&context.valid_referers, referer, &variables.server_names);
    if variables.invalid_referer != invalid_referer {
        variables.invalid_referer = invalid_referer;
    }
    refresh_secure_link(&context.secure_link, variables);
    refresh_browser_variables(&context.browser, variables);
}

pub(in crate::server) fn fastcgi_names(uri: &str, context: &EffectiveConfig) -> (String, String) {
    let captures = context
        .fastcgi_split_path_info
        .as_ref()
        .and_then(|regex| regex.captures(uri));
    let mut script_name = captures
        .as_ref()
        .and_then(|captures| captures.get(1))
        .map(|capture| capture.as_str().to_string())
        .unwrap_or_else(|| uri.to_string());
    if script_name.ends_with('/') {
        script_name.push_str(&context.fastcgi_index);
    }
    let path_info = captures
        .as_ref()
        .and_then(|captures| captures.get(2))
        .map(|capture| capture.as_str().to_string())
        .unwrap_or_default();
    (script_name, path_info)
}
