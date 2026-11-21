#define _GNU_SOURCE
#include <dlfcn.h>
#include <errno.h>
#include <fcntl.h>
#include <limits.h>
#include <stdarg.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <sys/uio.h>
#include <unistd.h>

static const char *configured_match(void) {
    const char *match = getenv("STARLING_FAULT_MATCH");
    return match == NULL || *match == '\0' ? NULL : match;
}

static int operation_enabled(const char *wanted) {
    const char *operations = getenv("STARLING_FAULT_OPS");
    return operations != NULL && strstr(operations, wanted) != NULL;
}

static int configured_errno(void) {
    const char *value = getenv("STARLING_FAULT_ERRNO");
    return value == NULL || *value == '\0' ? ENOSPC : atoi(value);
}

static int path_matches(const char *path) {
    const char *match = configured_match();
    return match != NULL && strstr(path, match) != NULL;
}

static int fd_matches(int fd) {
    char link[64];
    char path[PATH_MAX];
    snprintf(link, sizeof(link), "/proc/self/fd/%d", fd);
    ssize_t length = readlink(link, path, sizeof(path) - 1);
    if (length < 0) {
        return 0;
    }
    path[length] = '\0';
    return path_matches(path);
}

static int should_fail_fd(int fd, const char *operation) {
    return operation_enabled(operation) && fd_matches(fd);
}

static int should_fail_path(const char *path, const char *operation) {
    return operation_enabled(operation) && path_matches(path);
}

static int should_fail_open(const char *path, int dirfd, const char *operation) {
    if (!operation_enabled(operation)) {
        return 0;
    }
    if (path_matches(path)) {
        return 1;
    }
    return path[0] != '/' && dirfd != AT_FDCWD && fd_matches(dirfd);
}

static int open_needs_mode(int flags) {
    return (flags & O_CREAT) != 0 || (flags & O_TMPFILE) == O_TMPFILE;
}

int open(const char *path, int flags, ...) {
    static int (*real_open)(const char *, int, ...);
    if (real_open == NULL) {
        real_open = dlsym(RTLD_NEXT, "open");
    }
    va_list arguments;
    va_start(arguments, flags);
    mode_t mode = open_needs_mode(flags) ? va_arg(arguments, mode_t) : 0;
    va_end(arguments);
    if (should_fail_open(path, AT_FDCWD, "open")) {
        errno = configured_errno();
        return -1;
    }
    if (open_needs_mode(flags)) {
        return real_open(path, flags, mode);
    }
    return real_open(path, flags);
}

int open64(const char *path, int flags, ...) {
    static int (*real_open64)(const char *, int, ...);
    if (real_open64 == NULL) {
        real_open64 = dlsym(RTLD_NEXT, "open64");
    }
    va_list arguments;
    va_start(arguments, flags);
    mode_t mode = open_needs_mode(flags) ? va_arg(arguments, mode_t) : 0;
    va_end(arguments);
    if (should_fail_open(path, AT_FDCWD, "open")) {
        errno = configured_errno();
        return -1;
    }
    if (open_needs_mode(flags)) {
        return real_open64(path, flags, mode);
    }
    return real_open64(path, flags);
}

int openat(int dirfd, const char *path, int flags, ...) {
    static int (*real_openat)(int, const char *, int, ...);
    if (real_openat == NULL) {
        real_openat = dlsym(RTLD_NEXT, "openat");
    }
    va_list arguments;
    va_start(arguments, flags);
    mode_t mode = open_needs_mode(flags) ? va_arg(arguments, mode_t) : 0;
    va_end(arguments);
    if (should_fail_open(path, dirfd, "open")) {
        errno = configured_errno();
        return -1;
    }
    if (open_needs_mode(flags)) {
        return real_openat(dirfd, path, flags, mode);
    }
    return real_openat(dirfd, path, flags);
}

int openat64(int dirfd, const char *path, int flags, ...) {
    static int (*real_openat64)(int, const char *, int, ...);
    if (real_openat64 == NULL) {
        real_openat64 = dlsym(RTLD_NEXT, "openat64");
    }
    va_list arguments;
    va_start(arguments, flags);
    mode_t mode = open_needs_mode(flags) ? va_arg(arguments, mode_t) : 0;
    va_end(arguments);
    if (should_fail_open(path, dirfd, "open")) {
        errno = configured_errno();
        return -1;
    }
    if (open_needs_mode(flags)) {
        return real_openat64(dirfd, path, flags, mode);
    }
    return real_openat64(dirfd, path, flags);
}

