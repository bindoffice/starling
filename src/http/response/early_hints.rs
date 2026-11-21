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
        let mut head = String::from("HTTP/1.1 103 Early Hints\r\n");
        for header in headers {
            head.push_str(&header.name);
            head.push_str(": ");
            head.push_str(&sanitize_header_value(&header.value));
            head.push_str("\r\n");
        }
        head.push_str("\r\n");
        writer.write_all(head.as_bytes()).await?;
        writer.flush().await?;
    }
    Ok(())
}
