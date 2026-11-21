use std::io;

use socket2::Socket;

use crate::config::ListenSocketOptions;

#[cfg(any(target_os = "android", target_os = "linux"))]
pub(super) fn apply_tcp_listener_options(
    socket: &Socket,
    options: &ListenSocketOptions,
) -> io::Result<()> {
    use std::os::fd::AsRawFd;

    if let Some(fastopen) = options.fastopen {
        let value = i32::try_from(fastopen).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "fastopen queue length exceeds platform limit",
            )
        })?;
        set_int_option(
            socket.as_raw_fd(),
            libc::IPPROTO_TCP,
            libc::TCP_FASTOPEN,
            value,
        )?;
    }
    if options.deferred {
        // Nginx uses a one-second deferred queue because queue age is not observable.
        set_int_option(
            socket.as_raw_fd(),
            libc::IPPROTO_TCP,
            libc::TCP_DEFER_ACCEPT,
            1,
        )?;
    }
    Ok(())
}

#[cfg(any(target_os = "android", target_os = "linux"))]
fn set_int_option(fd: std::os::fd::RawFd, level: i32, name: i32, value: i32) -> io::Result<()> {
    // SAFETY: fd is an open socket owned by the caller, and the pointer/size
    // describe a live c_int for the duration of the setsockopt call.
    let result = unsafe {
        libc::setsockopt(
            fd,
            level,
            name,
            (&value as *const i32).cast(),
            std::mem::size_of_val(&value) as libc::socklen_t,
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(not(any(target_os = "android", target_os = "linux")))]
pub(super) fn apply_tcp_listener_options(
    _socket: &Socket,
    options: &ListenSocketOptions,
) -> io::Result<()> {
    if options.fastopen.is_some() {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "fastopen is not supported on this platform",
        ));
    }
    if options.deferred {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "deferred accept is not supported on this platform",
        ));
    }
    Ok(())
}
