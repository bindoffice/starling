use super::*;
use crate::http::{
    BodySegment, ResponseStreamItem, buffer_pool::PooledBuffer, is_forbidden_trailer_name,
};
use bytes::Bytes;
use h3::error::Code;
use std::io;

const H3_BODY_CHUNK_SIZE: usize = 16 * 1024;

pub(super) async fn send_h3_response(
    send: &mut H3SendStream,
    mut response: Response,
    request_method: &str,
    server_tokens: ServerTokens,
) -> io::Result<()> {
    prepare_response_headers(&mut response, server_tokens);
    let mut builder = HttpResponse::builder()
        .status(StatusCode::from_u16(response.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR));
    for header in &response.headers {
        if is_h2_connection_header(&header.name) {
            continue;
        }
        let Some(name) = h2_header_name(&header.name) else {
            continue;
        };
        let Ok(value) = h2_header_value(&header.value) else {
            continue;
        };
        builder = builder.header(name, value);
    }
    let response_head = builder.body(()).map_err(io::Error::other)?;

    let is_head = request_method.eq_ignore_ascii_case("HEAD");
    let has_body = !is_head && !response.body.is_empty();
    let has_trailers = !is_head
        && !response.trailers.is_empty()
        && response.status >= 200
        && !matches!(response.status, 204 | 304);
    let has_output = has_body || has_trailers;
    let send_result = async {
        send.send_response(response_head)
            .await
            .map_err(|error| io::Error::other(error.to_string()))?;
        if !has_output {
            return Ok(());
        }

        let body = std::mem::replace(&mut response.body, ResponseBody::Empty);
        let mut trailers = send_h3_body(send, body).await?;
        if has_trailers {
            trailers.extend(response.trailers.iter().cloned());
        }
        if !trailers.is_empty() {
            send_trailers(send, &trailers).await?;
        }
        Ok(())
    }
    .await;

    match send_result {
        Ok(()) => send
            .finish()
            .await
            .map_err(|error| io::Error::other(error.to_string())),
        Err(error) => {
            send.stop_stream(h3_error_code(&error));
            Err(error)
        }
    }
}

async fn send_h3_body(send: &mut H3SendStream, body: ResponseBody) -> io::Result<Vec<Header>> {
    if let ResponseBody::Stream(mut stream) = body {
        let preserve_on_client_abort = stream.preserve_on_client_abort();
        let mut remaining = stream.length();
        let mut trailers = Vec::new();
        loop {
            let Some(item) = stream.next().await? else {
                break;
            };
            match item {
                ResponseStreamItem::Data(data) => {
                    if let Some(remaining) = remaining.as_mut() {
                        if data.len() as u64 > *remaining {
                            return Err(io::Error::new(
                                io::ErrorKind::InvalidData,
                                "response body exceeds Content-Length",
                            ));
                        }
                        *remaining -= data.len() as u64;
                    }
                    if let Err(error) = send_data(send, data).await {
                        if preserve_on_client_abort {
                            stream.drain().await;
                        }
                        return Err(error);
                    }
                }
                ResponseStreamItem::Trailers(stream_trailers) => {
                    trailers = stream_trailers;
                }
            }
        }
        if remaining.is_some_and(|remaining| remaining != 0) {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "response body ended before Content-Length",
            ));
        }
        return Ok(trailers);
    }

    let length = response_body_len(&body);
    let mut sender = H3BodySender {
        send,
        remaining: length,
    };
    match body {
        ResponseBody::Empty => {}
        ResponseBody::Bytes(body) => sender.send_bytes(Bytes::from(body)).await?,
        ResponseBody::File {
            mut file,
            offset,
            length,
        } => sender.send_file(&mut file, offset, length).await?,
        ResponseBody::FileWithPrefix {
            prefix,
            mut file,
            offset,
            length,
        } => {
            if !prefix.is_empty() {
                sender.send_bytes(Bytes::from(prefix)).await?;
            }
            sender.send_file(&mut file, offset, length).await?;
        }
        ResponseBody::Segments { source, segments } => {
            sender.send_segments(*source, &segments).await?;
        }
        ResponseBody::Stream(_) => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "streaming response body was not handled",
            ));
        }
    }
    if sender.remaining != 0 {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "response body ended before Content-Length",
        ));
    }
    Ok(Vec::new())
}

struct H3BodySender<'a> {
    send: &'a mut H3SendStream,
    remaining: u64,
}

