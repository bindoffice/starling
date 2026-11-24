use super::*;
use crate::http::{BodySegment, ResponseStreamItem, is_forbidden_trailer_name};
use bytes::{Buf, BytesMut};

pub(super) async fn send_h2_body(
    send: &mut h2::SendStream<Bytes>,
    body: ResponseBody,
    has_trailers: bool,
    chunk_size: usize,
) -> io::Result<Vec<Header>> {
    if let ResponseBody::Stream(mut stream) = body {
        let preserve_on_client_abort = stream.preserve_on_client_abort();
        let mut remaining = stream.length();
        let mut trailers = Vec::new();
        loop {
            let item = match next_stream_item(send, &mut stream).await {
                Ok(item) => item,
                Err(error) => {
                    if preserve_on_client_abort {
                        stream.drain().await;
                    }
                    return Err(error);
                }
            };
            let Some(item) = item else {
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
                    if let Err(error) = h2_send_data(send, data, false, chunk_size.max(1)).await {
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
        let has_stream_trailers = !trailers.is_empty();
        h2_send_data(
            send,
            Bytes::new(),
            !has_trailers && !has_stream_trailers,
            chunk_size.max(1),
        )
        .await?;
        return Ok(trailers);
    }
    let length = response_body_len(&body);
    let mut sender = H2BodySender {
        send,
        remaining: length,
        has_trailers,
        chunk_size: chunk_size.max(1),
    };
    match body {
        ResponseBody::Empty => {}
        ResponseBody::Bytes(body) => {
            sender.send_bytes(Bytes::from(body)).await?;
        }
        ResponseBody::File {
            mut file,
            offset,
            length,
        } => {
            sender.send_file(&mut file, offset, length).await?;
        }
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

async fn next_stream_item(
    send: &mut h2::SendStream<Bytes>,
    stream: &mut ResponseStream,
) -> io::Result<Option<ResponseStreamItem>> {
    tokio::select! {
        item = stream.next() => item,
        reason = std::future::poll_fn(|cx| send.poll_reset(cx)) => {
            let reason = reason.map_err(io::Error::other)?;
            Err(io::Error::new(
                io::ErrorKind::ConnectionReset,
                format!("HTTP/2 client reset the response stream: {reason:?}"),
            ))
        }
    }
}

struct H2BodySender<'a> {
    send: &'a mut h2::SendStream<Bytes>,
    remaining: u64,
    has_trailers: bool,
    chunk_size: usize,
}

impl H2BodySender<'_> {
    async fn send_bytes(&mut self, data: Bytes) -> io::Result<()> {
        let length = data.len() as u64;
        if length > self.remaining {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "response body exceeds Content-Length",
            ));
        }
        self.remaining -= length;
        h2_send_data(
            self.send,
            data,
            self.remaining == 0 && !self.has_trailers,
            self.chunk_size,
        )
        .await
    }

    async fn send_file(
        &mut self,
        file: &mut tokio::fs::File,
        offset: u64,
        length: u64,
    ) -> io::Result<()> {
        self.send_file_range(file, offset, length).await
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
                Ok(())
            }
            ResponseBody::Bytes(source) => {
                for segment in segments {
                    self.send_bytes(Bytes::from(segment.prefix.clone())).await?;
                    let (start, end) = source_bounds(source.len() as u64, segment)?;
                    self.send_bytes(Bytes::copy_from_slice(&source[start..end]))
                        .await?;
                }
                Ok(())
            }
            ResponseBody::File {
                mut file,
                offset,
                length,
            } => {
                for segment in segments {
                    self.send_bytes(Bytes::from(segment.prefix.clone())).await?;
                    source_bounds(length, segment)?;
                    self.send_file_range(
                        &mut file,
                        offset.saturating_add(segment.start),
                        segment.end - segment.start,
                    )
                    .await?;
                }
                Ok(())
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
                        self.send_file_range(
                            &mut file,
                            offset.saturating_add(file_start - prefix_len),
                            file_end - file_start,
                        )
                        .await?;
                    }
                }
                Ok(())
            }
            ResponseBody::Segments { .. } | ResponseBody::Stream(_) => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "nested response body segments",
            )),
        }
    }

    async fn send_file_range(
        &mut self,
        file: &mut tokio::fs::File,
        offset: u64,
        length: u64,
    ) -> io::Result<()> {
        use tokio::io::{AsyncSeekExt, SeekFrom};
        if let Err(err) = file.seek(SeekFrom::Start(offset)).await {
            self.send.send_reset(h2::Reason::INTERNAL_ERROR);
            return Err(err);
        }
        if length > self.remaining {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "response body exceeds Content-Length",
            ));
        }
        let mut file_remaining = length;
        let mut buffer = vec![0_u8; self.chunk_size];
        let mut pending = BytesMut::with_capacity(self.chunk_size);
        while file_remaining > 0 || !pending.is_empty() {
            while file_remaining > 0 && pending.len() < self.chunk_size {
                let chunk_len =
                    file_remaining.min((self.chunk_size - pending.len()) as u64) as usize;
                let read = match file.read(&mut buffer[..chunk_len]).await {
                    Ok(0) => {
                        self.send.send_reset(h2::Reason::INTERNAL_ERROR);
                        return Err(io::Error::new(
                            io::ErrorKind::UnexpectedEof,
                            "static file ended before Content-Length",
                        ));
                    }
                    Ok(read) => read,
                    Err(err) => {
                        self.send.send_reset(h2::Reason::INTERNAL_ERROR);
                        return Err(err);
                    }
                };
                file_remaining -= read as u64;
                self.remaining -= read as u64;
                pending.extend_from_slice(&buffer[..read]);
            }

            if pending.is_empty() {
                break;
            }
            let end_of_stream = self.remaining == 0 && !self.has_trailers;
            match h2_send_data_available(self.send, &pending, end_of_stream, self.chunk_size) {
                Ok(0) => {}
                Ok(sent) => {
                    pending.advance(sent);
                    continue;
                }
                Err(err) => return Err(err),
            }

            self.send.reserve_capacity(pending.len());
            match std::future::poll_fn(|cx| self.send.poll_capacity(cx)).await {
                Some(Ok(_)) => {}
                Some(Err(err)) => return Err(io::Error::other(err)),
                None => {
                    return Err(io::Error::new(
                        io::ErrorKind::BrokenPipe,
                        "HTTP/2 stream closed",
                    ));
                }
            }
        }
        Ok(())
    }
}

