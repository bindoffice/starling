use super::*;

#[cfg(unix)]
#[tokio::test]
async fn read_only_temp_directory_recovers_after_permissions_are_restored() {
    use std::os::unix::fs::PermissionsExt;

    let directory = std::env::temp_dir().join(format!(
        "starling-body-window-permissions-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).unwrap();
    let window = BodyWindow::new(options(directory.clone(), 0, 8, 4));
    std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o500)).unwrap();

    let result = window.push_data(Bytes::from_static(b"abcd")).await;
    std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700)).unwrap();

    if result.is_ok() {
        // Root can bypass directory permissions, so this environment cannot
        // exercise the permission-denied path.
        window.finish().await;
        while window.pop().await.unwrap().is_some() {}
        drop(window);
        let _ = std::fs::remove_dir_all(directory);
        return;
    }

    let error = result.expect_err("read-only temp directory must fail");
    assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
    assert!(
        window.spill_path().await.is_none(),
        "failed temp-file creation must not retain a spill handle"
    );

    window
        .push_data(Bytes::from_static(b"abcd"))
        .await
        .expect("restored permissions should release reserved temp space");
    window.finish().await;
    let mut body = Vec::new();
    while let Some(WindowItem::Data(data)) = window.pop().await.unwrap() {
        body.extend_from_slice(&data);
    }
    assert_eq!(body, b"abcd");
    drop(window);
    let _ = std::fs::remove_dir_all(directory);
}
