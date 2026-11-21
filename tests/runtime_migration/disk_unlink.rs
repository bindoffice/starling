use std::fs;
use std::path::Path;

use super::disk_fault_support::{EIO, FaultServer, FaultSpec, compiler_available, response_config};
use super::support::{TestDirectory, fake_upstream, request, reserve_port};

#[test]
#[ignore = "requires local TCP bind and a C compiler; run with --ignored"]
fn unlink_fault_leaves_residual_temp_but_keeps_the_worker_alive() {
    if compiler_available().is_none() {
        eprintln!("skipping disk-unlink migration: cc is not available");
        return;
    }

    let port = reserve_port();
    let body = "u".repeat(128 * 1024);
    let Some(upstream) = fake_upstream(&body) else {
        return;
    };
    let directory = TestDirectory::new("unlink-response-spill");
    let Some(mut server) = FaultServer::start(
        directory,
        port,
        &response_config(port, upstream.port),
        FaultSpec {
            path: "/proxy-temp/",
            error: EIO,
            operations: "unlink",
        },
    ) else {
        return;
    };

    let response = request(port, "/spill");
    let (head, response_body) = response
        .split_once("\r\n\r\n")
        .expect("unlink-fault response headers");
    assert!(
        head.starts_with("HTTP/1.1 200 OK\r\n"),
        "unlink failure rewrote the response:\n{response}"
    );
    assert_eq!(
        response_body, body,
        "unlink failure changed the downstream response"
    );
    upstream.captured();
    server.assert_running();
    assert!(
        !files_under(&server.temp_path("proxy-temp")).is_empty(),
        "the unlink fault should leave the cleanup failure observable"
    );
}

#[test]
#[ignore = "requires local TCP bind and a C compiler; run with --ignored"]
fn write_and_unlink_faults_preserve_committed_response_semantics() {
    if compiler_available().is_none() {
        eprintln!("skipping disk-unlink migration: cc is not available");
        return;
    }

    let port = reserve_port();
    let body = "v".repeat(128 * 1024);
    let Some(upstream) = fake_upstream(&body) else {
        return;
    };
    let directory = TestDirectory::new("write-unlink-response-spill");
    let Some(mut server) = FaultServer::start(
        directory,
        port,
        &response_config(port, upstream.port),
        FaultSpec {
            path: "/proxy-temp/",
            error: EIO,
            operations: "write,unlink",
        },
    ) else {
        return;
    };

    let response = request(port, "/spill");
    let (head, response_body) = response
        .split_once("\r\n\r\n")
        .expect("combined-fault response headers");
    assert!(
        head.starts_with("HTTP/1.1 200 OK\r\n"),
        "combined write/unlink failure rewrote the response:\n{response}"
    );
    assert!(
        response_body.len() < body.len(),
        "combined write/unlink failure did not truncate the response"
    );
    upstream.captured();
    server.assert_running();
    assert!(
        !files_under(&server.temp_path("proxy-temp")).is_empty(),
        "the unlink failure should leave cleanup residue observable"
    );
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
