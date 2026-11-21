use super::*;

#[tokio::test]
async fn spills_in_order_and_cleans_the_temp_file() {
    let directory =
        std::env::temp_dir().join(format!("starling-body-window-spill-{}", std::process::id()));
    let window = BodyWindow::new(options(directory.clone(), 4, 16, 2));
    window
        .push_data(Bytes::from_static(b"abcdefghijkl"))
        .await
        .unwrap();
    window.finish().await;

    let path = window.spill_path().await.expect("spill file");
    assert!(path.exists());
    let mut body = Vec::new();
    while let Some(WindowItem::Data(data)) = window.pop().await.unwrap() {
        body.extend_from_slice(&data);
    }
    assert_eq!(body, b"abcdefghijkl");
    drop(window);
    assert!(!path.exists());
    let _ = std::fs::remove_dir_all(directory);
}
