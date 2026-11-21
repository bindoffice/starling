use super::*;

#[tokio::test]
async fn waits_when_memory_and_temp_capacity_are_exhausted() {
    let directory =
        std::env::temp_dir().join(format!("starling-body-window-block-{}", std::process::id()));
    let window = Arc::new(BodyWindow::new(options(directory.clone(), 4, 4, 2)));
    window
        .push_data(Bytes::from_static(b"abcdefgh"))
        .await
        .unwrap();
    let mut producer = {
        let window = window.clone();
        tokio::spawn(async move { window.push_data(Bytes::from_static(b"i")).await })
    };
    assert!(
        tokio::time::timeout(Duration::from_millis(50), &mut producer)
            .await
            .is_err(),
        "producer should wait when the full window is busy"
    );

    let first = window.pop().await.unwrap().expect("first window item");
    let WindowItem::Data(first) = first else {
        panic!("expected data");
    };
    assert_eq!(&first[..], b"abcd");
    assert!(
        tokio::time::timeout(Duration::from_millis(50), &mut producer)
            .await
            .is_err(),
        "producer should wait while the popped buffer is still busy downstream"
    );
    let first_body = first.to_vec();
    drop(first);
    producer.await.unwrap().unwrap();
    window.finish().await;

    let mut body = first_body;
    while let Some(WindowItem::Data(data)) = window.pop().await.unwrap() {
        body.extend_from_slice(&data);
    }
    assert_eq!(body, b"abcdefghi");
    drop(window);
    let _ = std::fs::remove_dir_all(directory);
}

#[tokio::test]
async fn busy_limit_holds_the_next_buffer_until_downstream_releases() {
    let directory =
        std::env::temp_dir().join(format!("starling-body-window-busy-{}", std::process::id()));
    let window = Arc::new(BodyWindow::new(options_with_busy(
        directory.clone(),
        8,
        0,
        4,
        4,
    )));
    window
        .push_data(Bytes::from_static(b"abcdefgh"))
        .await
        .unwrap();

    let first = window.pop().await.unwrap().expect("first window item");
    let WindowItem::Data(first) = first else {
        panic!("expected data");
    };
    assert_eq!(&first[..], b"abcd");

    let mut second = {
        let window = window.clone();
        tokio::spawn(async move { window.pop().await })
    };
    assert!(
        tokio::time::timeout(Duration::from_millis(50), &mut second)
            .await
            .is_err(),
        "a full busy window must stop the next pop"
    );

    drop(first);
    let second = tokio::time::timeout(Duration::from_secs(1), &mut second)
        .await
        .expect("busy capacity should wake the blocked pop")
        .unwrap()
        .unwrap()
        .expect("second window item");
    let WindowItem::Data(second) = second else {
        panic!("expected data");
    };
    assert_eq!(&second[..], b"efgh");
    drop(second);
    window.finish().await;
    assert!(window.pop().await.unwrap().is_none());
    drop(window);
    let _ = std::fs::remove_dir_all(directory);
}

#[tokio::test]
async fn busy_limit_applies_to_temp_file_buffers() {
    let directory = std::env::temp_dir().join(format!(
        "starling-body-window-file-busy-{}",
        std::process::id()
    ));
    let window = Arc::new(BodyWindow::new(options_with_busy(
        directory.clone(),
        0,
        8,
        4,
        4,
    )));
    window
        .push_data(Bytes::from_static(b"abcdefgh"))
        .await
        .unwrap();

    let first = window.pop().await.unwrap().expect("first window item");
    let WindowItem::Data(first) = first else {
        panic!("expected data");
    };
    assert_eq!(&first[..], b"abcd");
    let mut second = {
        let window = window.clone();
        tokio::spawn(async move { window.pop().await })
    };
    assert!(
        tokio::time::timeout(Duration::from_millis(50), &mut second)
            .await
            .is_err(),
        "a file-backed buffer must also occupy the busy window"
    );

    drop(first);
    let second = tokio::time::timeout(Duration::from_secs(1), &mut second)
        .await
        .expect("file-backed busy capacity should wake after release")
        .unwrap()
        .unwrap()
        .expect("second window item");
    let WindowItem::Data(second) = second else {
        panic!("expected data");
    };
    assert_eq!(&second[..], b"efgh");
    drop(second);
    window.finish().await;
    assert!(window.pop().await.unwrap().is_none());
    drop(window);
    let _ = std::fs::remove_dir_all(directory);
}

