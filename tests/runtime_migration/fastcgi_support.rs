use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::net::TcpStream;
use std::time::Duration;

pub(crate) const FCGI_END_REQUEST: u8 = 3;
pub(crate) const FCGI_PARAMS: u8 = 4;
pub(crate) const FCGI_STDIN: u8 = 5;
pub(crate) const FCGI_STDOUT: u8 = 6;
const FCGI_BEGIN_REQUEST: u8 = 1;

pub(crate) fn configure_stream(stream: &TcpStream) -> io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))
}

pub(crate) fn read_fastcgi_request(
    stream: &mut TcpStream,
) -> io::Result<(u16, HashMap<String, String>)> {
    let mut request_id = 1_u16;
    let mut encoded_params = Vec::new();
    loop {
        let record = read_record(stream)?;
        if record.request_id != 0 {
            request_id = record.request_id;
        }
        match record.kind {
            FCGI_PARAMS if !record.content.is_empty() => encoded_params.extend(record.content),
            FCGI_STDIN if record.content.is_empty() => break,
            FCGI_BEGIN_REQUEST | FCGI_PARAMS | FCGI_STDIN => {}
            _ => {}
        }
    }
    let params = parse_params(&encoded_params)?;
    Ok((request_id, params))
}

pub(crate) fn write_fastcgi_response(
    stream: &mut TcpStream,
    request_id: u16,
    headers: &str,
    body: &str,
) -> io::Result<()> {
    let mut content = String::from("Status: 200 OK\r\nContent-Type: text/plain\r\n");
    if !headers.is_empty() {
        content.push_str(headers);
        content.push_str("\r\n");
    }
    content.push_str(&format!("Content-Length: {}\r\n\r\n{body}", body.len()));
    write_record(stream, FCGI_STDOUT, request_id, content.as_bytes())?;
    write_record(stream, FCGI_END_REQUEST, request_id, &[0; 8])
}

fn write_record(
    stream: &mut TcpStream,
    kind: u8,
    request_id: u16,
    content: &[u8],
) -> io::Result<()> {
    let length = u16::try_from(content.len()).expect("FastCGI test record fits u16");
    let mut header = [0_u8; 8];
    header[0] = 1;
    header[1] = kind;
    header[2..4].copy_from_slice(&request_id.to_be_bytes());
    header[4..6].copy_from_slice(&length.to_be_bytes());
    stream.write_all(&header)?;
    stream.write_all(content)
}

fn parse_params(data: &[u8]) -> io::Result<HashMap<String, String>> {
    let mut params = HashMap::new();
    let mut position = 0;
    while position < data.len() {
        let name_length = read_length(data, &mut position)?;
        let value_length = read_length(data, &mut position)?;
        let name_end = position
            .checked_add(name_length)
            .filter(|end| *end <= data.len())
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "invalid FastCGI name"))?;
        let name = String::from_utf8(data[position..name_end].to_vec())
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        position = name_end;
        let value_end = position
            .checked_add(value_length)
            .filter(|end| *end <= data.len())
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "invalid FastCGI value"))?;
        let value = String::from_utf8(data[position..value_end].to_vec())
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        position = value_end;
        params.insert(name, value);
    }
    Ok(params)
}

fn read_length(data: &[u8], position: &mut usize) -> io::Result<usize> {
    let first = *data
        .get(*position)
        .ok_or_else(|| io::Error::new(io::ErrorKind::UnexpectedEof, "missing FastCGI length"))?;
    if first & 0x80 == 0 {
        *position += 1;
        return Ok(first as usize);
    }
    let bytes = data
        .get(*position..*position + 4)
        .ok_or_else(|| io::Error::new(io::ErrorKind::UnexpectedEof, "short FastCGI length"))?;
    *position += 4;
    Ok(u32::from_be_bytes([bytes[0] & 0x7f, bytes[1], bytes[2], bytes[3]]) as usize)
}

struct Record {
    kind: u8,
    request_id: u16,
    content: Vec<u8>,
}

fn read_record(stream: &mut TcpStream) -> io::Result<Record> {
    let mut header = [0_u8; 8];
    stream.read_exact(&mut header)?;
    let length = u16::from_be_bytes([header[4], header[5]]) as usize;
    let padding = header[6] as usize;
    let mut content = vec![0_u8; length];
    stream.read_exact(&mut content)?;
    if padding > 0 {
        let mut padding = vec![0_u8; padding];
        stream.read_exact(&mut padding)?;
    }
    Ok(Record {
        kind: header[1],
        request_id: u16::from_be_bytes([header[2], header[3]]),
        content,
    })
}
