use super::*;

pub(in crate::server) const FCGI_BEGIN_REQUEST: u8 = 1;
pub(in crate::server) const FCGI_END_REQUEST: u8 = 3;
pub(in crate::server) const FCGI_PARAMS: u8 = 4;
pub(in crate::server) const FCGI_STDIN: u8 = 5;
pub(in crate::server) const FCGI_STDOUT: u8 = 6;
pub(in crate::server) const FCGI_STDERR: u8 = 7;

pub(in crate::server) struct FastCgiRecord {
    pub(in crate::server) record_type: u8,
    pub(in crate::server) content: Vec<u8>,
}

pub(in crate::server) async fn read_fastcgi_record<R: AsyncRead + Unpin>(
    reader: &mut R,
) -> io::Result<FastCgiRecord> {
    let mut header = [0_u8; 8];
    reader.read_exact(&mut header).await?;
    if header[0] != 1 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid FastCGI version",
        ));
    }
    let request_id = u16::from_be_bytes([header[2], header[3]]);
    if request_id != 1 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "unexpected FastCGI request id",
        ));
    }
    let content_length = u16::from_be_bytes([header[4], header[5]]) as usize;
    let padding = header[6] as usize;
    let mut content = vec![0_u8; content_length];
    reader.read_exact(&mut content).await?;
    if padding > 0 {
        let mut padding = vec![0_u8; padding];
        reader.read_exact(&mut padding).await?;
    }
    Ok(FastCgiRecord {
        record_type: header[1],
        content,
    })
}

pub(in crate::server) async fn read_fastcgi_record_with_timeout<R: AsyncRead + Unpin>(
    reader: &mut R,
    timeout: Duration,
) -> io::Result<FastCgiRecord> {
    // Nginx applies read_timeout to the absence of socket progress, not to
    // the total wall-clock time needed to receive one FastCGI record.
    let mut reader = TimedReader::new(reader, timeout);
    let record = read_fastcgi_record(&mut reader).await?;
    if !matches!(
        record.record_type,
        FCGI_STDOUT | FCGI_STDERR | FCGI_END_REQUEST
    ) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("unexpected FastCGI record type {}", record.record_type),
        ));
    }
    Ok(record)
}

#[cfg(test)]
pub(in crate::server) async fn read_fastcgi_response_with_policy<R: AsyncRead + Unpin>(
    reader: &mut R,
    catch_stderr: &[String],
    limit_rate: u64,
    max_header_bytes: usize,
    buffer_policy: UpstreamBodyBufferPolicy,
) -> io::Result<(Response, bool)> {
    let mut decoder = CgiResponseDecoder::with_policy(max_header_bytes, buffer_policy);
    let mut read_rate = (limit_rate != 0).then(|| ReadRate::new(limit_rate));
    loop {
        let record = read_fastcgi_record(reader).await?;
        match record.record_type {
            FCGI_STDERR => {
                if !record.content.is_empty()
                    && catch_stderr.iter().any(|pattern| {
                        String::from_utf8_lossy(&record.content).contains(pattern.as_str())
                    })
                {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "FastCGI stderr matched fastcgi_catch_stderr",
                    ));
                }
            }
            FCGI_STDOUT => {
                if let Some(rate) = read_rate.as_mut() {
                    rate.account(record.content.len() as u64).await;
                }
                decoder.push(&record.content).await?;
            }
            FCGI_END_REQUEST => {
                return Ok((decoder.finish().await?, true));
            }
            _ => {}
        }
    }
}
