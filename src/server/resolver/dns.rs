use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::atomic::{AtomicU16, Ordering};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpStream, UdpSocket};

pub(super) const TYPE_A: u16 = 1;
pub(super) const TYPE_AAAA: u16 = 28;

const CLASS_IN: u16 = 1;
const MAX_DNS_MESSAGE: usize = 65_535;

#[derive(Debug)]
pub(super) struct Lookup {
    pub(super) addresses: Vec<IpAddr>,
    pub(super) ttl: Option<u32>,
}

pub(super) async fn query(
    server: SocketAddr,
    host: &str,
    query_type: u16,
    timeout: Duration,
) -> io::Result<Lookup> {
    let id = next_id();
    let request = build_query(id, host, query_type)?;
    let response = match tokio::time::timeout(timeout, udp_query(server, &request)).await {
        Ok(result) => result?,
        Err(_) => {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!("DNS query to {server} timed out"),
            ));
        }
    };
    let parsed = parse_response(&response, id)?;
    if !parsed.truncated {
        return Ok(parsed.lookup);
    }

    let response = tokio::time::timeout(timeout, tcp_query(server, &request))
        .await
        .map_err(|_| {
            io::Error::new(
                io::ErrorKind::TimedOut,
                format!("DNS TCP query to {server} timed out"),
            )
        })??;
    Ok(parse_response(&response, id)?.lookup)
}

async fn udp_query(server: SocketAddr, request: &[u8]) -> io::Result<Vec<u8>> {
    let bind = match server {
        SocketAddr::V4(_) => SocketAddr::from(([0, 0, 0, 0], 0)),
        SocketAddr::V6(_) => SocketAddr::from(([0u16; 8], 0)),
    };
    let socket = UdpSocket::bind(bind).await?;
    socket.connect(server).await?;
    socket.send(request).await?;
    let mut response = vec![0; MAX_DNS_MESSAGE];
    let length = socket.recv(&mut response).await?;
    response.truncate(length);
    Ok(response)
}

async fn tcp_query(server: SocketAddr, request: &[u8]) -> io::Result<Vec<u8>> {
    let mut stream = TcpStream::connect(server).await?;
    stream
        .write_all(&(request.len() as u16).to_be_bytes())
        .await?;
    stream.write_all(request).await?;
    let length = stream.read_u16().await? as usize;
    let mut response = vec![0; length];
    stream.read_exact(&mut response).await?;
    Ok(response)
}

fn next_id() -> u16 {
    static NEXT_ID: AtomicU16 = AtomicU16::new(0x5341);
    NEXT_ID.fetch_add(1, Ordering::Relaxed)
}

fn build_query(id: u16, host: &str, query_type: u16) -> io::Result<Vec<u8>> {
    let mut packet = Vec::with_capacity(512);
    packet.extend_from_slice(&id.to_be_bytes());
    packet.extend_from_slice(&0x0100_u16.to_be_bytes());
    packet.extend_from_slice(&1_u16.to_be_bytes());
    packet.extend_from_slice(&[0; 6]);
    encode_name(host, &mut packet)?;
    packet.extend_from_slice(&query_type.to_be_bytes());
    packet.extend_from_slice(&CLASS_IN.to_be_bytes());
    Ok(packet)
}

fn encode_name(host: &str, packet: &mut Vec<u8>) -> io::Result<()> {
    let mut length = 1;
    for label in host.trim_end_matches('.').split('.') {
        if label.is_empty() || label.len() > 63 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("invalid DNS name \"{host}\""),
            ));
        }
        length += label.len() + 1;
        if length > 255 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("DNS name too long \"{host}\""),
            ));
        }
        packet.push(label.len() as u8);
        packet.extend_from_slice(label.as_bytes());
    }
    packet.push(0);
    Ok(())
}

struct ParsedResponse {
    lookup: Lookup,
    truncated: bool,
}

