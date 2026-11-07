# Platform signals

The process-local Signal, Waitable, wait_any and Ready implementation shared by
Varena and `platform::memory`. Unix uses the existing nonblocking socket pair and
rustix poll; Windows uses the existing manual-reset event and wait set. Their
implementation files are moved byte-for-byte from platform.

This crate depends only on the OS bindings needed for those operations. It does
not acquire platform's installer/fetch features when a worker enables them.
Otherwise shared Varena could absorb AWS-LC's Rust dependency metadata while its
native C symbols remain unavailable to statically linked worker consumers.

`platform::memory` directly re-exports these types/functions. The signal error
retains the same operation text and original io::Error, and converts to both
io::Error and platform::Error for existing callers. No wake, wait, clear,
readiness, error-errno or retirement behavior is changed.
