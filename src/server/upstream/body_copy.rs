use super::*;
use std::io::SeekFrom;
use tokio::io::{AsyncSeekExt, AsyncWriteExt};

pub(in crate::server) async fn copy_response_body(
    body: &ResponseBody,
    output: &mut tokio::fs::File,
) -> io::Result<()> {
    match body {
        ResponseBody::Empty => Ok(()),
        ResponseBody::Bytes(body) => output.write_all(body).await,
        ResponseBody::File {
            file,
            offset,
            length,
        } => copy_file_range(file, *offset, *length, output).await,
        ResponseBody::FileWithPrefix {
            prefix,
            file,
            offset,
            length,
        } => {
            output.write_all(prefix).await?;
            copy_file_range(file, *offset, *length, output).await
        }
        ResponseBody::Segments { source, segments } => {
            for segment in segments {
                output.write_all(&segment.prefix).await?;
                copy_body_range(source, segment.start, segment.end, output).await?;
            }
            Ok(())
        }
        ResponseBody::Stream(_) => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "streaming response bodies cannot be copied to storage",
        )),
    }
}

async fn copy_body_range(
    body: &ResponseBody,
    start: u64,
    end: u64,
    output: &mut tokio::fs::File,
) -> io::Result<()> {
    if start > end {
        return Err(out_of_bounds());
    }
    match body {
        ResponseBody::Empty => {
            if start == 0 && end == 0 {
                Ok(())
            } else {
                Err(out_of_bounds())
            }
        }
        ResponseBody::Bytes(body) => {
            let (start, end) = checked_bounds(body.len() as u64, start, end)?;
            let start = usize::try_from(start).map_err(|_| out_of_bounds())?;
            let end = usize::try_from(end).map_err(|_| out_of_bounds())?;
            output.write_all(&body[start..end]).await
        }
        ResponseBody::File {
            file,
            offset,
            length,
        } => {
            checked_bounds(*length, start, end)?;
            copy_file_range(file, offset.saturating_add(start), end - start, output).await
        }
        ResponseBody::FileWithPrefix {
            prefix,
            file,
            offset,
            length,
        } => {
            let prefix_len = prefix.len() as u64;
            let total = prefix_len.saturating_add(*length);
            let (start, end) = checked_bounds(total, start, end)?;
            let memory_end = end.min(prefix_len);
            if start < memory_end {
                let start = usize::try_from(start).map_err(|_| out_of_bounds())?;
                let memory_end = usize::try_from(memory_end).map_err(|_| out_of_bounds())?;
                output.write_all(&prefix[start..memory_end]).await?;
            }
            let file_start = start.max(prefix_len);
            if file_start < end {
                copy_file_range(
                    file,
                    offset.saturating_add(file_start - prefix_len),
                    end - file_start,
                    output,
                )
                .await?;
            }
            Ok(())
        }
        ResponseBody::Segments { .. } | ResponseBody::Stream(_) => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "nested response body segments",
        )),
    }
}

async fn copy_file_range(
    file: &tokio::fs::File,
    offset: u64,
    length: u64,
    output: &mut tokio::fs::File,
) -> io::Result<()> {
    let mut source = file.try_clone().await?;
    source.seek(SeekFrom::Start(offset)).await?;
    let mut limited = (&mut source).take(length);
    let copied = tokio::io::copy(&mut limited, output).await?;
    if copied != length {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "response body ended before the requested range",
        ));
    }
    Ok(())
}

fn checked_bounds(length: u64, start: u64, end: u64) -> io::Result<(u64, u64)> {
    if start > end || end > length {
        return Err(out_of_bounds());
    }
    Ok((start, end))
}

fn out_of_bounds() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "response body segment is out of bounds",
    )
}
