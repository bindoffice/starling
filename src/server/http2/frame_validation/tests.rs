use super::*;

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
fn continuation_ignores_headers_only_flags() {
    let frame = HeaderFrame::new(FRAME_CONTINUATION, FLAG_PADDED | FLAG_PRIORITY, 16);
    assert_eq!(frame.prefix_remaining, 0);
    assert_eq!(frame.header_remaining, Some(16));
}
