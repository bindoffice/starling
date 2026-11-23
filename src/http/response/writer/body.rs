use super::*;

pub(super) async fn write_body<W: AsyncWrite + Unpin>(
    writer: &mut W,
    rate: ResponseRate,
    body: &mut ResponseBody,
) -> io::Result<()> {
    let mut pacer = ResponsePacer::new(rate);
    match body {
        ResponseBody::Empty => {}
        ResponseBody::Bytes(body) => pacer.write_all(writer, body).await?,
        ResponseBody::File {
            file,
            offset,
            length,
        } => write_file_body(writer, &mut pacer, file, *offset, *length).await?,
        ResponseBody::FileWithPrefix {
            prefix,
            file,
            offset,
            length,
        } => {
            pacer.write_all(writer, prefix).await?;
            write_file_body(writer, &mut pacer, file, *offset, *length).await?;
        }
        ResponseBody::Segments { source, segments } => {
            write_segments(writer, &mut pacer, source, segments, false).await?;
        }
        ResponseBody::Stream(stream) => {
            let mut remaining = stream.length();
            while let Some(item) = stream.next().await? {
                let ResponseStreamItem::Data(data) = item else {
                    continue;
                };
                if let Some(remaining) = remaining.as_mut() {
                    if data.len() as u64 > *remaining {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "response body exceeds Content-Length",
                        ));
                    }
                    *remaining -= data.len() as u64;
                }
                pacer.write_all(writer, &data).await?;
                writer.flush().await?;
            }
            if remaining.is_some_and(|remaining| remaining != 0) {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "response body ended before Content-Length",
                ));
            }
        }
    }
    Ok(())
}

pub(super) async fn write_chunked_body<W: AsyncWrite + Unpin>(
    writer: &mut W,
    rate: ResponseRate,
    body: &mut ResponseBody,
) -> io::Result<Vec<Header>> {
    let mut pacer = ResponsePacer::new(rate);
    match body {
        ResponseBody::Empty => {}
        ResponseBody::Bytes(body) => write_chunk(writer, &mut pacer, body).await?,
        ResponseBody::File {
            file,
            offset,
            length,
        } => write_chunked_file(writer, &mut pacer, file, *offset, *length).await?,
        ResponseBody::FileWithPrefix {
            prefix,
            file,
            offset,
            length,
        } => {
            write_chunk(writer, &mut pacer, prefix).await?;
            write_chunked_file(writer, &mut pacer, file, *offset, *length).await?;
        }
        ResponseBody::Segments { source, segments } => {
            write_segments(writer, &mut pacer, source, segments, true).await?;
        }
        ResponseBody::Stream(stream) => {
            let mut trailers = Vec::new();
            while let Some(item) = stream.next().await? {
                match item {
                    ResponseStreamItem::Data(data) => {
                        write_chunk(writer, &mut pacer, &data).await?;
                        writer.flush().await?;
                    }
                    ResponseStreamItem::Trailers(stream_trailers) => {
                        trailers = stream_trailers;
                    }
                }
            }
            return Ok(trailers);
        }
    }
    Ok(Vec::new())
}

async fn write_segments<W: AsyncWrite + Unpin>(
    writer: &mut W,
    pacer: &mut ResponsePacer,
    source: &mut ResponseBody,
    segments: &[BodySegment],
    chunked: bool,
) -> io::Result<()> {
    for segment in segments {
        write_piece(writer, pacer, &segment.prefix, chunked).await?;
        write_source_range(writer, pacer, source, segment.start, segment.end, chunked).await?;
    }
    Ok(())
}

async fn write_source_range<W: AsyncWrite + Unpin>(
    writer: &mut W,
    pacer: &mut ResponsePacer,
    source: &mut ResponseBody,
    start: u64,
    end: u64,
    chunked: bool,
) -> io::Result<()> {
    match source {
        ResponseBody::Empty => {
            if start == 0 && end == 0 {
                return Ok(());
            }
        }
        ResponseBody::Bytes(body) => {
            let (start, end) = source_bounds(body.len() as u64, start, end)?;
            let start = usize::try_from(start).map_err(|_| {
                io::Error::new(io::ErrorKind::InvalidData, "response segment is too large")
            })?;
            let end = usize::try_from(end).map_err(|_| {
                io::Error::new(io::ErrorKind::InvalidData, "response segment is too large")
            })?;
            return write_piece(writer, pacer, &body[start..end], chunked).await;
        }
        ResponseBody::File {
            file,
            offset,
            length,
        } => {
            source_bounds(*length, start, end)?;
            return write_file_range(
                writer,
                pacer,
                file,
                offset.saturating_add(start),
                end - start,
                chunked,
            )
            .await;
        }
        ResponseBody::FileWithPrefix {
            prefix,
            file,
            offset,
            length,
        } => {
            let prefix_len = prefix.len() as u64;
            let total = prefix_len.saturating_add(*length);
            let (start, end) = source_bounds(total, start, end)?;
            let memory_end = end.min(prefix_len);
            if start < memory_end {
                write_piece(
                    writer,
                    pacer,
                    &prefix[start as usize..memory_end as usize],
                    chunked,
                )
                .await?;
            }
            let file_start = start.max(prefix_len);
            if file_start < end {
                write_file_range(
                    writer,
                    pacer,
                    file,
                    offset.saturating_add(file_start - prefix_len),
                    end - file_start,
                    chunked,
                )
                .await?;
            }
            return Ok(());
        }
        ResponseBody::Segments { .. } | ResponseBody::Stream(_) => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "nested response body segments",
            ));
        }
    }
    Err(io::Error::new(
        io::ErrorKind::InvalidData,
        "response body segment is out of bounds",
    ))
}

