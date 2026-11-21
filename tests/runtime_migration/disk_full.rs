use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

use super::disk_fault_support::{
    EDQUOT, EIO, ENOSPC, EROFS, FaultServer, FaultSpec, assert_complete_stream, cache_config,
    compiler_available, request_body_config, response_config, store_config, wait_for_no_files,
};
use super::support::{TestDirectory, fake_upstream, request, reserve_port};

#[test]
#[ignore = "requires local TCP bind and a C compiler; run with --ignored"]
fn write_faults_preserve_stream_semantics_and_clean_temp_files() {
    if compiler_available().is_none() {
        eprintln!("skipping disk-full migration: cc is not available");
        return;
    }

    for (error, label) in [
        (ENOSPC, "ENOSPC"),
        (EDQUOT, "EDQUOT"),
        (EROFS, "EROFS"),
        (EIO, "EIO"),
    ] {
        response_spill_write_fault_truncates_after_commit(error, label);
        cache_write_fault_keeps_downstream_complete(error, label);
        store_write_fault_keeps_downstream_complete(error, label);
        request_body_write_fault_maps_to_500(error, label);
    }
}

fn response_spill_write_fault_truncates_after_commit(error: i32, label: &str) {
    let port = reserve_port();
    let body = "x".repeat(128 * 1024);
    let Some(upstream) = fake_upstream(&body) else {
        return;
    };
    let directory = TestDirectory::new(&format!("response-{label}"));
    let Some(mut server) = FaultServer::start(
        directory,
        port,
        &response_config(port, upstream.port),
        FaultSpec {
            path: "/proxy-temp/",
            error,
            operations: "write",
        },
    ) else {
        return;
    };

    let response = request(port, "/spill");
    let (head, response_body) = response
        .split_once("\r\n\r\n")
        .expect("write-fault response headers");
    assert!(
        head.starts_with("HTTP/1.1 200 OK\r\n"),
        "{label} committed response was rewritten:\n{response}"
    );
    assert!(
        response_body.len() < body.len(),
        "{label} did not truncate the spilled response"
    );
    assert!(
        !response_body.is_empty(),
        "the committed prefix should reach the downstream"
    );
    upstream.captured();
    server.assert_running();
    wait_for_no_files(&server.temp_path("proxy-temp"), label);
}

fn cache_write_fault_keeps_downstream_complete(error: i32, label: &str) {
    let port = reserve_port();
    let body = "c".repeat(128 * 1024);
    let Some(upstream) = fake_upstream(&body) else {
        return;
    };
    let directory = TestDirectory::new(&format!("cache-{label}"));
    let Some(mut server) = FaultServer::start(
        directory,
        port,
        &cache_config(port, upstream.port),
        FaultSpec {
            path: "/cache/",
            error,
            operations: "write",
        },
    ) else {
        return;
    };

    assert_complete_stream(port, "/cache", &body, &format!("cache {label}"));
    upstream.captured();
    server.assert_running();
    wait_for_no_files(&server.temp_path("cache"), label);
}

fn store_write_fault_keeps_downstream_complete(error: i32, label: &str) {
    let port = reserve_port();
    let body = "s".repeat(128 * 1024);
    let Some(upstream) = fake_upstream(&body) else {
        return;
    };
    let directory = TestDirectory::new(&format!("store-{label}"));
    let Some(mut server) = FaultServer::start(
        directory,
        port,
        &store_config(port, upstream.port),
        FaultSpec {
            path: "/html/",
            error,
            operations: "write",
        },
    ) else {
        return;
    };

    assert_complete_stream(port, "/stored", &body, &format!("store {label}"));
    upstream.captured();
    server.assert_running();
    wait_for_no_files(&server.temp_path("html"), label);
    assert!(
        !server.temp_path("html/stored").exists(),
        "store published a target after {label}"
    );
}

fn request_body_write_fault_maps_to_500(error: i32, label: &str) {
    let port = reserve_port();
    let directory = TestDirectory::new(&format!("request-body-{label}"));
    let Some(mut server) = FaultServer::start(
        directory,
        port,
        &request_body_config(port),
        FaultSpec {
            path: "/body-temp/",
            error,
            operations: "write",
        },
    ) else {
        return;
    };

    let response = post_large(port, "/upload", 128 * 1024);
    assert!(
        response.starts_with("HTTP/1.1 500 "),
        "request body spool {label} should map to 500:\n{response}"
    );
    server.assert_running();
    wait_for_no_files(&server.temp_path("body-temp"), label);
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
