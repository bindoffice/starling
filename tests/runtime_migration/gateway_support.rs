use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::net::TcpStream;
use std::time::Duration;

#[derive(Clone, Copy)]
pub(crate) enum Gateway {
    Uwsgi,
    Scgi,
}

impl Gateway {
    pub(crate) fn directive(self) -> &'static str {
        match self {
            Self::Uwsgi => "uwsgi",
            Self::Scgi => "scgi",
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Uwsgi => "uWSGI",
            Self::Scgi => "SCGI",
        }
    }
}

pub(crate) fn configure_stream(stream: &TcpStream) -> io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))
}

pub(crate) fn read_gateway_params(
    gateway: Gateway,
    stream: &mut TcpStream,
) -> io::Result<HashMap<String, String>> {
    let encoded = match gateway {
        Gateway::Uwsgi => read_uwsgi_params(stream)?,
        Gateway::Scgi => read_scgi_params(stream)?,
    };
    parse_null_separated_params(&encoded)
}

pub(crate) fn write_gateway_response(
    stream: &mut TcpStream,
    headers: &str,
    body: &str,
) -> io::Result<()> {
    let mut response = String::from("Status: 200 OK\r\nContent-Type: text/plain\r\n");
    if !headers.is_empty() {
        response.push_str(headers);
        response.push_str("\r\n");
    }
    response.push_str(&format!("Content-Length: {}\r\n\r\n{body}", body.len()));
    stream.write_all(response.as_bytes())?;
    stream.flush()
}

fn read_uwsgi_params(stream: &mut TcpStream) -> io::Result<Vec<u8>> {
    let mut header = [0_u8; 4];
    stream.read_exact(&mut header)?;
    let length = u16::from_le_bytes([header[1], header[2]]) as usize;
    let mut params = vec![0_u8; length];
    stream.read_exact(&mut params)?;
    Ok(params)
}

fn read_scgi_params(stream: &mut TcpStream) -> io::Result<Vec<u8>> {
    let mut length = Vec::new();
    loop {
        let mut byte = [0_u8; 1];
        stream.read_exact(&mut byte)?;
        if byte[0] == b':' {
            break;
        }
        length.push(byte[0]);
    }
    let length = String::from_utf8(length)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?
        .parse::<usize>()
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    let mut params = vec![0_u8; length];
    stream.read_exact(&mut params)?;
    let mut comma = [0_u8; 1];
    stream.read_exact(&mut comma)?;
    if comma != [b','] {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "SCGI request does not end with a comma",
        ));
    }
    Ok(params)
}

fn parse_null_separated_params(data: &[u8]) -> io::Result<HashMap<String, String>> {
    let parts = data.split(|byte| *byte == 0).collect::<Vec<_>>();
    if parts.len() % 2 != 1 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "gateway parameters are not name/value pairs",
        ));
    }
    let mut params = HashMap::new();
    for index in (0..parts.len() - 1).step_by(2) {
        let name = String::from_utf8(parts[index].to_vec())
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        let value = String::from_utf8(parts[index + 1].to_vec())
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        params.insert(name, value);
    }
    Ok(params)
}