#[tokio::test]
async fn cloned_downstream_buffers_hold_capacity_until_the_last_reference_drops() {
    let directory =
        std::env::temp_dir().join(format!("starling-body-window-clone-{}", std::process::id()));
    let window = Arc::new(BodyWindow::new(options(directory.clone(), 4, 0, 2)));
    window.push_data(Bytes::from_static(b"abcd")).await.unwrap();
    let mut producer = {
        let window = window.clone();
        tokio::spawn(async move { window.push_data(Bytes::from_static(b"e")).await })
    };

    let WindowItem::Data(first) = window.pop().await.unwrap().expect("first window item") else {
        panic!("expected data");
    };
    let clone = first.clone();
    drop(first);
    assert!(
        tokio::time::timeout(Duration::from_millis(50), &mut producer)
            .await
            .is_err(),
        "a cloned downstream buffer must keep the window capacity busy"
    );
    drop(clone);

    tokio::time::timeout(Duration::from_secs(1), producer)
        .await
        .expect("producer should resume after the final buffer reference drops")
        .unwrap()
        .unwrap();
    window.finish().await;
    while let Some(item) = window.pop().await.unwrap() {
        let WindowItem::Data(data) = item else {
            panic!("expected data");
        };
        assert_eq!(&data[..], b"e");
    }
    drop(window);
    let _ = std::fs::remove_dir_all(directory);
}

#[tokio::test]
async fn reuses_consumed_temp_space_without_deadlocking_large_streams() {
    let directory =
        std::env::temp_dir().join(format!("starling-body-window-reuse-{}", std::process::id()));
    let window = Arc::new(BodyWindow::new(options(directory.clone(), 0, 4, 2)));
    window.push_data(Bytes::from_static(b"abcd")).await.unwrap();
    let mut producer = {
        let window = window.clone();
        tokio::spawn(async move { window.push_data(Bytes::from_static(b"efgh")).await })
    };
    assert!(
        tokio::time::timeout(Duration::from_millis(50), &mut producer)
            .await
            .is_err(),
        "producer should wait while all temp-file space is unread"
    );

    let first = window.pop().await.unwrap().expect("first window item");
    let WindowItem::Data(first) = first else {
        panic!("expected data");
    };
    assert_eq!(&first[..], b"ab");

    let second = window.pop().await.unwrap().expect("second window item");
    let WindowItem::Data(second) = second else {
        panic!("expected data");
    };
    assert_eq!(&second[..], b"cd");
    assert!(
        tokio::time::timeout(Duration::from_millis(50), &mut producer)
            .await
            .is_err(),
        "producer should not reuse temp extents held by downstream buffers"
    );
    let first_body = first.to_vec();
    let second_body = second.to_vec();
    drop(first);
    drop(second);
    tokio::time::timeout(Duration::from_secs(1), producer)
        .await
        .expect("producer should resume after temp space is consumed")
        .unwrap()
        .unwrap();
    window.finish().await;

    let mut body = first_body;
    body.extend_from_slice(&second_body);
    while let Some(WindowItem::Data(data)) = window.pop().await.unwrap() {
        body.extend_from_slice(&data);
    }
    assert_eq!(body, b"abcdefgh");
    drop(window);
    let _ = std::fs::remove_dir_all(directory);
}

#[tokio::test]
async fn reuses_fragmented_temp_extents_released_out_of_order() {
    let directory = std::env::temp_dir().join(format!(
        "starling-body-window-fragmented-{}",
        std::process::id()
    ));
    let window = Arc::new(BodyWindow::new(options(directory.clone(), 0, 16, 8)));
    window.push_data(Bytes::from_static(b"aaaa")).await.unwrap();
    window
        .push_data(Bytes::from_static(b"bbbbbbbb"))
        .await
        .unwrap();
    window.push_data(Bytes::from_static(b"cccc")).await.unwrap();

    let first = window.pop().await.unwrap().expect("first window item");
    let second = window.pop().await.unwrap().expect("second window item");
    let third = window.pop().await.unwrap().expect("third window item");
    let (WindowItem::Data(first), WindowItem::Data(second), WindowItem::Data(third)) =
        (first, second, third)
    else {
        panic!("expected data");
    };
    let first_body = first.to_vec();
    let second_body = second.to_vec();
    let third_body = third.to_vec();

    // Keep the middle extent busy while releasing the two smaller extents.
    drop(first);
    drop(third);
    let mut producer = {
        let window = window.clone();
        tokio::spawn(async move { window.push_data(Bytes::from_static(b"dddddddd")).await })
    };
    tokio::time::timeout(Duration::from_secs(1), &mut producer)
        .await
        .expect("fragmented temp extents should not panic or deadlock")
        .unwrap()
        .unwrap();
    drop(second);
    window.finish().await;

    let mut body = first_body;
    body.extend_from_slice(&second_body);
    body.extend_from_slice(&third_body);
    while let Some(item) = window.pop().await.unwrap() {
        let WindowItem::Data(data) = item else {
            panic!("expected data");
        };
        body.extend_from_slice(&data);
    }
    assert_eq!(body, b"aaaabbbbbbbbccccdddddddd");
    drop(window);
    let _ = std::fs::remove_dir_all(directory);
}

