// LD_PRELOAD into Calcifer (state_restart, cl-1010): state.db on a cold cache of a busy disk. The first read of each 4 KiB page
// waits BUSY_DISK_US (default 50 µs, ~80 MB/s of random reads); a page read before is in the page cache and free. BUSY_DISK_LOG
// names a file that says how many reads waited.
#define _GNU_SOURCE
#include <dlfcn.h>
#include <fcntl.h>
#include <stdarg.h>
#include <stdatomic.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/types.h>
#include <unistd.h>

#define FDS 65536
#define PAGES (1u << 22) // 16 GiB of 4 KiB pages
static atomic_uchar store_fd[FDS];
static atomic_uchar touched[PAGES / 8];
static atomic_ulong waited;

static int is_store(const char *path) {
    static const char name[] = "/calcifer/state.db";
    size_t n = strlen(path), m = sizeof name - 1;
    return n >= m && strcmp(path + n - m, name) == 0;
}

static void mark(int fd, const char *path) {
    if (fd >= 0 && fd < FDS) atomic_store(&store_fd[fd], path && is_store(path));
}

static void cold(int fd, off_t offset, size_t count) {
    if (fd < 0 || fd >= FDS || !atomic_load(&store_fd[fd]) || offset < 0) return;
    static long us = -1;
    if (us < 0) { const char *said = getenv("BUSY_DISK_US"); us = said ? atol(said) : 50; }
    unsigned long first = (unsigned long)offset / 4096, last = ((unsigned long)offset + (count ? count - 1 : 0)) / 4096, fresh = 0;
    for (unsigned long page = first; page <= last && page < PAGES; page++) {
        unsigned char bit = (unsigned char)(1u << (page % 8));
        if (!(atomic_fetch_or(&touched[page / 8], bit) & bit)) fresh++;
    }
    if (!fresh) return;
    usleep((useconds_t)(us * (long)fresh));
    unsigned long n = atomic_fetch_add(&waited, fresh) + fresh;
    const char *log = getenv("BUSY_DISK_LOG");
    if (log && (n / 1000 != (n - fresh) / 1000)) {
        static int (*real_open)(const char *, int, ...);
        if (!real_open) *(void **)&real_open = dlsym(RTLD_NEXT, "open64");
        char line[64];
        int len = snprintf(line, sizeof line, "%d %lu\n", getpid(), n);
        int out = real_open(log, O_WRONLY | O_CREAT | O_APPEND | O_CLOEXEC, 0600);
        if (out >= 0) { ssize_t w = write(out, line, (size_t)len); (void)w; close(out); }
    }
}

#define OPEN(fn, ...) \
    mode_t mode = 0; \
    if (flags & (O_CREAT | O_TMPFILE)) { va_list ap; va_start(ap, flags); mode = (mode_t)va_arg(ap, int); va_end(ap); } \
    static int (*real)(__VA_ARGS__, int, ...); \
    if (!real) *(void **)&real = dlsym(RTLD_NEXT, fn);

int open(const char *path, int flags, ...) { OPEN("open", const char *) int fd = real(path, flags, mode); mark(fd, path); return fd; }
int open64(const char *path, int flags, ...) { OPEN("open64", const char *) int fd = real(path, flags, mode); mark(fd, path); return fd; }
int openat(int dir, const char *path, int flags, ...) { OPEN("openat", int, const char *) int fd = real(dir, path, flags, mode); mark(fd, path); return fd; }
int openat64(int dir, const char *path, int flags, ...) { OPEN("openat64", int, const char *) int fd = real(dir, path, flags, mode); mark(fd, path); return fd; }

int close(int fd) {
    static int (*real)(int);
    if (!real) *(void **)&real = dlsym(RTLD_NEXT, "close");
    mark(fd, NULL);
    return real(fd);
}

ssize_t pread(int fd, void *buf, size_t count, off_t offset) {
    static ssize_t (*real)(int, void *, size_t, off_t);
    if (!real) *(void **)&real = dlsym(RTLD_NEXT, "pread");
    cold(fd, offset, count);
    return real(fd, buf, count, offset);
}

ssize_t pread64(int fd, void *buf, size_t count, off_t offset) {
    static ssize_t (*real)(int, void *, size_t, off_t);
    if (!real) *(void **)&real = dlsym(RTLD_NEXT, "pread64");
    cold(fd, offset, count);
    return real(fd, buf, count, offset);
}

ssize_t read(int fd, void *buf, size_t count) {
    static ssize_t (*real)(int, void *, size_t);
    if (!real) *(void **)&real = dlsym(RTLD_NEXT, "read");
    if (fd >= 0 && fd < FDS && atomic_load(&store_fd[fd])) cold(fd, lseek(fd, 0, SEEK_CUR), count);
    return real(fd, buf, count);
}
