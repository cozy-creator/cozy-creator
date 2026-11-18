// LD_PRELOAD into TensorD (answers_while_committing, cl-1202): while the file SLOW_SYNC_ON names exists, each fsync or fdatasync of
// the machine's journal (executions.sqlite3 and its WAL) waits SLOW_SYNC_MS (default 3000) first, as a commit behind a busy disk's
// writeback does. Each wait is a line of SLOW_SYNC_LOG.
#define _GNU_SOURCE
#include <dlfcn.h>
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include <unistd.h>

static void slow(int fd) {
    const char *on = getenv("SLOW_SYNC_ON");
    if (!on || access(on, F_OK) != 0) return;
    char link[64], path[4096];
    snprintf(link, sizeof link, "/proc/self/fd/%d", fd);
    ssize_t n = readlink(link, path, sizeof path - 1);
    if (n <= 0) return;
    path[n] = 0;
    if (!strstr(path, "/executions.sqlite3")) return;
    const char *said = getenv("SLOW_SYNC_MS");
    long ms = said ? atol(said) : 3000;
    struct timespec left = {ms / 1000, (ms % 1000) * 1000000L};
    while (nanosleep(&left, &left) == -1 && errno == EINTR) {}
    const char *log = getenv("SLOW_SYNC_LOG");
    if (!log) return;
    char line[96];
    int len = snprintf(line, sizeof line, "%d %ld\n", getpid(), ms);
    int out = open(log, O_WRONLY | O_CREAT | O_APPEND | O_CLOEXEC, 0600);
    if (out >= 0) { ssize_t w = write(out, line, (size_t)len); (void)w; close(out); }
}

int fsync(int fd) {
    static int (*real)(int);
    if (!real) *(void **)&real = dlsym(RTLD_NEXT, "fsync");
    slow(fd);
    return real(fd);
}

int fdatasync(int fd) {
    static int (*real)(int);
    if (!real) *(void **)&real = dlsym(RTLD_NEXT, "fdatasync");
    slow(fd);
    return real(fd);
}