int creat(const char *path, mode_t mode) {
    static int (*real_creat)(const char *, mode_t);
    if (real_creat == NULL) {
        real_creat = dlsym(RTLD_NEXT, "creat");
    }
    if (should_fail_open(path, AT_FDCWD, "open")) {
        errno = configured_errno();
        return -1;
    }
    return real_creat(path, mode);
}

int creat64(const char *path, mode_t mode) {
    static int (*real_creat64)(const char *, mode_t);
    if (real_creat64 == NULL) {
        real_creat64 = dlsym(RTLD_NEXT, "creat64");
    }
    if (should_fail_open(path, AT_FDCWD, "open")) {
        errno = configured_errno();
        return -1;
    }
    return real_creat64(path, mode);
}

ssize_t write(int fd, const void *buffer, size_t count) {
    static ssize_t (*real_write)(int, const void *, size_t);
    if (real_write == NULL) {
        real_write = dlsym(RTLD_NEXT, "write");
    }
    if (should_fail_fd(fd, "write")) {
        errno = configured_errno();
        return -1;
    }
    return real_write(fd, buffer, count);
}

ssize_t pwrite(int fd, const void *buffer, size_t count, off_t offset) {
    static ssize_t (*real_pwrite)(int, const void *, size_t, off_t);
    if (real_pwrite == NULL) {
        real_pwrite = dlsym(RTLD_NEXT, "pwrite");
    }
    if (should_fail_fd(fd, "write")) {
        errno = configured_errno();
        return -1;
    }
    return real_pwrite(fd, buffer, count, offset);
}

ssize_t pwrite64(int fd, const void *buffer, size_t count, off64_t offset) {
    static ssize_t (*real_pwrite64)(int, const void *, size_t, off64_t);
    if (real_pwrite64 == NULL) {
        real_pwrite64 = dlsym(RTLD_NEXT, "pwrite64");
    }
    if (should_fail_fd(fd, "write")) {
        errno = configured_errno();
        return -1;
    }
    return real_pwrite64(fd, buffer, count, offset);
}

ssize_t writev(int fd, const struct iovec *iov, int iovcnt) {
    static ssize_t (*real_writev)(int, const struct iovec *, int);
    if (real_writev == NULL) {
        real_writev = dlsym(RTLD_NEXT, "writev");
    }
    if (should_fail_fd(fd, "write")) {
        errno = configured_errno();
        return -1;
    }
    return real_writev(fd, iov, iovcnt);
}

int rename(const char *old_path, const char *new_path) {
    static int (*real_rename)(const char *, const char *);
    if (real_rename == NULL) {
        real_rename = dlsym(RTLD_NEXT, "rename");
    }
    if (should_fail_path(old_path, "rename") || should_fail_path(new_path, "rename")) {
        errno = configured_errno();
        return -1;
    }
    return real_rename(old_path, new_path);
}

int renameat(int olddirfd, const char *old_path, int newdirfd, const char *new_path) {
    static int (*real_renameat)(int, const char *, int, const char *);
    if (real_renameat == NULL) {
        real_renameat = dlsym(RTLD_NEXT, "renameat");
    }
    if (should_fail_path(old_path, "rename") || should_fail_path(new_path, "rename")) {
        errno = configured_errno();
        return -1;
    }
    return real_renameat(olddirfd, old_path, newdirfd, new_path);
}

int renameat2(int olddirfd, const char *old_path, int newdirfd, const char *new_path,
              unsigned int flags) {
    static int (*real_renameat2)(int, const char *, int, const char *, unsigned int);
    if (real_renameat2 == NULL) {
        real_renameat2 = dlsym(RTLD_NEXT, "renameat2");
    }
    if (should_fail_path(old_path, "rename") || should_fail_path(new_path, "rename")) {
        errno = configured_errno();
        return -1;
    }
    return real_renameat2(olddirfd, old_path, newdirfd, new_path, flags);
}

int fsync(int fd) {
    static int (*real_fsync)(int);
    if (real_fsync == NULL) {
        real_fsync = dlsym(RTLD_NEXT, "fsync");
    }
    if (should_fail_fd(fd, "fsync")) {
        errno = configured_errno();
        return -1;
    }
    return real_fsync(fd);
}

int fdatasync(int fd) {
    static int (*real_fdatasync)(int);
    if (real_fdatasync == NULL) {
        real_fdatasync = dlsym(RTLD_NEXT, "fdatasync");
    }
    if (should_fail_fd(fd, "fsync")) {
        errno = configured_errno();
        return -1;
    }
    return real_fdatasync(fd);
}
