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
    let mut prefix = Vec::new();
    encode_fastcgi_record(
        &mut prefix,
        FCGI_BEGIN_REQUEST,
        &[0, 1, flags, 0, 0, 0, 0, 0],
    );
    for (name, value) in build_fastcgi_params(request, env, context, variables) {
        let mut parameter = Vec::with_capacity(name.len() + value.len() + 8);
        write_fastcgi_length(&mut parameter, name.len());
        write_fastcgi_length(&mut parameter, value.len());
        parameter.extend_from_slice(name.as_bytes());
        parameter.extend_from_slice(value.as_bytes());
        encode_fastcgi_record(&mut prefix, FCGI_PARAMS, &parameter);
    }
    encode_fastcgi_record(&mut prefix, FCGI_PARAMS, &[]);

    let mut sent_end_record = false;
    if let Some(body_stream) = stream {
        let mut total = 0_u64;
        let mut body_error = None;
        loop {
            match body_stream.try_next().await {
                Ok(Some(chunk)) => {
                    if chunk.is_empty() {
                        continue;
                    }
                    total = total
                        .checked_add(chunk.len() as u64)
                        .ok_or_else(crate::http::request_body_too_large_error)?;
                    if limit != 0 && total > limit {
                        return Err(crate::http::request_body_too_large_error());
                    }
                    append_fastcgi_stdin_records(&mut prefix, &chunk);
                }
                Ok(None) => {
                    if body_stream.channel_ended() {
                        encode_fastcgi_record(&mut prefix, FCGI_STDIN, &[]);
                        sent_end_record = true;
                    }
                    break;
                }
                Err(error) => {
                    body_error = Some(error);
                    break;
                }
            }
        }
        write_all_with_timeout(writer, &prefix, send_timeout).await?;
        if let Some(error) = body_error {
            return Err(error);
        }
        while let Some(chunk) = body_stream.next_limited(&mut total, limit).await? {
            if chunk.is_empty() {
                continue;
            }
            let mut records = Vec::with_capacity(chunk.len() + 64);
            append_fastcgi_stdin_records(&mut records, &chunk);
            let ended = body_stream.channel_ended();
            if ended {
                encode_fastcgi_record(&mut records, FCGI_STDIN, &[]);
                sent_end_record = true;
            }
            write_all_with_timeout(writer, &records, send_timeout).await?;
            if ended {
                break;
            }
        }
    } else {
        let body = context
            .fastcgi_pass_request_body
            .then_some(request.body.as_slice())
            .unwrap_or_default();
        if body.len() <= 64 * 1024 {
            for chunk in body.chunks(u16::MAX as usize) {
                encode_fastcgi_record(&mut prefix, FCGI_STDIN, chunk);
            }
            encode_fastcgi_record(&mut prefix, FCGI_STDIN, &[]);
            write_all_with_timeout(writer, &prefix, send_timeout).await?;
        } else {
            write_all_with_timeout(writer, &prefix, send_timeout).await?;
            for chunk in body.chunks(u16::MAX as usize) {
                let mut records = Vec::with_capacity(chunk.len() + 16);
                encode_fastcgi_record(&mut records, FCGI_STDIN, chunk);
                write_all_with_timeout(writer, &records, send_timeout).await?;
            }
        }
    }
    if (stream.is_some() && !sent_end_record) || request.body.len() > 64 * 1024 {
        write_fastcgi_record_with_timeout(writer, FCGI_STDIN, &[], send_timeout).await?;
    }
    flush_with_timeout(writer, send_timeout).await
}

fn append_fastcgi_stdin_records(output: &mut Vec<u8>, data: &[u8]) {
    for chunk in data.chunks(u16::MAX as usize) {
        encode_fastcgi_record(output, FCGI_STDIN, chunk);
    }
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
    let mut record = Vec::with_capacity(content.len() + 15);
    encode_fastcgi_record(&mut record, record_type, content);
    writer.write_all(&record).await
}

fn encode_fastcgi_record(output: &mut Vec<u8>, record_type: u8, content: &[u8]) {
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
    output.extend_from_slice(&header);
    output.extend_from_slice(content);
    if padding > 0 {
        output.resize(output.len() + padding, 0);
    }
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
