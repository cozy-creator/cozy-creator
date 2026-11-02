/* Fixture only: call the real Storage.resize_ and inspect the callback marker while
 * PyDLL still holds the GIL. No allocator interception, fake OOM or Python headers. */
#include <stdint.h>
#include <string.h>

typedef struct _object PyObject;
extern int PyGILState_Check(void);
extern PyObject *PyObject_CallMethod(PyObject *, const char *, const char *, ...);
extern void Py_DecRef(PyObject *);
extern void PyErr_Fetch(PyObject **, PyObject **, PyObject **);
extern void PyErr_Clear(void);
extern PyObject *PyObject_Str(PyObject *);
extern const char *PyUnicode_AsUTF8(PyObject *);

struct resize_result {
    int gil_before;
    int gil_after;
    int failed;
    int callback_marker;
    char error[2048];
};

int resize_holding_gil(PyObject *storage, uint64_t bytes,
                       volatile int *callback_marker, struct resize_result *out) {
    memset(out, 0, sizeof(*out));
    out->gil_before = PyGILState_Check();
    if (!out->gil_before) return -1;
    PyObject *result = PyObject_CallMethod(storage, "resize_", "K", (unsigned long long)bytes);
    if (result) {
        Py_DecRef(result);
    } else {
        PyObject *kind = 0, *value = 0, *trace = 0;
        out->failed = 1;
        PyErr_Fetch(&kind, &value, &trace);
        PyObject *text = value ? PyObject_Str(value) : 0;
        const char *message = text ? PyUnicode_AsUTF8(text) : 0;
        if (message) strncpy(out->error, message, sizeof(out->error) - 1);
        if (text) Py_DecRef(text);
        if (kind) Py_DecRef(kind);
        if (value) Py_DecRef(value);
        if (trace) Py_DecRef(trace);
        PyErr_Clear();
    }
    out->callback_marker = *callback_marker;
    out->gil_after = PyGILState_Check();
    return out->gil_after ? 0 : -2;
}
