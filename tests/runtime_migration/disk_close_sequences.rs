use std::fs;
use std::io::Write;
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use super::disk_fault_support::{
    EIO, FaultServer, FaultSpec, cache_config, compiler_available, response_config, store_config,
};
use super::support::{TestDirectory, fake_upstream, request, reserve_port};

#[test]
#[ignore = "requires local TCP bind and a C compiler; run with --ignored"]
fn close_faults_preserve_cache_store_publication_and_reload_generations() {
    if compiler_available().is_none() {
        eprintln!("skipping disk-close sequence migration: cc is not available");
        return;
    }

    cache_close_fault_publishes_entry();
    store_close_fault_publishes_target();
    reload_during_close_faults_keeps_generations_isolated();
}

fn cache_close_fault_publishes_entry() {
    let port = reserve_port();
    let body = "c".repeat(128 * 1024);
    let Some(upstream) = fake_upstream(&body) else {
        return;
    };
    let config = cache_config(port, upstream.port).replace(
        "server_name localhost;",
        "server_name localhost;\n\
         add_header X-Cache-Status $upstream_cache_status always;",
    );
    let directory = TestDirectory::new("close-cache-publish");
    let Some(mut server) = FaultServer::start(
        directory,
        port,
        &config,
        FaultSpec {
            path: "/cache/",
            error: EIO,
            operations: "close",
        },
    ) else {
        return;
    };

    let first = request(port, "/cache");
    let (head, response_body) = first
        .split_once("\r\n\r\n")
        .expect("cache close-fault headers");
    assert!(
        head.starts_with("HTTP/1.1 200 OK\r\n"),
        "cache close failure changed the first response:\n{first}"
    );
    assert_eq!(
        response_body, body,
        "cache close failure changed the first body"
    );
    upstream.captured();
    wait_for_cache_hit(port, &body);
    server.assert_running();
}

fn store_close_fault_publishes_target() {
    let port = reserve_port();
    let body = "s".repeat(128 * 1024);
    let Some(upstream) = fake_upstream(&body) else {
        return;
    };
    let directory = TestDirectory::new("close-store-publish");
    let Some(mut server) = FaultServer::start(
        directory,
        port,
        &store_config(port, upstream.port),
        FaultSpec {
            path: "/html/",
            error: EIO,
            operations: "close",
        },
    ) else {
        return;
    };

    let response = request(port, "/stored");
    assert!(
        response.starts_with("HTTP/1.1 200 OK\r\n"),
        "store close failure changed the response:\n{response}"
    );
    assert!(
        response.ends_with(&body),
        "store close failure truncated the response"
    );
    upstream.captured();
    wait_for_store_file(&server.temp_path("html/stored"), &body);
    server.assert_running();
}

fn reload_during_close_faults_keeps_generations_isolated() {
    let port = reserve_port();
    let body = "r".repeat(128 * 1024);
    let Some(upstream) = fake_upstream(&body) else {
        return;
    };
    let directory = TestDirectory::new("close-reload-generation");
    let Some(mut server) = FaultServer::start(
        directory,
        port,
        &response_config(port, upstream.port),
        FaultSpec {
            path: "/proxy-temp/",
            error: EIO,
            operations: "close,unlink",
        },
    ) else {
        return;
    };

    let mut client = TcpStream::connect(("127.0.0.1", port)).expect("connect reload spill client");
    socket2::SockRef::from(&client)
        .set_recv_buffer_size(1024)
        .unwrap();
    client
        .write_all(b"GET /spill HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .unwrap();
    wait_for_files(
        &server.temp_path("proxy-temp"),
        "old-generation reload spill",
    );

    let reloaded = response_config(port, upstream.port)
        .replace(
            "proxy_temp_path proxy-temp;",
            "proxy_temp_path proxy-temp-next;",
        )
        .replace(
            "server_name localhost;",
            "server_name localhost;\n\
             location = /version { return 200 \"v2\"; }",
        );
    server.write_config(&reloaded);
    server.reload();
    wait_for_version(port);
    reset_client(client);
    thread::sleep(Duration::from_millis(300));

    upstream.captured();
    server.assert_running();
    assert!(
        !files_under(&server.temp_path("proxy-temp")).is_empty(),
        "old-generation unlink failure should remain observable"
    );
    assert!(
        files_under(&server.temp_path("proxy-temp-next")).is_empty(),
        "reload cancellation polluted the new temp root"
    );
}

fn wait_for_cache_hit(port: u16, body: &str) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let response = request(port, "/cache");
        if response.starts_with("HTTP/1.1 200 OK\r\n")
            && response.ends_with(body)
            && response
                .to_ascii_lowercase()
                .contains("x-cache-status: hit")
        {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "cache entry was not published after a close failure:\n{response}"
        );
        thread::sleep(Duration::from_millis(20));
    }
}

fn wait_for_store_file(path: &Path, body: &str) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if fs::read(path).is_ok_and(|contents| contents == body.as_bytes()) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "store target was not published after a close failure: {}",
            path.display()
        );
        thread::sleep(Duration::from_millis(20));
    }
}

fn wait_for_version(port: u16) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let response = request(port, "/version");
        if response.starts_with("HTTP/1.1 200 OK\r\n") && response.ends_with("v2") {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "reload did not activate while the old spill was in flight:\n{response}"
        );
        thread::sleep(Duration::from_millis(20));
    }
}

fn reset_client(client: TcpStream) {
    socket2::SockRef::from(&client)
        .set_linger(Some(Duration::ZERO))
        .unwrap();
    drop(client);
}

fn wait_for_files(root: &Path, label: &str) {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        if !files_under(root).is_empty() {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "{label} did not create a file under {}",
            root.display()
        );
        thread::sleep(Duration::from_millis(20));
    }
}

fn files_under(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let Ok(entries) = fs::read_dir(root) else {
        return files;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            files.extend(files_under(&path));
        } else {
            files.push(path);
        }
    }
    files
}
