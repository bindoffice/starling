use super::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn invalid_frame_shape(kind: u8, flags: u8, length: usize) -> Option<h2::Reason> {
    super::invalid_frame_shape(kind, flags, length, DEFAULT_MAX_FRAME_SIZE)
}

#[test]
fn validates_fixed_length_frames_like_nginx() {
    assert_eq!(
        invalid_frame_shape(FRAME_RST_STREAM, 0, 3),
        Some(h2::Reason::FRAME_SIZE_ERROR)
    );
    assert_eq!(
        invalid_frame_shape(FRAME_PING, 0, 7),
        Some(h2::Reason::FRAME_SIZE_ERROR)
    );
    assert_eq!(
        invalid_frame_shape(FRAME_SETTINGS, FLAG_SETTINGS_ACK, 6),
        Some(h2::Reason::FRAME_SIZE_ERROR)
    );
    assert_eq!(
        invalid_frame_shape(FRAME_GOAWAY, 0, 7),
        Some(h2::Reason::FRAME_SIZE_ERROR)
    );
    assert_eq!(
        invalid_frame_shape(FRAME_PRIORITY_UPDATE, 0, 3),
        Some(h2::Reason::FRAME_SIZE_ERROR)
    );
    assert_eq!(invalid_frame_shape(FRAME_PING, 0, 8), None);
    assert_eq!(invalid_frame_shape(FRAME_SETTINGS, 0, 6), None);
    assert_eq!(
        invalid_frame_shape(FRAME_SETTINGS, FLAG_SETTINGS_ACK, 0),
        None
    );
    assert_eq!(
        invalid_frame_shape(FRAME_DATA, FLAG_PADDED, 0),
        Some(h2::Reason::FRAME_SIZE_ERROR)
    );
    assert_eq!(
        invalid_frame_shape(FRAME_HEADERS, 0, 0),
        Some(h2::Reason::FRAME_SIZE_ERROR)
    );
    assert_eq!(
        invalid_frame_shape(FRAME_HEADERS, FLAG_PADDED, 1),
        Some(h2::Reason::FRAME_SIZE_ERROR)
    );
    assert_eq!(
        invalid_frame_shape(FRAME_HEADERS, FLAG_PRIORITY, 5),
        Some(h2::Reason::FRAME_SIZE_ERROR)
    );
    assert_eq!(invalid_frame_shape(FRAME_HEADERS, FLAG_PADDED, 2), None);
    assert_eq!(invalid_frame_shape(FRAME_HEADERS, FLAG_PRIORITY, 6), None);
}

#[test]
fn rejects_frames_larger_than_the_advertised_receive_size() {
    assert_eq!(
        invalid_frame_shape(FRAME_DATA, 0, DEFAULT_MAX_FRAME_SIZE + 1),
        Some(h2::Reason::FRAME_SIZE_ERROR)
    );
}

#[test]
fn accepts_frames_up_to_the_configured_receive_size() {
    let configured = 128 * 1024;
    assert_eq!(
        super::invalid_frame_shape(FRAME_HEADERS, FLAG_END_HEADERS, configured, configured),
        None
    );
    assert_eq!(
        super::invalid_frame_shape(FRAME_HEADERS, FLAG_END_HEADERS, configured + 1, configured),
        Some(h2::Reason::FRAME_SIZE_ERROR)
    );
}

#[test]
fn continuation_ignores_headers_only_flags() {
    let frame = HeaderFrame::new(FRAME_CONTINUATION, FLAG_PADDED | FLAG_PRIORITY, 16);
    assert_eq!(frame.prefix_remaining, 0);
    assert_eq!(frame.header_remaining, Some(16));
}

#[tokio::test]
async fn records_settings_acknowledgements() {
    let (mut client, server) = tokio::io::duplex(256);
    let (timeouts, _) = FrameTimeoutTracker::new(
        std::time::Duration::from_secs(1),
        std::time::Duration::from_secs(1),
    );
    let settings_acked = Arc::new(AtomicBool::new(false));
    let mut stream = FrameValidatedStream::new(
        server,
        8 * 1024,
        DEFAULT_MAX_FRAME_SIZE,
        timeouts,
        settings_acked.clone(),
    );

    let mut preface = b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n".to_vec();
    preface.extend_from_slice(&[0, 0, 0, FRAME_SETTINGS, FLAG_SETTINGS_ACK, 0, 0, 0, 0]);
    client.write_all(&preface).await.unwrap();

    let mut received = vec![0_u8; preface.len()];
    stream.read_exact(&mut received).await.unwrap();
    assert_eq!(received, preface);
    assert!(settings_acked.load(Ordering::Relaxed));
}

#[tokio::test]
async fn withholds_an_incomplete_data_frame() {
    let (mut client, server) = tokio::io::duplex(256);
    let (timeouts, _) = FrameTimeoutTracker::new(
        std::time::Duration::from_secs(1),
        std::time::Duration::from_secs(1),
    );
    let mut stream = FrameValidatedStream::new(
        server,
        8 * 1024,
        DEFAULT_MAX_FRAME_SIZE,
        timeouts,
        Arc::new(AtomicBool::new(false)),
    );

    let preface = b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n";
    let frame_header = [0, 0, 8, FRAME_DATA, 0, 0, 0, 0, 1];
    client.write_all(preface).await.unwrap();
    client.write_all(&frame_header).await.unwrap();
    client.write_all(b"half").await.unwrap();

    let mut received_preface = vec![0_u8; preface.len()];
    stream.read_exact(&mut received_preface).await.unwrap();
    assert_eq!(received_preface, preface);
    assert!(
        tokio::time::timeout(
            std::time::Duration::from_millis(20),
            stream.read(&mut [0_u8; 1])
        )
        .await
        .is_err(),
        "an incomplete DATA frame must not be exposed to h2"
    );

    client.write_all(b"more").await.unwrap();
    let mut received_frame = vec![0_u8; frame_header.len() + 8];
    stream.read_exact(&mut received_frame).await.unwrap();
    assert_eq!(&received_frame[..frame_header.len()], &frame_header);
    assert_eq!(&received_frame[frame_header.len()..], b"halfmore");
}
