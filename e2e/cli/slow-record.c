// LD_PRELOAD into Calcifer (watch_restart, cl-885): its lock record's rename into place waits 5 s, as on a busy disk.
#define _GNU_SOURCE
#include <dlfcn.h>
#include <string.h>
#include <unistd.h>

int rename(const char *from, const char *to) {
    static int (*real)(const char *, const char *);
    static const char record[] = "/calcifer/calcifer.lock.holder";
    if (!real) *(void **)&real = dlsym(RTLD_NEXT, "rename");
    size_t n = strlen(to), m = sizeof record - 1;
    if (n >= m && strcmp(to + n - m, record) == 0) sleep(5);
    return real(from, to);
}