fn h2_send_data_available(
    send: &mut h2::SendStream<Bytes>,
    data: &[u8],
    end_of_stream: bool,
    chunk_size: usize,
) -> io::Result<usize> {
    if data.is_empty() {
        if end_of_stream {
            send.send_data(Bytes::new(), true)
                .map_err(io::Error::other)?;
        }
        return Ok(0);
    }
    let mut offset = 0;
    while offset < data.len() {
        let remaining = data.len() - offset;
        send.reserve_capacity(remaining);
        let capacity = send.capacity();
        if capacity == 0 {
            break;
        }
        let end = offset + remaining.min(capacity).min(chunk_size.max(1));
        let last = end == data.len();
        send.send_data(
            Bytes::copy_from_slice(&data[offset..end]),
            last && end_of_stream,
        )
        .map_err(io::Error::other)?;
        offset = end;
    }
    Ok(offset)
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

pub(super) fn send_h2_trailers(
    send: &mut h2::SendStream<Bytes>,
    trailers: &[Header],
) -> io::Result<()> {
    let mut fields = http::HeaderMap::new();
    for trailer in trailers {
        if super::is_h2_connection_header(&trailer.name) || is_forbidden_trailer_name(&trailer.name)
        {
            continue;
        }
        let Ok(name) = http::header::HeaderName::from_bytes(trailer.name.as_bytes()) else {
            continue;
        };
        let Ok(value) = super::send::h2_header_value(&trailer.value) else {
            continue;
        };
        fields.append(name, value);
    }
    if fields.is_empty() {
        send.send_data(Bytes::new(), true).map_err(io::Error::other)
    } else {
        send.send_trailers(fields).map_err(io::Error::other)
    }
}

async fn h2_send_data(
    send: &mut h2::SendStream<Bytes>,
    data: Bytes,
    end_of_stream: bool,
    chunk_size: usize,
) -> io::Result<()> {
    if data.is_empty() {
        if end_of_stream {
            send.send_data(data, true).map_err(io::Error::other)?;
        }
        return Ok(());
    }
    let mut offset = 0;
    while offset < data.len() {
        let remaining = data.len() - offset;
        send.reserve_capacity(remaining);
        let capacity = if send.capacity() > 0 {
            send.capacity()
        } else {
            match std::future::poll_fn(|cx| send.poll_capacity(cx)).await {
                Some(Ok(capacity)) => capacity,
                Some(Err(err)) => return Err(io::Error::other(err)),
                None => {
                    return Err(io::Error::new(
                        io::ErrorKind::BrokenPipe,
                        "HTTP/2 stream closed",
                    ));
                }
            }
        };
        let end = offset + remaining.min(capacity).min(chunk_size);
        let last = end == data.len();
        send.send_data(data.slice(offset..end), last && end_of_stream)
            .map_err(io::Error::other)?;
        offset = end;
    }
    Ok(())
}
