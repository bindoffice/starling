use super::{WasmError, WasmErrorKind, WasmRequest, WasmResponse};

pub(super) const ABI_VERSION: i32 = 1;
pub(super) const MAX_WASM_HEADER_BYTES: usize = 64 * 1024;
pub(super) const MAX_WASM_RESPONSE_OVERHEAD: usize =
    8 + 2 + 2 + 2 + 4 + MAX_WASM_HEADERS * 6 + MAX_WASM_HEADER_BYTES;

const REQUEST_MAGIC: &[u8; 8] = b"STWLRQ01";
const RESPONSE_MAGIC: &[u8; 8] = b"STWLRS01";
const MAX_WASM_HEADERS: usize = 256;

pub(super) fn encode_request(request: &WasmRequest) -> Result<Vec<u8>, WasmError> {
    let mut output = Vec::with_capacity(
        request.body.len()
            + request
                .headers
                .iter()
                .map(|(name, value)| name.len() + value.len() + 8)
                .sum::<usize>()
            + 256,
    );
    output.extend_from_slice(REQUEST_MAGIC);
    put_u16(&mut output, ABI_VERSION as u16);
    put_string16(&mut output, &request.method)?;
    put_string32(&mut output, &request.target)?;
    put_string32(&mut output, &request.uri)?;
    put_string32(&mut output, &request.args)?;
    put_string16(&mut output, &request.version)?;
    put_string16(&mut output, &request.scheme)?;
    put_string32(&mut output, &request.host)?;
    put_string32(&mut output, &request.remote_addr)?;
    put_string32(&mut output, &request.local_addr)?;
    let header_count = u16::try_from(request.headers.len())
        .map_err(|_| WasmError::new(WasmErrorKind::Abi, "too many request headers for WASM ABI"))?;
    put_u16(&mut output, header_count);
    for (name, value) in &request.headers {
        put_string16(&mut output, name)?;
        put_string32(&mut output, value)?;
    }
    let body_len = u32::try_from(request.body.len()).map_err(|_| {
        WasmError::new(WasmErrorKind::Abi, "request body is too large for WASM ABI")
    })?;
    put_u32(&mut output, body_len);
    output.extend_from_slice(&request.body);
    Ok(output)
}

pub(super) fn decode_response(bytes: &[u8], body_limit: u64) -> Result<WasmResponse, WasmError> {
    let mut reader = AbiReader::new(bytes);
    if reader.take(8)? != RESPONSE_MAGIC {
        return Err(WasmError::new(
            WasmErrorKind::Abi,
            "WASM handler returned an invalid response magic",
        ));
    }
    let version = reader.u16()?;
    if version != ABI_VERSION as u16 {
        return Err(WasmError::new(
            WasmErrorKind::Abi,
            format!("unsupported WASM response ABI version {version}"),
        ));
    }
    let status = reader.u16()?;
    if !(200..=599).contains(&status) {
        return Err(WasmError::new(
            WasmErrorKind::Abi,
            format!("WASM handler returned invalid status {status}"),
        ));
    }
    let header_count = reader.u16()? as usize;
    if header_count > MAX_WASM_HEADERS {
        return Err(WasmError::new(
            WasmErrorKind::Abi,
            format!("WASM handler returned more than {MAX_WASM_HEADERS} headers"),
        ));
    }
    let mut headers = Vec::with_capacity(header_count);
    let mut header_bytes = 0_usize;
    for _ in 0..header_count {
        let name = reader.string16()?;
        let value = reader.string32()?;
        header_bytes = header_bytes
            .saturating_add(name.len())
            .saturating_add(value.len());
        if header_bytes > MAX_WASM_HEADER_BYTES {
            return Err(WasmError::new(
                WasmErrorKind::Abi,
                format!("WASM response headers exceed the {MAX_WASM_HEADER_BYTES}-byte limit"),
            ));
        }
        validate_response_header(&name, &value)?;
        headers.push((name, value));
    }
    let body_len = reader.u32()? as usize;
    if body_len as u64 > body_limit {
        return Err(WasmError::new(
            WasmErrorKind::Abi,
            format!("WASM response body exceeds the {body_limit}-byte limit"),
        ));
    }
    let body = reader.take(body_len)?.to_vec();
    if !reader.is_empty() {
        return Err(WasmError::new(
            WasmErrorKind::Abi,
            "WASM handler returned trailing response bytes",
        ));
    }
    if matches!(status, 204 | 205 | 304) && !body.is_empty() {
        return Err(WasmError::new(
            WasmErrorKind::Abi,
            format!("WASM handler returned a body for status {status}"),
        ));
    }
    Ok(WasmResponse {
        status,
        headers,
        body,
    })
}

