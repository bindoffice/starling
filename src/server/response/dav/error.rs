use super::*;

pub(super) fn status(code: u16) -> Response {
    error_response(code)
}

pub(super) fn io_error(error: &io::Error, not_found_status: u16) -> Response {
    let status = match error.kind() {
        io::ErrorKind::NotFound | io::ErrorKind::NotADirectory => not_found_status,
        io::ErrorKind::PermissionDenied => 403,
        io::ErrorKind::AlreadyExists => 405,
        io::ErrorKind::DirectoryNotEmpty => 409,
        _ if error.raw_os_error() == Some(libc::ENOSPC) => 507,
        _ => 500,
    };
    status_response(status)
}

pub(super) fn status_response(code: u16) -> Response {
    error_response(code)
}
