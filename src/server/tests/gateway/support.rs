use super::*;

pub(super) fn test_env() -> RequestEnv {
    RequestEnv {
        uri: "/".into(),
        args: None,
        unparsed_uri: Some("/".into()),
        internal: false,
        bypass_try_files: false,
        limits_checked: false,
        auth_request_checked: false,
        mirror_checked: false,
    }
}

pub(super) fn remote() -> SocketAddr {
    "127.0.0.1:45678".parse().unwrap()
}

pub(super) fn local(port: u16) -> SocketAddr {
    format!("127.0.0.1:{port}").parse().unwrap()
}

pub(super) fn streaming_request(
    uri: &str,
    local_addr: &str,
    length: usize,
) -> (crate::http::RequestBodySender, Request) {
    let (sender, stream) = crate::http::RequestBodyStream::channel();
    let request = Request {
        method: "POST".into(),
        target: uri.into(),
        version: "HTTP/1.1".into(),
        headers: vec![
            Header {
                name: "Host".into(),
                value: "localhost".into(),
            },
            Header {
                name: "Content-Length".into(),
                value: length.to_string(),
            },
        ],
        body_stream: Some(stream),
        body: Vec::new(),
        remote_addr: "127.0.0.1:45678".into(),
        local_addr: local_addr.into(),
        keep_alive: false,
    };
    (sender, request)
}

pub(super) fn streaming_chunked_request(
    uri: &str,
    local_addr: &str,
) -> (crate::http::RequestBodySender, Request) {
    let (sender, stream) = crate::http::RequestBodyStream::channel();
    let request = Request {
        method: "POST".into(),
        target: uri.into(),
        version: "HTTP/1.1".into(),
        headers: vec![
            Header {
                name: "Host".into(),
                value: "localhost".into(),
            },
            Header {
                name: "Transfer-Encoding".into(),
                value: "chunked".into(),
            },
        ],
        body_stream: Some(stream),
        body: Vec::new(),
        remote_addr: "127.0.0.1:45678".into(),
        local_addr: local_addr.into(),
        keep_alive: false,
    };
    (sender, request)
}

pub(super) fn plain_request(uri: &str, local_addr: &str) -> Request {
    Request {
        method: "GET".into(),
        target: uri.into(),
        version: "HTTP/1.1".into(),
        headers: vec![Header {
            name: "Host".into(),
            value: "localhost".into(),
        }],
        body_stream: None,
        body: Vec::new(),
        remote_addr: "127.0.0.1:45678".into(),
        local_addr: local_addr.into(),
        keep_alive: false,
    }
}

pub(super) async fn next_fastcgi_stdin<R: AsyncRead + Unpin>(reader: &mut R) -> Vec<u8> {
    loop {
        let record = read_fastcgi_record(reader).await.unwrap();
        if record.record_type == FCGI_STDIN {
            return record.content;
        }
    }
}

pub(super) async fn read_uwsgi_header<R: AsyncRead + Unpin>(reader: &mut R) {
    let mut header = [0_u8; 4];
    reader.read_exact(&mut header).await.unwrap();
    let length = u16::from_le_bytes([header[1], header[2]]) as usize;
    let mut parameters = vec![0_u8; length];
    reader.read_exact(&mut parameters).await.unwrap();
}

pub(super) async fn read_scgi_header<R: AsyncRead + Unpin>(reader: &mut R) {
    let mut length = Vec::new();
    let mut byte = [0_u8; 1];
    loop {
        reader.read_exact(&mut byte).await.unwrap();
        if byte[0] == b':' {
            break;
        }
        length.push(byte[0]);
    }
    let length = std::str::from_utf8(&length).unwrap().parse().unwrap();
    let mut parameters = vec![0_u8; length];
    reader.read_exact(&mut parameters).await.unwrap();
}