#[tokio::test]
async fn temp_file_creation_failure_releases_reserved_space_and_allows_retry() {
    let directory = std::env::temp_dir().join(format!(
        "starling-body-window-temp-failure-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&directory).unwrap();
    let temp_path = directory.join("not-a-directory");
    std::fs::write(&temp_path, b"blocked").unwrap();
    let window = BodyWindow::new(options(temp_path.clone(), 0, 8, 2));

    let error = window
        .push_data(Bytes::from_static(b"abcd"))
        .await
        .expect_err("a file in place of the temp directory must fail");
    assert!(
        error.kind() == io::ErrorKind::NotADirectory
            || error.kind() == io::ErrorKind::AlreadyExists
            || error.kind() == io::ErrorKind::Other,
        "unexpected temp-file error: {error}"
    );
    assert!(
        window.spill_path().await.is_none(),
        "a failed temp-file creation must not leave a spill handle"
    );

    std::fs::remove_file(&temp_path).unwrap();
    std::fs::create_dir_all(&temp_path).unwrap();
    window
        .push_data(Bytes::from_static(b"abcd"))
        .await
        .expect("reserved temp space should be reusable after fixing the directory");
    window.finish().await;

    let mut body = Vec::new();
    while let Some(WindowItem::Data(data)) = window.pop().await.unwrap() {
        body.extend_from_slice(&data);
    }
    assert_eq!(body, b"abcd");
    drop(window);
    let _ = std::fs::remove_dir_all(directory);
}

#[tokio::test]
async fn truncated_temp_file_reports_read_failure_and_cleans_up_on_abort() {
    let directory = std::env::temp_dir().join(format!(
        "starling-body-window-truncated-{}",
        std::process::id()
    ));
    let window = BodyWindow::new(options(directory.clone(), 0, 8, 4));
    window
        .push_data(Bytes::from_static(b"abcdefgh"))
        .await
        .unwrap();
    let path = window.spill_path().await.expect("spill file");
    let mut state = window.state.lock().await;
    let spill = state.spill.as_mut().expect("spill file state");
    spill.file.set_len(2).await.unwrap();
    spill.file.sync_all().await.unwrap();
    drop(state);
    window.finish().await;

    let error = match window.pop().await {
        Err(error) => error,
        Ok(_) => panic!("a truncated spill file must fail the body stream"),
    };
    assert_eq!(error.kind(), io::ErrorKind::UnexpectedEof);
    window.abort().await;
    assert!(!path.exists(), "abort must remove a failed spill file");
    assert!(window.spill_path().await.is_none());
    drop(window);
    let _ = std::fs::remove_dir_all(directory);
}

#[tokio::test]
async fn failed_spill_write_preserves_earlier_file_data_and_reports_its_error() {
    let directory = std::env::temp_dir().join(format!(
        "starling-body-window-mid-write-failure-{}",
        std::process::id()
    ));
    let window = BodyWindow::new(options(directory.clone(), 0, 8, 2));
    window
        .spill_write_failure_after
        .store(1, std::sync::atomic::Ordering::Relaxed);

    let error = window
        .push_data(Bytes::from_static(b"abcd"))
        .await
        .expect_err("the second spill write should fail");
    assert_eq!(error.kind(), io::ErrorKind::Other);
    window.fail(error).await;

    let path = window.spill_path().await.expect("spill file");
    let first = window.pop().await.unwrap().expect("earlier file data");
    let WindowItem::Data(first) = first else {
        panic!("expected data");
    };
    assert_eq!(&first[..], b"ab");
    drop(first);

    let error = match window.pop().await {
        Err(error) => error,
        Ok(_) => panic!("the original write error must follow buffered data"),
    };
    assert_eq!(error.kind(), io::ErrorKind::Other);
    assert_eq!(error.to_string(), "injected spill write failure");

    window.abort().await;
    assert!(!path.exists(), "abort must remove the failed spill file");
    drop(window);
    let _ = std::fs::remove_dir_all(directory);
}

#[tokio::test]
async fn failed_spill_write_discards_the_partial_file_and_keeps_state_clean() {
    let directory = std::env::temp_dir().join(format!(
        "starling-body-window-write-failure-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&directory).unwrap();
    let options = options(directory.clone(), 0, 8, 4);
    let path = {
        let (path, file) = create_temp_file(
            options.temp_path.as_ref().expect("temp path"),
            "failed spill write",
        )
        .unwrap();
        let _guard = TempFileGuard::new(path.clone(), true);
        write_spill_file(tokio::fs::File::from_std(file), u64::MAX, b"abcd")
            .await
            .expect_err("an invalid spill offset must fail the write");
        path
    };
    assert!(
        !path.exists(),
        "failed spill write must not leave a temp file"
    );
    let _ = std::fs::remove_dir_all(directory);
}
