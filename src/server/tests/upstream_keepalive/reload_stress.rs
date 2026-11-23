use super::*;
use crate::server::run::reload;

const FIRST_MARKER: u64 = 1000;
const RELOADS: u64 = 64;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_snapshots_keep_config_and_generation_atomic_during_reloads() {
    let (prefix, config) = test_config(&source(FIRST_MARKER), &[]);
    let state = test_state(&config);
    let config_path = prefix.join("conf/nginx.conf");
    let (pairs_tx, mut pairs_rx) = mpsc::unbounded_channel();
    let (stop_tx, stop_rx) = watch::channel(false);

    let mut readers = Vec::new();
    for _ in 0..16 {
        let state = state.clone();
        let pairs = pairs_tx.clone();
        let mut stop = stop_rx.clone();
        readers.push(tokio::spawn(async move {
            while !*stop.borrow() {
                let snapshot = state.config_snapshot();
                let marker = snapshot.config.http.as_ref().unwrap().servers[0]
                    .context
                    .client_max_body_size;
                pairs
                    .send((marker, snapshot.upstream_keepalive_generation))
                    .unwrap();
                tokio::select! {
                    _ = tokio::time::sleep(Duration::from_micros(100)) => {}
                    _ = stop.changed() => {}
                }
            }
        }));
    }
    drop(pairs_tx);

    for revision in 1..=RELOADS {
        let marker = FIRST_MARKER + revision;
        std::fs::write(&config_path, source(marker)).unwrap();
        reload(&state, &config_path, &prefix).await;
        let snapshot = state.config_snapshot();
        assert_eq!(snapshot.upstream_keepalive_generation, revision);
        assert_eq!(
            config_marker(&snapshot.config),
            marker,
            "reload did not publish its configuration"
        );
    }
    tokio::time::sleep(Duration::from_millis(50)).await;
    stop_tx.send(true).unwrap();
    for reader in readers {
        reader.await.unwrap();
    }

    let mut observed = std::collections::HashSet::new();
    while let Some((marker, generation)) = pairs_rx.recv().await {
        assert_eq!(
            marker,
            FIRST_MARKER + generation,
            "configuration {marker} was paired with keepalive generation {generation}"
        );
        observed.insert(marker);
    }
    assert!(
        observed.contains(&(FIRST_MARKER + RELOADS)),
        "readers never observed the final reloaded configuration"
    );
    assert!(observed.len() > 1, "readers did not overlap reloads");
    let _ = std::fs::remove_dir_all(prefix);
}

fn config_marker(config: &RuntimeConfig) -> u64 {
    config.http.as_ref().unwrap().servers[0]
        .context
        .client_max_body_size
}

fn source(marker: u64) -> String {
    format!(
        r#"
            worker_processes 1;
            events {{ worker_connections 64; }}
            http {{
                client_max_body_size {marker};
                upstream backend {{
                    server 127.0.0.1:18180;
                    keepalive 2;
                }}
                server {{
                    listen 127.0.0.1:18724;
                    location / {{
                        proxy_pass http://backend;
                        proxy_http_version 1.1;
                    }}
                }}
            }}
        "#
    )
}
