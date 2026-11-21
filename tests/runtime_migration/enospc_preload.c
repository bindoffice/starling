#define _GNU_SOURCE
#include <dlfcn.h>
#include <errno.h>
#include <limits.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/uio.h>
#include <unistd.h>

static int should_fail(int fd) {
    const char *match = getenv("STARLING_ENOSPC_MATCH");
    if (match == NULL || *match == '\0') {
        return 0;
    }
    char link[64];
    char path[PATH_MAX];
    snprintf(link, sizeof(link), "/proc/self/fd/%d", fd);
    ssize_t length = readlink(link, path, sizeof(path) - 1);
    if (length < 0) {
        return 0;
    }
    path[length] = '\0';
    return strstr(path, match) != NULL;
}

ssize_t write(int fd, const void *buffer, size_t count) {
    static ssize_t (*real_write)(int, const void *, size_t);
    if (real_write == NULL) {
        real_write = dlsym(RTLD_NEXT, "write");
    }
    if (should_fail(fd)) {
        errno = ENOSPC;
        return -1;
    }
    return real_write(fd, buffer, count);
}

ssize_t pwrite(int fd, const void *buffer, size_t count, off_t offset) {
    static ssize_t (*real_pwrite)(int, const void *, size_t, off_t);
    if (real_pwrite == NULL) {
        real_pwrite = dlsym(RTLD_NEXT, "pwrite");
    }
    if (should_fail(fd)) {
        errno = ENOSPC;
        return -1;
    }
    return real_pwrite(fd, buffer, count, offset);
}

ssize_t pwrite64(int fd, const void *buffer, size_t count, off64_t offset) {
    static ssize_t (*real_pwrite64)(int, const void *, size_t, off64_t);
    if (real_pwrite64 == NULL) {
        real_pwrite64 = dlsym(RTLD_NEXT, "pwrite64");
    }
    if (should_fail(fd)) {
        errno = ENOSPC;
        return -1;
    }
    return real_pwrite64(fd, buffer, count, offset);
}

ssize_t writev(int fd, const struct iovec *iov, int iovcnt) {
    static ssize_t (*real_writev)(int, const struct iovec *, int);
    if (real_writev == NULL) {
        real_writev = dlsym(RTLD_NEXT, "writev");
    }
    if (should_fail(fd)) {
        errno = ENOSPC;
        return -1;
    }
    return real_writev(fd, iov, iovcnt);
}
