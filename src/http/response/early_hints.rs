use super::*;

pub(crate) async fn write_early_hints<W: AsyncWrite + Unpin>(
    writer: &mut W,
    early_hints: &[Vec<Header>],
    request_version: &str,
) -> io::Result<()> {
    if request_version != "HTTP/1.1" {
        return Ok(());
    }
    for headers in early_hints {
        if headers.is_empty() {
            continue;
        }
        let mut head = b"HTTP/1.1 103 Early Hints\r\n".to_vec();
        for header in headers {
            head.extend_from_slice(header.name.as_bytes());
            head.extend_from_slice(b": ");
            head.extend(
                header_value_bytes(&header.value)
                    .iter()
                    .map(|byte| match byte {
                        b'\r' | b'\n' => b' ',
                        byte => *byte,
                    }),
            );
            head.extend_from_slice(b"\r\n");
        }
        head.extend_from_slice(b"\r\n");
        writer.write_all(&head).await?;
        writer.flush().await?;
    }
    Ok(())
}
