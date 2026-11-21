use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

use super::disk_fault_support::{
    EMFILE, ENFILE, FaultServer, FaultSpec, assert_complete_stream, cache_config,
    compiler_available, dav_config, request_body_config, response_config, store_config,
    wait_for_no_files,
};
use super::support::{TestDirectory, fake_upstream, request, reserve_port};

#[test]
#[ignore = "requires local TCP bind and a C compiler; run with --ignored"]
fn open_faults_preserve_stream_semantics_and_clean_temp_files() {
    if compiler_available().is_none() {
        eprintln!("skipping disk-open migration: cc is not available");
        return;
    }

    for (error, label) in [(EMFILE, "EMFILE"), (ENFILE, "ENFILE")] {
        response_spill_open_fault_truncates_after_commit(error, label);
        cache_open_fault_keeps_downstream_complete(error, label);
        store_open_fault_keeps_downstream_complete(error, label);
        request_body_open_fault_maps_to_500(error, label);
        dav_open_fault_maps_to_500(error, label);
    }
}

fn response_spill_open_fault_truncates_after_commit(error: i32, label: &str) {
    let port = reserve_port();
    let body = "o".repeat(128 * 1024);
    let Some(upstream) = fake_upstream(&body) else {
        return;
    };
    let directory = TestDirectory::new(&format!("open-response-{label}"));
    let Some(mut server) = FaultServer::start(
        directory,
        port,
        &response_config(port, upstream.port),
        FaultSpec {
            path: "/proxy-temp/",
            error,
            operations: "open",
        },
    ) else {
        return;
    };

    let response = request(port, "/spill");
    let (head, response_body) = response
        .split_once("\r\n\r\n")
        .expect("open-fault response headers");
    assert!(
        head.starts_with("HTTP/1.1 200 OK\r\n"),
        "{label} committed response was rewritten after temp-file open failure:\n{response}"
    );
    assert!(
        response_body.len() < body.len(),
        "{label} temp-file open failure did not truncate the response"
    );
    upstream.captured();
    server.assert_running();
    wait_for_no_files(&server.temp_path("proxy-temp"), label);
}

fn cache_open_fault_keeps_downstream_complete(error: i32, label: &str) {
    let port = reserve_port();
    let body = "a".repeat(128 * 1024);
    let Some(upstream) = fake_upstream(&body) else {
        return;
    };
    let directory = TestDirectory::new(&format!("open-cache-{label}"));
    let Some(mut server) = FaultServer::start(
        directory,
        port,
        &cache_config(port, upstream.port),
        FaultSpec {
            path: "/cache/",
            error,
            operations: "open",
        },
    ) else {
        return;
    };

    assert_complete_stream(port, "/cache", &body, &format!("cache open {label}"));
    upstream.captured();
    server.assert_running();
    wait_for_no_files(&server.temp_path("cache"), label);
}

fn store_open_fault_keeps_downstream_complete(error: i32, label: &str) {
    let port = reserve_port();
    let body = "b".repeat(128 * 1024);
    let Some(upstream) = fake_upstream(&body) else {
        return;
    };
    let directory = TestDirectory::new(&format!("open-store-{label}"));
    let Some(mut server) = FaultServer::start(
        directory,
        port,
        &store_config(port, upstream.port),
        FaultSpec {
            path: "/html/",
            error,
            operations: "open",
        },
    ) else {
        return;
    };

    assert_complete_stream(port, "/stored", &body, &format!("store open {label}"));
    upstream.captured();
    server.assert_running();
    wait_for_no_files(&server.temp_path("html"), label);
    assert!(
        !server.temp_path("html/stored").exists(),
        "store published a target after {label}"
    );
}

fn request_body_open_fault_maps_to_500(error: i32, label: &str) {
    let port = reserve_port();
    let directory = TestDirectory::new(&format!("open-request-body-{label}"));
    let Some(mut server) = FaultServer::start(
        directory,
        port,
        &request_body_config(port),
        FaultSpec {
            path: "/body-temp/",
            error,
            operations: "open",
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

fn dav_open_fault_maps_to_500(error: i32, label: &str) {
    let port = reserve_port();
    let directory = TestDirectory::new(&format!("open-dav-{label}"));
    let Some(mut server) = FaultServer::start(
        directory,
        port,
        &dav_config(port),
        FaultSpec {
            path: "/html/",
            error,
            operations: "open",
        },
    ) else {
        return;
    };

    let response = put_body(port, "/dav/file.bin", 64 * 1024);
    assert!(
        response.starts_with("HTTP/1.1 500 "),
        "DAV temp-file {label} should map to 500:\n{response}"
    );
    server.assert_running();
    wait_for_no_files(&server.temp_path("html"), label);
    assert!(
        !server.temp_path("html/file.bin").exists(),
        "DAV published a target after {label}"
    );
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

fn put_body(port: u16, target: &str, length: usize) -> String {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect DAV client");
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    stream
        .set_write_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    write!(
        stream,
        "PUT {target} HTTP/1.1\r\nHost: localhost\r\nContent-Length: {length}\r\n\
         Connection: close\r\n\r\n"
    )
    .unwrap();
    stream.write_all(&vec![b'd'; length]).unwrap();
    let mut response = Vec::new();
    stream.read_to_end(&mut response).unwrap();
    String::from_utf8_lossy(&response).into_owned()
}
