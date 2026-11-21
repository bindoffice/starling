use super::*;

pub(in crate::server) struct FastCgiRequestBody<'a> {
    pub(in crate::server) stream: Option<&'a crate::http::RequestBodyStream>,
    pub(in crate::server) limit: u64,
    pub(in crate::server) send_timeout: Duration,
}

pub(in crate::server) async fn write_fastcgi_request_with_stream<W: AsyncWrite + Unpin>(
    writer: &mut W,
    request: &Request,
    env: &RequestEnv,
    context: &EffectiveConfig,
    variables: &Variables,
    body: FastCgiRequestBody<'_>,
) -> io::Result<()> {
    let FastCgiRequestBody {
        stream,
        limit,
        send_timeout,
    } = body;
    let flags = u8::from(context.fastcgi_keep_conn);
    write_fastcgi_record_with_timeout(
        writer,
        FCGI_BEGIN_REQUEST,
        &[0, 1, flags, 0, 0, 0, 0, 0],
        send_timeout,
    )
    .await?;
    for (name, value) in build_fastcgi_params(request, env, context, variables) {
        let mut parameter = Vec::with_capacity(name.len() + value.len() + 8);
        write_fastcgi_length(&mut parameter, name.len());
        write_fastcgi_length(&mut parameter, value.len());
        parameter.extend_from_slice(name.as_bytes());
        parameter.extend_from_slice(value.as_bytes());
        write_fastcgi_record_with_timeout(writer, FCGI_PARAMS, &parameter, send_timeout).await?;
    }
    write_fastcgi_record_with_timeout(writer, FCGI_PARAMS, &[], send_timeout).await?;
    if let Some(body_stream) = stream {
        let mut total = 0_u64;
        while let Some(chunk) = body_stream.next_limited(&mut total, limit).await? {
            for chunk in chunk.chunks(u16::MAX as usize) {
                write_fastcgi_record_with_timeout(writer, FCGI_STDIN, chunk, send_timeout).await?;
            }
        }
    } else if context.fastcgi_pass_request_body {
        let body = request.body.as_slice();
        for chunk in body.chunks(u16::MAX as usize) {
            write_fastcgi_record_with_timeout(writer, FCGI_STDIN, chunk, send_timeout).await?;
        }
    }
    write_fastcgi_record_with_timeout(writer, FCGI_STDIN, &[], send_timeout).await?;
    flush_with_timeout(writer, send_timeout).await
}

async fn write_fastcgi_record_with_timeout<W: AsyncWrite + Unpin>(
    writer: &mut W,
    record_type: u8,
    content: &[u8],
    timeout: Duration,
) -> io::Result<()> {
    let mut writer = TimedWriter::new(writer, timeout);
    write_fastcgi_record(&mut writer, record_type, content).await
}

pub(in crate::server) fn write_fastcgi_length(output: &mut Vec<u8>, length: usize) {
    if length < 128 {
        output.push(length as u8);
    } else {
        output.extend_from_slice(&(length as u32 | 0x8000_0000).to_be_bytes());
    }
}

pub(in crate::server) async fn write_fastcgi_record<W: AsyncWrite + Unpin>(
    writer: &mut W,
    record_type: u8,
    content: &[u8],
) -> io::Result<()> {
    let padding = (8 - content.len() % 8) % 8;
    let header = [
        1,
        record_type,
        0,
        1,
        (content.len() >> 8) as u8,
        content.len() as u8,
        padding as u8,
        0,
    ];
    writer.write_all(&header).await?;
    writer.write_all(content).await?;
    if padding > 0 {
        writer.write_all(&[0_u8; 8][..padding]).await?;
    }
    Ok(())
}

pub(in crate::server) fn build_fastcgi_params(
    request: &Request,
    env: &RequestEnv,
    context: &EffectiveConfig,
    variables: &Variables,
) -> Vec<(String, String)> {
    let mut params = Vec::with_capacity(context.fastcgi_params.len() + 4);
    for parameter in &context.fastcgi_params {
        let value = eval_variables(&parameter.value, variables);
        if parameter.if_not_empty && value.is_empty() {
            continue;
        }
        params.push((parameter.name.clone(), value));
    }
    if context.fastcgi_pass_request_headers {
        append_cgi_request_headers(&mut params, &request.headers);
    }
    push_fastcgi_default(&mut params, "REQUEST_METHOD", &request.method);
    push_fastcgi_default(
        &mut params,
        "QUERY_STRING",
        env.args.as_deref().unwrap_or(""),
    );
    push_fastcgi_default(
        &mut params,
        "CONTENT_LENGTH",
        &request
            .header("content-length")
            .map(str::to_string)
            .or_else(|| request.body_file().map(|file| file.len().to_string()))
            .unwrap_or_default(),
    );
    push_fastcgi_default(&mut params, "SERVER_PROTOCOL", &request.version);
    push_fastcgi_default(&mut params, "GATEWAY_INTERFACE", "CGI/1.1");
    params
}

pub(in crate::server) fn push_fastcgi_default(
    params: &mut Vec<(String, String)>,
    name: &str,
    value: &str,
) {
    if !params.iter().any(|(existing, _)| existing == name) {
        params.push((name.to_string(), value.to_string()));
    }
}
