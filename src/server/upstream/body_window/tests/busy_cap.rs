use super::*;

async fn next_data(window: &BodyWindow) -> Bytes {
    let item = window.pop().await.unwrap().expect("window item");
    let WindowItem::Data(data) = item else {
        panic!("expected data");
    };
    data
}

#[tokio::test]
async fn emits_memory_data_in_configured_buffer_sizes() {
    let directory = std::env::temp_dir().join(format!(
        "starling-body-window-buffer-size-{}",
        std::process::id()
    ));
    let mut options = options_with_busy(directory.clone(), 12, 0, 4, usize::MAX);
    options.buffer_count = 3;
    options.buffer_size = 4;
    let window = BodyWindow::new(options);
    window
        .push_data(Bytes::from_static(b"abcdefghij"))
        .await
        .unwrap();
    window.finish().await;

    assert_eq!(&next_data(&window).await[..], b"abcd");
    assert_eq!(&next_data(&window).await[..], b"efgh");
    assert_eq!(&next_data(&window).await[..], b"ij");
    assert!(window.pop().await.unwrap().is_none());
    drop(window);
    let _ = std::fs::remove_dir_all(directory);
}

#[tokio::test]
async fn coalesces_small_pushes_into_buffer_sized_chunks() {
    let directory = std::env::temp_dir().join(format!(
        "starling-body-window-buffer-coalesce-{}",
        std::process::id()
    ));
    let mut options = options_with_busy(directory.clone(), 8, 0, 4, usize::MAX);
    options.buffer_count = 2;
    options.buffer_size = 4;
    let window = BodyWindow::new(options);
    for chunk in [b"ab".as_slice(), b"cde", b"f"] {
        window
            .push_data(Bytes::copy_from_slice(chunk))
            .await
            .unwrap();
    }
    window.finish().await;

    assert_eq!(&next_data(&window).await[..], b"abcd");
    assert_eq!(&next_data(&window).await[..], b"ef");
    assert!(window.pop().await.unwrap().is_none());
    drop(window);
    let _ = std::fs::remove_dir_all(directory);
}

#[tokio::test]
async fn splits_file_data_at_the_configured_buffer_size() {
    let directory = std::env::temp_dir().join(format!(
        "starling-body-window-file-buffer-size-{}",
        std::process::id()
    ));
    let mut options = options_with_busy(directory.clone(), 0, 16, 8, usize::MAX);
    options.buffer_count = 2;
    options.buffer_size = 4;
    let window = BodyWindow::new(options);
    window
        .push_data(Bytes::from_static(b"abcdefgh"))
        .await
        .unwrap();
    window.finish().await;

    assert_eq!(&next_data(&window).await[..], b"abcd");
    assert_eq!(&next_data(&window).await[..], b"efgh");
    assert!(window.pop().await.unwrap().is_none());
    drop(window);
    let _ = std::fs::remove_dir_all(directory);
}

#[tokio::test]
async fn caps_memory_data_at_the_remaining_busy_window() {
    let directory = std::env::temp_dir().join(format!(
        "starling-body-window-memory-busy-cap-{}",
        std::process::id()
    ));
    let window = Arc::new(BodyWindow::new(options_with_busy(
        directory.clone(),
        8,
        0,
        4,
        4,
    )));
    window.push_data(Bytes::from_static(b"abc")).await.unwrap();
    window
        .push_data(Bytes::from_static(b"defgh"))
        .await
        .unwrap();

    let first = next_data(&window).await;
    assert_eq!(&first[..], b"abcd");

    let mut second = {
        let window = window.clone();
        tokio::spawn(async move { next_data(&window).await })
    };
    assert!(
        tokio::time::timeout(Duration::from_millis(50), &mut second)
            .await
            .is_err(),
        "a full busy window must stop the next buffer"
    );
    drop(first);
    let second = tokio::time::timeout(Duration::from_secs(1), &mut second)
        .await
        .expect("busy capacity should wake the blocked pop")
        .expect("pop task should complete");
    assert_eq!(&second[..], b"efgh");
    drop(second);
    window.finish().await;
    assert!(window.pop().await.unwrap().is_none());
    drop(window);
    let _ = std::fs::remove_dir_all(directory);
}

#[tokio::test]
async fn caps_file_data_at_the_remaining_busy_window() {
    let directory = std::env::temp_dir().join(format!(
        "starling-body-window-file-busy-cap-{}",
        std::process::id()
    ));
    let mut options = options_with_busy(directory.clone(), 0, 16, 4, 4);
    options.buffer_size = 4;
    let window = Arc::new(BodyWindow::new(options));
    window.push_data(Bytes::from_static(b"abc")).await.unwrap();
    window
        .push_data(Bytes::from_static(b"defgh"))
        .await
        .unwrap();

    let first = next_data(&window).await;
    assert_eq!(&first[..], b"abc");

    let second = next_data(&window).await;
    assert_eq!(
        &second[..],
        b"d",
        "file-backed data must obey the remaining byte cap"
    );
    drop(first);
    let third = next_data(&window).await;
    assert_eq!(&third[..], b"efg");
    drop(second);
    drop(third);
    let fourth = next_data(&window).await;
    assert_eq!(&fourth[..], b"h");
    drop(fourth);
    window.finish().await;
    assert!(window.pop().await.unwrap().is_none());
    drop(window);
    let _ = std::fs::remove_dir_all(directory);
}