fn parse_response(packet: &[u8], expected_id: u16) -> io::Result<ParsedResponse> {
    if packet.len() < 12 {
        return Err(invalid_data("short DNS response"));
    }
    let id = u16::from_be_bytes([packet[0], packet[1]]);
    if id != expected_id {
        return Err(invalid_data("DNS response ID mismatch"));
    }
    let flags = u16::from_be_bytes([packet[2], packet[3]]);
    let truncated = flags & 0x0200 != 0;
    let rcode = flags & 0x000f;
    if rcode != 0 && !truncated {
        return Err(match rcode {
            3 => io::Error::new(io::ErrorKind::NotFound, "DNS name does not exist"),
            _ => invalid_data(format!("DNS response error code {rcode}")),
        });
    }

    let questions = u16::from_be_bytes([packet[4], packet[5]]) as usize;
    let answers = u16::from_be_bytes([packet[6], packet[7]]) as usize;
    let mut position = 12;
    for _ in 0..questions {
        skip_name(packet, &mut position)?;
        position = position
            .checked_add(4)
            .filter(|position| *position <= packet.len())
            .ok_or_else(|| invalid_data("truncated DNS question"))?;
    }

    let mut addresses = Vec::new();
    let mut ttl = None;
    for _ in 0..answers {
        skip_name(packet, &mut position)?;
        let record = packet
            .get(position..position + 10)
            .ok_or_else(|| invalid_data("truncated DNS answer"))?;
        let record_type = u16::from_be_bytes([record[0], record[1]]);
        let record_class = u16::from_be_bytes([record[2], record[3]]);
        let record_ttl = u32::from_be_bytes([record[4], record[5], record[6], record[7]]);
        let length = u16::from_be_bytes([record[8], record[9]]) as usize;
        position += 10;
        let data = packet
            .get(position..position + length)
            .ok_or_else(|| invalid_data("truncated DNS record data"))?;
        position += length;
        if record_class != CLASS_IN {
            continue;
        }
        let address = match (record_type, length) {
            (TYPE_A, 4) => Some(IpAddr::V4(Ipv4Addr::new(
                data[0], data[1], data[2], data[3],
            ))),
            (TYPE_AAAA, 16) => {
                let octets: [u8; 16] = data.try_into().expect("AAAA record length");
                Some(IpAddr::V6(Ipv6Addr::from(octets)))
            }
            _ => None,
        };
        if let Some(address) = address {
            addresses.push(address);
            ttl = Some(ttl.map_or(record_ttl, |current: u32| current.min(record_ttl)));
        }
    }

    Ok(ParsedResponse {
        lookup: Lookup { addresses, ttl },
        truncated,
    })
}

fn skip_name(packet: &[u8], position: &mut usize) -> io::Result<()> {
    loop {
        let length = *packet
            .get(*position)
            .ok_or_else(|| invalid_data("truncated DNS name"))?;
        *position += 1;
        if length == 0 {
            return Ok(());
        }
        if length & 0xc0 == 0xc0 {
            if *position >= packet.len() {
                return Err(invalid_data("truncated DNS compression pointer"));
            }
            *position += 1;
            return Ok(());
        }
        if length & 0xc0 != 0 {
            return Err(invalid_data("invalid DNS label"));
        }
        *position = position
            .checked_add(length as usize)
            .filter(|position| *position <= packet.len())
            .ok_or_else(|| invalid_data("truncated DNS label"))?;
    }
}

fn invalid_data(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_and_parses_a_response() {
        let query = build_query(7, "ocsp.example.test", TYPE_A).unwrap();
        assert_eq!(&query[..2], &7_u16.to_be_bytes());
        let mut response = query[..2].to_vec();
        response.extend_from_slice(&0x8180_u16.to_be_bytes());
        response.extend_from_slice(&1_u16.to_be_bytes());
        response.extend_from_slice(&1_u16.to_be_bytes());
        response.extend_from_slice(&[0; 4]);
        response.extend_from_slice(&query[12..]);
        response.extend_from_slice(&[0xc0, 0x0c, 0, TYPE_A as u8, 0, 1]);
        response.extend_from_slice(&60_u32.to_be_bytes());
        response.extend_from_slice(&4_u16.to_be_bytes());
        response.extend_from_slice(&[192, 0, 2, 10]);

        let parsed = parse_response(&response, 7).unwrap();
        assert_eq!(
            parsed.lookup.addresses,
            vec![IpAddr::V4(Ipv4Addr::new(192, 0, 2, 10))]
        );
        assert_eq!(parsed.lookup.ttl, Some(60));
    }
}
