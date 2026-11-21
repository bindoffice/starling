use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

use super::disk_fault_support::{
    EDQUOT, EIO, ENOSPC, FaultServer, FaultSpec, assert_complete_stream, cache_config,
    compiler_available, dav_config, store_config, wait_for_no_files,
};
use super::support::{TestDirectory, fake_upstream, reserve_port};

#[test]
#[ignore = "requires local TCP bind and a C compiler; run with --ignored"]
fn quota_rename_and_fsync_faults_clean_up_without_publishing() {
    if compiler_available().is_none() {
        eprintln!("skipping disk metadata migration: cc is not available");
        return;
    }

    cache_rename_edquot_keeps_downstream_complete();
    store_rename_eio_keeps_downstream_complete();
    dav_fsync_enospc_maps_to_507_and_cleans_temp();
}

fn cache_rename_edquot_keeps_downstream_complete() {
    let port = reserve_port();
    let body = "q".repeat(128 * 1024);
    let Some(upstream) = fake_upstream(&body) else {
        return;
    };
    let directory = TestDirectory::new("edquot-cache-rename");
    let Some(mut server) = FaultServer::start(
        directory,
        port,
        &cache_config(port, upstream.port),
        FaultSpec {
            path: "/cache/",
            error: EDQUOT,
            operations: "rename",
        },
    ) else {
        return;
    };

    assert_complete_stream(port, "/cache", &body, "cache rename EDQUOT");
    upstream.captured();
    server.assert_running();
    wait_for_no_files(&server.temp_path("cache"), "cache rename");
}

fn store_rename_eio_keeps_downstream_complete() {
    let port = reserve_port();
    let body = "m".repeat(128 * 1024);
    let Some(upstream) = fake_upstream(&body) else {
        return;
    };
    let directory = TestDirectory::new("eio-store-rename");
    let Some(mut server) = FaultServer::start(
        directory,
        port,
        &store_config(port, upstream.port),
        FaultSpec {
            path: "/html/",
            error: EIO,
            operations: "rename",
        },
    ) else {
        return;
    };

    assert_complete_stream(port, "/stored", &body, "store rename EIO");
    upstream.captured();
    server.assert_running();
    wait_for_no_files(&server.temp_path("html"), "store rename");
    assert!(
        !server.temp_path("html/stored").exists(),
        "store published a target after a rename failure"
    );
}

fn dav_fsync_enospc_maps_to_507_and_cleans_temp() {
    let port = reserve_port();
    let directory = TestDirectory::new("enospc-dav-fsync");
    let Some(mut server) = FaultServer::start(
        directory,
        port,
        &dav_config(port),
        FaultSpec {
            path: "/html/",
            error: ENOSPC,
            operations: "fsync",
        },
    ) else {
        return;
    };

    let response = put_body(port, "/dav/file.bin", 64 * 1024);
    assert!(
        response.starts_with("HTTP/1.1 507 "),
        "DAV fsync ENOSPC should map to 507:\n{response}"
    );
    server.assert_running();
    wait_for_no_files(&server.temp_path("html"), "DAV fsync");
    assert!(
        !server.temp_path("html/file.bin").exists(),
        "DAV published a target after an fsync failure"
    );
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