fn validate_response_header(name: &str, value: &str) -> Result<(), WasmError> {
    if name.is_empty() || !name.bytes().all(is_token_byte) {
        return Err(WasmError::new(
            WasmErrorKind::Abi,
            "WASM handler returned an invalid response header name",
        ));
    }
    if value.bytes().any(|byte| {
        byte == b'\r'
            || byte == b'\n'
            || byte == 0
            || byte == 0x7f
            || (byte < b' ' && byte != b'\t')
    }) {
        return Err(WasmError::new(
            WasmErrorKind::Abi,
            format!("WASM handler returned an invalid value for response header {name}"),
        ));
    }
    if matches!(
        name.to_ascii_lowercase().as_str(),
        "connection"
            | "content-length"
            | "keep-alive"
            | "proxy-connection"
            | "te"
            | "trailer"
            | "transfer-encoding"
            | "upgrade"
    ) {
        return Err(WasmError::new(
            WasmErrorKind::Abi,
            format!("WASM handler cannot set framing or hop-by-hop header {name}"),
        ));
    }
    Ok(())
}

fn is_token_byte(byte: u8) -> bool {
    matches!(
        byte,
        b'!' | b'#'
            | b'$'
            | b'%'
            | b'&'
            | b'\''
            | b'*'
            | b'+'
            | b'-'
            | b'.'
            | b'^'
            | b'_'
            | b'`'
            | b'|'
            | b'~'
            | b'0'..=b'9'
            | b'A'..=b'Z'
            | b'a'..=b'z'
    )
}

fn put_u16(output: &mut Vec<u8>, value: u16) {
    output.extend_from_slice(&value.to_le_bytes());
}

fn put_u32(output: &mut Vec<u8>, value: u32) {
    output.extend_from_slice(&value.to_le_bytes());
}

fn put_string16(output: &mut Vec<u8>, value: &str) -> Result<(), WasmError> {
    let len = u16::try_from(value.len()).map_err(|_| {
        WasmError::new(
            WasmErrorKind::Abi,
            "WASM request string exceeds 16-bit ABI limit",
        )
    })?;
    put_u16(output, len);
    output.extend_from_slice(value.as_bytes());
    Ok(())
}

fn put_string32(output: &mut Vec<u8>, value: &str) -> Result<(), WasmError> {
    let len = u32::try_from(value.len()).map_err(|_| {
        WasmError::new(
            WasmErrorKind::Abi,
            "WASM request string exceeds 32-bit ABI limit",
        )
    })?;
    put_u32(output, len);
    output.extend_from_slice(value.as_bytes());
    Ok(())
}

