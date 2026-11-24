use super::*;

pub(super) async fn write_uwsgi_request<S>(
    stream: &mut S,
    request: &Request,
    context: &EffectiveConfig,
    variables: &Variables,
) -> Result<(), ProxyFailure>
where
    S: AsyncWrite + Unpin,
{
    let streaming_request_body = request_body_stream_enabled(request, context);
    let packet = build_uwsgi_header(request, context, variables, streaming_request_body)
        .map_err(|_| ProxyFailure::Failure)?;
    write_all_with_timeout(stream, &packet, context.uwsgi_send_timeout)
        .await
        .map_err(proxy_failure_from_io)?;
    if let Some(body_stream) = context
        .uwsgi_pass_request_body
        .then(|| request_body_stream_for_upstream(request, context))
        .flatten()
        .cloned()
    {
        let mut total = 0_u64;
        while let Some(chunk) = body_stream
            .next_limited(&mut total, context.client_max_body_size)
            .await
            .map_err(proxy_failure_from_io)?
        {
            write_all_with_timeout(stream, &chunk, context.uwsgi_send_timeout)
                .await
                .map_err(proxy_failure_from_io)?;
        }
    } else if context.uwsgi_pass_request_body {
        write_all_with_timeout(stream, &request.body, context.uwsgi_send_timeout)
            .await
            .map_err(proxy_failure_from_io)?;
    }
    flush_with_timeout(stream, context.uwsgi_send_timeout)
        .await
        .map_err(proxy_failure_from_io)
}

pub(super) fn uwsgi_upstream_pool_key(
    host: &str,
    port: u16,
    tls: bool,
    context: &EffectiveConfig,
    variables: &Variables,
) -> UpstreamPoolKey {
    UpstreamPoolKey {
        generation: variables.upstream_keepalive_generation,
        scheme: if tls { "suwsgi" } else { "uwsgi" }.into(),
        host: host.to_string(),
        port,
        tls_name: tls.then(|| uwsgi_tls_name(context, variables, host)),
        tls_verify: tls && context.uwsgi_ssl_verify,
        tls_trusted_certificate: tls
            .then(|| context.uwsgi_ssl_trusted_certificate.clone())
            .flatten(),
        tls_verify_depth: tls.then_some(context.uwsgi_ssl_verify_depth),
        tls_crl: tls.then(|| context.uwsgi_ssl_crl.clone()).flatten(),
        tls_client_certificate: tls.then(|| context.uwsgi_ssl_certificate.clone()).flatten(),
        tls_client_certificate_key: tls
            .then(|| context.uwsgi_ssl_certificate_key.clone())
            .flatten(),
        tls_protocols: tls.then_some(context.uwsgi_ssl_protocols),
        tls_ciphers: tls.then(|| context.uwsgi_ssl_ciphers.clone()),
        proxy_bind: evaluate_uwsgi_bind(context, variables),
        socket_keepalive: context.uwsgi_socket_keepalive,
        socket_rcvbuf: context.uwsgi_socket_rcvbuf,
        socket_sndbuf: context.uwsgi_socket_sndbuf,
        http_version: "uwsgi".into(),
    }
}

fn build_uwsgi_header(
    request: &Request,
    context: &EffectiveConfig,
    variables: &Variables,
    streaming_request_body: bool,
) -> io::Result<Vec<u8>> {
    let mut parameters = Vec::new();
    for parameter in &context.uwsgi_params {
        let value = eval_variables(&parameter.value, variables);
        if parameter.if_not_empty && value.is_empty() {
            continue;
        }
        parameters.push((parameter.name.clone(), value));
    }
    if context.uwsgi_pass_request_headers {
        append_cgi_request_headers(&mut parameters, &request.headers);
    }
    if !parameters
        .iter()
        .any(|(name, _)| name.eq_ignore_ascii_case("CONTENT_LENGTH"))
    {
        let content_length = request
            .header("content-length")
            .map(str::to_string)
            .or_else(|| request.body_file().map(|file| file.len().to_string()))
            .unwrap_or_else(|| {
                if streaming_request_body {
                    String::new()
                } else {
                    request.body.len().to_string()
                }
            });
        parameters.push(("CONTENT_LENGTH".into(), content_length));
    }
    let mut parameters = encode_null_separated_params(&parameters);
    parameters.extend_from_slice(context.uwsgi_string.as_bytes());
    if parameters.len() > u16::MAX as usize {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "uWSGI request is too large",
        ));
    }
    let mut packet = Vec::with_capacity(4 + parameters.len());
    packet.extend_from_slice(&[
        context.uwsgi_modifier1,
        (parameters.len() & 0xff) as u8,
        ((parameters.len() >> 8) & 0xff) as u8,
        context.uwsgi_modifier2,
    ]);
    packet.extend_from_slice(&parameters);
    Ok(packet)
}
