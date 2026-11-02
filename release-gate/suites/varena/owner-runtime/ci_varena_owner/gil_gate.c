/* Release-gate helper only. PyDLL retains the GIL; verify it before signalling. */
#include <poll.h>
#include <unistd.h>

extern int PyGILState_Check(void);

int hold_gil(void) {
    if (!PyGILState_Check()) return -1;
    const char ready[] = "{\"gil_gate\":true,\"gil_held\":1}\n";
    if (write(STDOUT_FILENO, ready, sizeof(ready) - 1) != sizeof(ready) - 1) return -2;
    struct pollfd input = { .fd = STDIN_FILENO, .events = POLLIN };
    if (poll(&input, 1, 10000) != 1) return -3; /* failing harness guard, never a pass */
    char release;
    if (read(STDIN_FILENO, &release, 1) != 1 || release != 'x') return -4;
    return PyGILState_Check() ? 0 : -5;
}
