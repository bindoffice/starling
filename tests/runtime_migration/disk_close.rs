use std::fs;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::Path;
use std::thread;
use std::time::Duration;

use super::disk_fault_support::{
    EIO, FaultServer, FaultSpec, compiler_available, request_body_config, response_config,
    wait_for_no_files,
};
use super::support::{TestDirectory, fake_upstream, request, reserve_port};

#[test]
#[ignore = "requires local TCP bind and a C compiler; run with --ignored"]
fn close_faults_preserve_responses_and_cleanup_attempts() {
    if compiler_available().is_none() {
        eprintln!("skipping disk-close migration: cc is not available");
        return;
    }

    response_spill_close_eio_keeps_the_complete_body();
    request_body_close_eio_keeps_the_response_and_cleans_temp();
    close_and_unlink_faults_leave_only_cleanup_residue();
    reset_during_close_and_unlink_faults_keeps_the_worker_usable();
    graceful_shutdown_and_reset_with_close_faults_exit_cleanly();
}

fn response_spill_close_eio_keeps_the_complete_body() {
    let port = reserve_port();
    let body = "c".repeat(128 * 1024);
    let Some(upstream) = fake_upstream(&body) else {
        return;
    };
    let directory = TestDirectory::new("close-response-spill");
    let Some(mut server) = FaultServer::start(
        directory,
        port,
        &response_config(port, upstream.port),
        FaultSpec {
            path: "/proxy-temp/",
            error: EIO,
            operations: "close",
        },
    ) else {
        return;
    };

    let response = request(port, "/spill");
    let (head, response_body) = response
        .split_once("\r\n\r\n")
        .expect("close-fault response headers");
    assert!(
        head.starts_with("HTTP/1.1 200 OK\r\n"),
        "close failure rewrote the committed response:\n{response}"
    );
    assert_eq!(
        response_body, body,
        "close failure changed the downstream response"
    );
    upstream.captured();
    server.assert_running();
    wait_for_no_files(&server.temp_path("proxy-temp"), "response spill close");
}

fn request_body_close_eio_keeps_the_response_and_cleans_temp() {
    let port = reserve_port();
    let directory = TestDirectory::new("close-request-body");
    let config = request_body_config(port);
    let Some(mut server) = FaultServer::start(
        directory,
        port,
        &config,
        FaultSpec {
            path: "/body-temp/",
            error: EIO,
            operations: "close",
        },
    ) else {
        return;
    };

    let response = post_large(port, "/upload", 128 * 1024);
    assert!(
        response.starts_with("HTTP/1.1 204 "),
        "request body close failure changed the response:\n{response}"
    );
    server.assert_running();
    wait_for_no_files(&server.temp_path("body-temp"), "request body close");
}

fn close_and_unlink_faults_leave_only_cleanup_residue() {
    let port = reserve_port();
    let body = "r".repeat(128 * 1024);
    let Some(upstream) = fake_upstream(&body) else {
        return;
    };
    let directory = TestDirectory::new("close-unlink-response-spill");
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

    let response = request(port, "/spill");
    let (head, response_body) = response
        .split_once("\r\n\r\n")
        .expect("close/unlink-fault response headers");
    assert!(
        head.starts_with("HTTP/1.1 200 OK\r\n"),
        "close/unlink failures rewrote the committed response:\n{response}"
    );
    assert_eq!(
        response_body, body,
        "close/unlink failures changed the downstream response"
    );
    upstream.captured();
    server.assert_running();
    assert!(
        !files_under(&server.temp_path("proxy-temp")).is_empty(),
        "the unlink fault should leave the cleanup failure observable"
    );
}

fn reset_during_close_and_unlink_faults_keeps_the_worker_usable() {
    let port = reserve_port();
    let body = "x".repeat(128 * 1024);
    let Some(upstream) = fake_upstream(&body) else {
        return;
    };
    let directory = TestDirectory::new("close-unlink-client-reset");
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

    let mut client = TcpStream::connect(("127.0.0.1", port)).expect("connect spill client");
    socket2::SockRef::from(&client)
        .set_recv_buffer_size(1024)
        .unwrap();
    client
        .write_all(b"GET /spill HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .unwrap();
    wait_for_files(&server.temp_path("proxy-temp"), "client reset spill");
    reset_client(client);

    thread::sleep(Duration::from_millis(250));
    server.assert_running();
    let health = request(port, "/health");
    assert!(
        health.starts_with("HTTP/1.1 404 "),
        "worker did not serve a request after reset cleanup failures:\n{health}"
    );
    assert!(
        !files_under(&server.temp_path("proxy-temp")).is_empty(),
        "the unlink fault should leave reset cleanup residue observable"
    );
}

fn graceful_shutdown_and_reset_with_close_faults_exit_cleanly() {
    let port = reserve_port();
    let body = "q".repeat(128 * 1024);
    let Some(upstream) = fake_upstream(&body) else {
        return;
    };
    let directory = TestDirectory::new("close-unlink-sigquit");
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

    let mut client = TcpStream::connect(("127.0.0.1", port)).expect("connect shutdown client");
    socket2::SockRef::from(&client)
        .set_recv_buffer_size(1024)
        .unwrap();
    client
        .write_all(b"GET /spill HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .unwrap();
    wait_for_files(&server.temp_path("proxy-temp"), "SIGQUIT spill");

    server.shutdown_gracefully();
    reset_client(client);
    server.wait_for_exit(Duration::from_secs(5));
    upstream.captured();
    assert!(
        !files_under(&server.temp_path("proxy-temp")).is_empty(),
        "unlink failure during SIGQUIT should leave cleanup residue observable"
    );
}

fn reset_client(client: TcpStream) {
    socket2::SockRef::from(&client)
        .set_linger(Some(Duration::ZERO))
        .unwrap();
    drop(client);
}

fn post_large(port: u16, target: &str, length: usize) -> String {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect request body client");
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    stream
        .set_write_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    write!(
        stream,
        "POST {target} HTTP/1.1\r\nHost: localhost\r\nContent-Length: {length}\r\n\
         Connection: close\r\n\r\n"
    )
    .unwrap();
    if let Err(error) = stream.write_all(&vec![b'z'; length]) {
        assert!(
            matches!(
                error.kind(),
                std::io::ErrorKind::BrokenPipe
                    | std::io::ErrorKind::ConnectionReset
                    | std::io::ErrorKind::ConnectionAborted
            ),
            "write request body: {error}"
        );
    }
    let mut response = Vec::new();
    stream.read_to_end(&mut response).unwrap();
    String::from_utf8_lossy(&response).into_owned()
}

fn wait_for_files(root: &Path, label: &str) {
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    loop {
        if !files_under(root).is_empty() {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "{label} did not create a file under {}",
            root.display()
        );
        thread::sleep(Duration::from_millis(20));
    }
}

fn files_under(root: &Path) -> Vec<std::path::PathBuf> {
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