fn source_bounds(length: u64, start: u64, end: u64) -> io::Result<(u64, u64)> {
    if start > end || end > length {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "response body segment is out of bounds",
        ));
    }
    Ok((start, end))
}

async fn write_piece<W: AsyncWrite + Unpin>(
    writer: &mut W,
    pacer: &mut ResponsePacer,
    data: &[u8],
    chunked: bool,
) -> io::Result<()> {
    if chunked {
        write_chunk(writer, pacer, data).await
    } else {
        pacer.write_all(writer, data).await
    }
}

async fn write_file_range<W: AsyncWrite + Unpin>(
    writer: &mut W,
    pacer: &mut ResponsePacer,
    file: &mut File,
    offset: u64,
    length: u64,
    chunked: bool,
) -> io::Result<()> {
    file.seek(SeekFrom::Start(offset)).await?;
    let mut remaining = length;
    let mut buffer = vec![0_u8; 64 * 1024];
    while remaining > 0 {
        let chunk_len = remaining.min(buffer.len() as u64) as usize;
        let read = file.read(&mut buffer[..chunk_len]).await?;
        if read == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "response body ended before the requested range",
            ));
        }
        write_piece(writer, pacer, &buffer[..read], chunked).await?;
        remaining -= read as u64;
    }
    Ok(())
}

async fn write_chunk<W: AsyncWrite + Unpin>(
    writer: &mut W,
    pacer: &mut ResponsePacer,
    data: &[u8],
) -> io::Result<()> {
    if data.is_empty() {
        return Ok(());
    }
    writer
        .write_all(format!("{:x}\r\n", data.len()).as_bytes())
        .await?;
    pacer.write_all(writer, data).await?;
    writer.write_all(b"\r\n").await
}

async fn write_chunked_file<W: AsyncWrite + Unpin>(
    writer: &mut W,
    pacer: &mut ResponsePacer,
    file: &mut File,
    offset: u64,
    length: u64,
) -> io::Result<()> {
    file.seek(SeekFrom::Start(offset)).await?;
    let mut remaining = length;
    let mut buffer = vec![0_u8; 64 * 1024];
    while remaining > 0 {
        let chunk_len = remaining.min(buffer.len() as u64) as usize;
        let read = file.read(&mut buffer[..chunk_len]).await?;
        if read == 0 {
            break;
        }
        write_chunk(writer, pacer, &buffer[..read]).await?;
        remaining -= read as u64;
    }
    Ok(())
}

pub(super) async fn write_chunked_trailers<W: AsyncWrite + Unpin>(
    writer: &mut W,
    trailers: &[Header],
) -> io::Result<()> {
    writer.write_all(b"0\r\n").await?;
    for trailer in trailers {
        if is_forbidden_trailer_name(&trailer.name) {
            continue;
        }
        writer.write_all(trailer.name.as_bytes()).await?;
        writer.write_all(b": ").await?;
        writer
            .write_all(sanitize_header_value(&trailer.value).as_bytes())
            .await?;
        writer.write_all(b"\r\n").await?;
    }
    writer.write_all(b"\r\n").await
}

async fn write_file_body<W: AsyncWrite + Unpin>(
    writer: &mut W,
    pacer: &mut ResponsePacer,
    file: &mut File,
    offset: u64,
    length: u64,
) -> io::Result<()> {
    file.seek(SeekFrom::Start(offset)).await?;
    let mut remaining = length;
    let mut buffer = vec![0_u8; 64 * 1024];
    while remaining > 0 {
        let chunk_len = remaining.min(buffer.len() as u64) as usize;
        let read = file.read(&mut buffer[..chunk_len]).await?;
        if read == 0 {
            break;
        }
        pacer.write_all(writer, &buffer[..read]).await?;
        remaining -= read as u64;
    }
    Ok(())
}

struct ResponsePacer {
    rate: u64,
    after: u64,
    body_bytes: u64,
    paced_bytes: u64,
    paced_started: Option<Instant>,
}

impl ResponsePacer {
    fn new(rate: ResponseRate) -> Self {
        Self {
            rate: rate.rate,
            after: rate.after,
            body_bytes: 0,
            paced_bytes: 0,
            paced_started: None,
        }
    }

    async fn write_all<W>(&mut self, writer: &mut W, mut data: &[u8]) -> io::Result<()>
    where
        W: AsyncWrite + Unpin,
    {
        if self.rate == 0 {
            return writer.write_all(data).await;
        }
        while !data.is_empty() {
            let free = self.after.saturating_sub(self.body_bytes);
            if free > 0 {
                let length = data.len().min(free.min(usize::MAX as u64) as usize);
                writer.write_all(&data[..length]).await?;
                self.body_bytes = self.body_bytes.saturating_add(length as u64);
                data = &data[length..];
                continue;
            }
            let paced_chunk = (self.rate / 10).clamp(1024, 16 * 1024) as usize;
            let length = data.len().min(paced_chunk);
            let started = self.paced_started.unwrap_or_else(|| {
                let now = Instant::now();
                self.paced_started = Some(now);
                now
            });
            let target = started
                + Duration::from_secs_f64(
                    (self.paced_bytes + length as u64) as f64 / self.rate as f64,
                );
            tokio::time::sleep_until(tokio::time::Instant::from_std(target)).await;
            writer.write_all(&data[..length]).await?;
            self.body_bytes = self.body_bytes.saturating_add(length as u64);
            self.paced_bytes = self.paced_bytes.saturating_add(length as u64);
            data = &data[length..];
        }
        Ok(())
    }
}