struct AbiReader<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl<'a> AbiReader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, position: 0 }
    }

    fn is_empty(&self) -> bool {
        self.position == self.bytes.len()
    }

    fn take(&mut self, len: usize) -> Result<&'a [u8], WasmError> {
        let end = self.position.checked_add(len).ok_or_else(|| {
            WasmError::new(WasmErrorKind::Abi, "WASM ABI buffer length overflowed")
        })?;
        let value = self.bytes.get(self.position..end).ok_or_else(|| {
            WasmError::new(WasmErrorKind::Abi, "WASM ABI response ended unexpectedly")
        })?;
        self.position = end;
        Ok(value)
    }

    fn u16(&mut self) -> Result<u16, WasmError> {
        let bytes = self.take(2)?;
        Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
    }

    fn u32(&mut self) -> Result<u32, WasmError> {
        let bytes = self.take(4)?;
        Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    fn string16(&mut self) -> Result<String, WasmError> {
        let length = self.u16()? as usize;
        String::from_utf8(self.take(length)?.to_vec())
            .map_err(|_| WasmError::new(WasmErrorKind::Abi, "WASM ABI string is not valid UTF-8"))
    }

    fn string32(&mut self) -> Result<String, WasmError> {
        let length = self.u32()? as usize;
        String::from_utf8(self.take(length)?.to_vec())
            .map_err(|_| WasmError::new(WasmErrorKind::Abi, "WASM ABI string is not valid UTF-8"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn response(status: u16, headers: &[(&str, &str)], body: &[u8]) -> Vec<u8> {
        let mut output = Vec::new();
        output.extend_from_slice(RESPONSE_MAGIC);
        put_u16(&mut output, ABI_VERSION as u16);
        put_u16(&mut output, status);
        put_u16(&mut output, headers.len() as u16);
        for (name, value) in headers {
            put_string16(&mut output, name).unwrap();
            put_string32(&mut output, value).unwrap();
        }
        put_u32(&mut output, body.len() as u32);
        output.extend_from_slice(body);
        output
    }

    #[test]
    fn request_encoder_contains_versioned_fields_and_body() {
        let request = WasmRequest {
            method: "POST".into(),
            target: "/hello?name=starling".into(),
            uri: "/hello".into(),
            args: "name=starling".into(),
            version: "HTTP/1.1".into(),
            scheme: "https".into(),
            host: "example.test".into(),
            remote_addr: "127.0.0.1:1234".into(),
            local_addr: "127.0.0.1:8443".into(),
            headers: vec![("content-type".into(), "text/plain".into())],
            body: b"payload".to_vec(),
        };
        let encoded = encode_request(&request).unwrap();
        assert_eq!(&encoded[..8], REQUEST_MAGIC);
        assert_eq!(u16::from_le_bytes([encoded[8], encoded[9]]), 1);
        assert!(encoded.ends_with(b"payload"));
    }

    #[test]
    fn response_decoder_accepts_valid_response() {
        let bytes = response(201, &[("content-type", "text/plain")], b"created");
        let response = decode_response(&bytes, 1024).unwrap();
        assert_eq!(response.status, 201);
        assert_eq!(response.headers[0].0, "content-type");
        assert_eq!(response.body, b"created");
    }

    #[test]
    fn response_decoder_rejects_framing_headers_and_oversized_bodies() {
        let framing = response(200, &[("content-length", "4")], b"test");
        assert!(decode_response(&framing, 1024).is_err());
        let oversized = response(200, &[], b"too large");
        assert!(decode_response(&oversized, 3).is_err());
        let reset_content = response(205, &[], b"forbidden");
        assert!(decode_response(&reset_content, 1024).is_err());
    }

    #[test]
    fn response_decoder_rejects_control_characters_in_header_values() {
        let control = response(200, &[("x-test", "bad\u{1}value")], b"");
        assert!(decode_response(&control, 1024).is_err());
    }

    #[test]
    fn response_decoder_rejects_invalid_envelope_fields() {
        let mut magic = response(200, &[], b"");
        magic[0] = b'X';
        assert!(decode_response(&magic, 1024).is_err());

        let mut version = response(200, &[], b"");
        version[8] = 2;
        assert!(decode_response(&version, 1024).is_err());

        let mut status = response(200, &[], b"");
        status[10..12].copy_from_slice(&199_u16.to_le_bytes());
        assert!(decode_response(&status, 1024).is_err());

        let mut trailing = response(200, &[], b"");
        trailing.push(0);
        assert!(decode_response(&trailing, 1024).is_err());
    }
}
