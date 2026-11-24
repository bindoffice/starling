use super::*;

pub(super) async fn bind_listeners() -> Option<(TcpListener, TcpListener)> {
    let upstream = match TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => listener,
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!("skipping client abort test: TCP binding is not permitted");
            return None;
        }
        Err(error) => panic!("bind upstream: {error}"),
    };
    let frontend = TcpListener::bind("127.0.0.1:0").await.unwrap();
    Some((upstream, frontend))
}

pub(super) fn spawn_frontend(
    frontend: TcpListener,
    config: &Arc<RuntimeConfig>,
) -> tokio::task::JoinHandle<()> {
    let group = build_listener_groups(config)[0].clone();
    let state = test_state(config);
    tokio::spawn(async move {
        let (stream, remote) = frontend.accept().await.unwrap();
        let (stream, control) = ClientSocketControl::attach_tcp(stream).unwrap();
        let local = stream
            .local_addr()
            .map(ListenAddress::Inet)
            .unwrap_or_else(|_| group.address.clone());
        handle_io_with_reset(
            stream,
            ConnectionAddresses::direct(ListenAddress::Inet(remote), local),
            group,
            state,
            None,
            control,
        )
        .await;
    })
}

pub(super) fn reset_client(client: TcpStream) {
    socket2::SockRef::from(&client)
        .set_linger(Some(Duration::ZERO))
        .unwrap();
    drop(client);
}

pub(super) async fn half_close_client(mut client: TcpStream) {
    client.shutdown().await.unwrap();
}

pub(super) async fn read_until<R>(client: &mut R, response: &mut Vec<u8>, needle: &[u8])
where
    R: AsyncRead + Unpin,
{
    while !response
        .windows(needle.len())
        .any(|window| window == needle)
    {
        let mut buffer = [0_u8; 1024];
        let read = client.read(&mut buffer).await.unwrap();
        assert!(
            read > 0,
            "client closed before receiving the expected bytes"
        );
        response.extend_from_slice(&buffer[..read]);
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
    let mut comma = [0_u8; 1];
    reader.read_exact(&mut comma).await.unwrap();
    assert_eq!(comma, [b',']);
}