impl H3BodySender<'_> {
    async fn send_bytes(&mut self, data: Bytes) -> io::Result<()> {
        let length = data.len() as u64;
        if length > self.remaining {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "response body exceeds Content-Length",
            ));
        }
        self.remaining -= length;
        send_data(self.send, data).await
    }

    async fn send_file(
        &mut self,
        file: &mut tokio::fs::File,
        offset: u64,
        length: u64,
    ) -> io::Result<()> {
        use tokio::io::{AsyncReadExt, AsyncSeekExt, SeekFrom};
        file.seek(SeekFrom::Start(offset)).await?;
        if length > self.remaining {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "response body exceeds Content-Length",
            ));
        }
        let mut remaining = length;
        let mut buffer = PooledBuffer::acquire(H3_BODY_CHUNK_SIZE);
        while remaining > 0 {
            let wanted = remaining.min(H3_BODY_CHUNK_SIZE as u64) as usize;
            let read = file.read(&mut buffer[..wanted]).await?;
            if read == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "static file ended before Content-Length",
                ));
            }
            remaining -= read as u64;
            self.remaining -= read as u64;
            send_data(self.send, Bytes::copy_from_slice(&buffer[..read])).await?;
        }
        Ok(())
    }

    async fn send_segments(
        &mut self,
        source: ResponseBody,
        segments: &[BodySegment],
    ) -> io::Result<()> {
        match source {
            ResponseBody::Empty => {
                for segment in segments {
                    if segment.start != 0 || segment.end != 0 {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "response body segment is out of bounds",
                        ));
                    }
                    self.send_bytes(Bytes::from(segment.prefix.clone())).await?;
                }
            }
            ResponseBody::Bytes(source) => {
                for segment in segments {
                    self.send_bytes(Bytes::from(segment.prefix.clone())).await?;
                    let (start, end) = source_bounds(source.len() as u64, segment)?;
                    self.send_bytes(Bytes::copy_from_slice(&source[start..end]))
                        .await?;
                }
            }
            ResponseBody::File {
                mut file,
                offset,
                length,
            } => {
                for segment in segments {
                    self.send_bytes(Bytes::from(segment.prefix.clone())).await?;
                    source_bounds(length, segment)?;
                    self.send_file(
                        &mut file,
                        offset.saturating_add(segment.start),
                        segment.end - segment.start,
                    )
                    .await?;
                }
            }
            ResponseBody::FileWithPrefix {
                prefix,
                mut file,
                offset,
                length,
            } => {
                let prefix_len = prefix.len() as u64;
                let total = prefix_len.saturating_add(length);
                for segment in segments {
                    self.send_bytes(Bytes::from(segment.prefix.clone())).await?;
                    let (start, end) = source_bounds(total, segment)?;
                    let memory_end = end.min(prefix_len as usize);
                    if start < memory_end {
                        self.send_bytes(Bytes::copy_from_slice(&prefix[start..memory_end]))
                            .await?;
                    }
                    let file_start = (start as u64).max(prefix_len);
                    let file_end = end as u64;
                    if file_start < file_end {
                        self.send_file(
                            &mut file,
                            offset.saturating_add(file_start - prefix_len),
                            file_end - file_start,
                        )
                        .await?;
                    }
                }
            }
            ResponseBody::Segments { .. } | ResponseBody::Stream(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "nested response body segments",
                ));
            }
        }
        Ok(())
    }
}

async fn send_data(send: &mut H3SendStream, data: Bytes) -> io::Result<()> {
    if data.is_empty() {
        return Ok(());
    }
    send.send_data(data)
        .await
        .map_err(|error| io::Error::other(error.to_string()))
}

async fn send_trailers(send: &mut H3SendStream, trailers: &[Header]) -> io::Result<()> {
    let mut fields = http::HeaderMap::new();
    for trailer in trailers {
        if is_h2_connection_header(&trailer.name) || is_forbidden_trailer_name(&trailer.name) {
            continue;
        }
        let Some(name) = h2_header_name(&trailer.name) else {
            continue;
        };
        let Ok(value) = h2_header_value(&trailer.value) else {
            continue;
        };
        fields.append(name, value);
    }
    if fields.is_empty() {
        return Ok(());
    }
    send.send_trailers(fields)
        .await
        .map_err(|error| io::Error::other(error.to_string()))
}

fn h3_error_code(error: &io::Error) -> Code {
    match error.kind() {
        io::ErrorKind::BrokenPipe
        | io::ErrorKind::ConnectionAborted
        | io::ErrorKind::ConnectionReset => Code::H3_REQUEST_CANCELLED,
        _ => Code::H3_INTERNAL_ERROR,
    }
}

fn response_body_len(body: &ResponseBody) -> u64 {
    match body {
        ResponseBody::Empty => 0,
        ResponseBody::Bytes(body) => body.len() as u64,
        ResponseBody::File { length, .. } => *length,
        ResponseBody::FileWithPrefix { prefix, length, .. } => {
            (prefix.len() as u64).saturating_add(*length)
        }
        ResponseBody::Segments { segments, .. } => segments.iter().fold(0_u64, |total, segment| {
            total
                .saturating_add(segment.prefix.len() as u64)
                .saturating_add(segment.end.saturating_sub(segment.start))
        }),
        ResponseBody::Stream(stream) => stream.length().unwrap_or(0),
    }
}

fn source_bounds(length: u64, segment: &BodySegment) -> io::Result<(usize, usize)> {
    if segment.start > segment.end || segment.end > length {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "response body segment is out of bounds",
        ));
    }
    let start = usize::try_from(segment.start).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "response segment start is too large",
        )
    })?;
    let end = usize::try_from(segment.end).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "response segment end is too large",
        )
    })?;
    Ok((start, end))
}
