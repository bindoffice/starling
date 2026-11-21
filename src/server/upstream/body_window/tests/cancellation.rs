use super::*;

#[tokio::test]
async fn abort_does_not_wait_for_a_blocked_spill_write_and_removes_the_file() {
    let directory = std::env::temp_dir().join(format!(
        "starling-body-window-blocked-spill-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&directory).unwrap();
    let mut window = BodyWindow::new(options(directory.clone(), 0, 4, 2));
    let barrier = Arc::new(TestSpillWriteBarrier::default());
    window.spill_write_barrier = Some(barrier.clone());
    let window = Arc::new(window);
    let producer = {
        let window = window.clone();
        tokio::spawn(async move { window.push_data(Bytes::from_static(b"abcd")).await })
    };

    barrier.entered.notified().await;
    tokio::time::timeout(Duration::from_millis(100), window.abort())
        .await
        .expect("abort must not wait for a blocked temp-file write");
    barrier.release.notify_one();

    let error = producer
        .await
        .unwrap()
        .expect_err("the producer must observe the abort after its write resumes");
    assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
    assert!(window.spill_path().await.is_none());
    assert!(
        std::fs::read_dir(&directory)
            .map(|mut entries| entries.next().is_none())
            .unwrap_or(true),
        "abort must remove the blocked spill file"
    );
    drop(window);
    let _ = std::fs::remove_dir_all(directory);
}

#[tokio::test]
async fn abort_unblocks_producer_and_removes_the_spill_file() {
    let directory =
        std::env::temp_dir().join(format!("starling-body-window-abort-{}", std::process::id()));
    let window = Arc::new(BodyWindow::new(options(directory.clone(), 0, 4, 2)));
    window.push_data(Bytes::from_static(b"abcd")).await.unwrap();
    let path = window.spill_path().await.expect("spill file");
    let mut producer = {
        let window = window.clone();
        tokio::spawn(async move { window.push_data(Bytes::from_static(b"efgh")).await })
    };
    assert!(
        tokio::time::timeout(Duration::from_millis(50), &mut producer)
            .await
            .is_err(),
        "producer should wait before cancellation"
    );

    window.abort().await;
    let error = tokio::time::timeout(Duration::from_secs(1), producer)
        .await
        .expect("abort should wake the producer")
        .unwrap()
        .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
    assert!(!path.exists(), "abort should remove the temp file");
    drop(window);
    let _ = std::fs::remove_dir_all(directory);
}

#[tokio::test]
async fn externally_removed_spill_file_reports_a_read_error_and_aborts_cleanly() {
    let directory = std::env::temp_dir().join(format!(
        "starling-body-window-external-delete-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&directory).unwrap();
    let window = Arc::new(BodyWindow::new(options(directory.clone(), 0, 4, 2)));
    window.push_data(Bytes::from_static(b"abcd")).await.unwrap();
    let path = window.spill_path().await.expect("spill file");
    std::fs::remove_file(&path).unwrap();

    let error = match window.pop().await {
        Err(error) => error,
        Ok(_) => panic!("a concurrently removed spill file must fail the body stream"),
    };
    assert_eq!(error.kind(), io::ErrorKind::NotFound);

    tokio::join!(window.abort(), window.abort());
    assert!(window.spill_path().await.is_none());
    assert!(!path.exists());
    drop(window);
    let _ = std::fs::remove_dir_all(directory);
}

#[tokio::test]
async fn dropping_the_response_stream_aborts_the_producer_and_cleans_up() {
    let directory =
        std::env::temp_dir().join(format!("starling-body-window-drop-{}", std::process::id()));
    let window = Arc::new(BodyWindow::new(options(directory.clone(), 0, 4, 2)));
    let producer_window = window.clone();
    let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
    let (dropped_tx, dropped_rx) = tokio::sync::oneshot::channel();
    let response = spawn_windowed_body(window.clone(), None, false, async move {
        producer_window
            .push_data(Bytes::from_static(b"abcd"))
            .await?;
        let _dropped = ProducerDropSignal(Some(dropped_tx));
        let _ = ready_tx.send(());
        std::future::pending::<io::Result<()>>().await
    });
    let ResponseBody::Stream(stream) = response else {
        panic!("expected a streamed response body");
    };
    ready_rx.await.unwrap();
    let path = window.spill_path().await.expect("spill file");
    drop(stream);

    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if window.spill_path().await.is_none() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("dropping the response stream should abort the window");
    assert!(!path.exists(), "abort should remove the temp file");
    tokio::time::timeout(Duration::from_secs(1), dropped_rx)
        .await
        .expect("downstream disconnect should abort the producer")
        .expect("producer future should be dropped");
    drop(window);
    let _ = std::fs::remove_dir_all(directory);
}

#[tokio::test]
async fn dropping_response_stream_cancels_a_producer_blocked_after_the_last_buffer() {
    let directory = std::env::temp_dir().join(format!(
        "starling-body-window-idle-producer-{}",
        std::process::id()
    ));
    let window = Arc::new(BodyWindow::new(options(directory.clone(), 0, 4, 4)));
    let producer_window = window.clone();
    let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
    let (dropped_tx, dropped_rx) = tokio::sync::oneshot::channel();
    let response = spawn_windowed_body(window.clone(), None, false, async move {
        producer_window
            .push_data(Bytes::from_static(b"abcd"))
            .await
            .unwrap();
        let _dropped = ProducerDropSignal(Some(dropped_tx));
        let _ = ready_tx.send(());
        std::future::pending::<io::Result<()>>().await
    });
    let ResponseBody::Stream(mut stream) = response else {
        panic!("expected a streamed response body");
    };
    ready_rx.await.unwrap();
    let path = window.spill_path().await.expect("spill file");
    let item = stream.next().await.unwrap().expect("buffered data");
    let ResponseStreamItem::Data(item) = item else {
        panic!("expected data");
    };
    assert_eq!(&item[..], b"abcd");
    drop(item);
    tokio::task::yield_now().await;
    drop(stream);

    tokio::time::timeout(Duration::from_secs(1), dropped_rx)
        .await
        .expect("downstream disconnect should abort the idle producer")
        .expect("producer future should be dropped");
    assert!(
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if window.spill_path().await.is_none() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .is_ok(),
        "downstream disconnect should remove the spill file"
    );
    assert!(!path.exists(), "abort should remove the temp file");
    drop(window);
    let _ = std::fs::remove_dir_all(directory);
}

#[tokio::test]
async fn dropping_response_stream_cancels_a_producer_blocked_for_capacity() {
    let directory = std::env::temp_dir().join(format!(
        "starling-body-window-capacity-cancel-{}",
        std::process::id()
    ));
    let window = Arc::new(BodyWindow::new(options(directory.clone(), 0, 4, 2)));
    let producer_window = window.clone();
    let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
    let (dropped_tx, dropped_rx) = tokio::sync::oneshot::channel();
    let response = spawn_windowed_body(window.clone(), None, false, async move {
        producer_window
            .push_data(Bytes::from_static(b"abcd"))
            .await
            .unwrap();
        let _dropped = ProducerDropSignal(Some(dropped_tx));
        let _ = ready_tx.send(());
        producer_window.push_data(Bytes::from_static(b"efgh")).await
    });
    let ResponseBody::Stream(stream) = response else {
        panic!("expected a streamed response body");
    };
    ready_rx.await.unwrap();
    let path = window.spill_path().await.expect("spill file");
    drop(stream);

    tokio::time::timeout(Duration::from_secs(1), dropped_rx)
        .await
        .expect("downstream disconnect should abort a producer waiting for capacity")
        .expect("producer future should be dropped");
    assert!(
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if window.spill_path().await.is_none() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .is_ok(),
        "capacity-blocked cancellation should remove the spill file"
    );
    assert!(!path.exists(), "abort should remove the temp file");
    drop(window);
    let _ = std::fs::remove_dir_all(directory);
}

struct ProducerDropSignal(Option<tokio::sync::oneshot::Sender<()>>);

impl Drop for ProducerDropSignal {
    fn drop(&mut self) {
        if let Some(sender) = self.0.take() {
            let _ = sender.send(());
        }
    }
}

#[tokio::test]
async fn producer_failure_drains_buffered_data_before_reporting_the_error() {
    let directory =
        std::env::temp_dir().join(format!("starling-body-window-error-{}", std::process::id()));
    let window = Arc::new(BodyWindow::new(options(directory.clone(), 4, 16, 2)));
    let producer_window = window.clone();
    let (path_tx, path_rx) = tokio::sync::oneshot::channel();
    let response = spawn_windowed_body(window.clone(), None, false, async move {
        producer_window
            .push_data(Bytes::from_static(b"abcdefghijkl"))
            .await?;
        let path = producer_window
            .spill_path()
            .await
            .expect("producer should have created a spill file");
        let _ = path_tx.send(path);
        Err(io::Error::new(
            io::ErrorKind::ConnectionAborted,
            "upstream response failed",
        ))
    });
    let path = path_rx
        .await
        .expect("producer should report the spill path");
    let ResponseBody::Stream(mut stream) = response else {
        panic!("expected a streamed response body");
    };

    let mut body = Vec::new();
    let error = loop {
        match stream.next().await {
            Ok(Some(ResponseStreamItem::Data(data))) => body.extend_from_slice(&data),
            Ok(Some(ResponseStreamItem::Trailers(_))) => {}
            Ok(None) => panic!("failed producer should not finish the stream cleanly"),
            Err(error) => break error,
        }
    };
    assert_eq!(body, b"abcdefghijkl");
    assert_eq!(error.kind(), io::ErrorKind::ConnectionAborted);
    assert!(
        !path.exists(),
        "the spill file should be removed after the producer error"
    );
    drop(window);
    let _ = std::fs::remove_dir_all(directory);
}

#[tokio::test]
async fn producer_panic_fails_the_stream_instead_of_hanging() {
    let window = Arc::new(BodyWindow::new(options_with_busy(
        std::env::temp_dir(),
        4,
        0,
        2,
        4,
    )));
    let producer_window = window.clone();
    let response = spawn_windowed_body(window.clone(), None, false, async move {
        producer_window
            .push_data(Bytes::from_static(b"data"))
            .await
            .unwrap();
        panic!("injected producer panic");
    });
    let ResponseBody::Stream(mut stream) = response else {
        panic!("expected a streamed response body");
    };

    let mut body = Vec::new();
    let error = tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            match stream.next().await {
                Ok(Some(ResponseStreamItem::Data(data))) => body.extend_from_slice(&data),
                Ok(Some(ResponseStreamItem::Trailers(_))) => {}
                Ok(None) => panic!("a panicked producer must not finish the stream cleanly"),
                Err(error) => break error,
            }
        }
    })
    .await
    .expect("a producer panic must not hang the downstream stream");
    assert_eq!(body, b"data");
    assert_eq!(error.kind(), io::ErrorKind::Other);
    drop(window);
}
