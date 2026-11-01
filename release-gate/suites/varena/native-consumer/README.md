# Standalone native cooperation consumer

This release-gate helper links Varena directly, without Python or Torch. It allocates and
verifies real CUDA working memory, can own the native automatic service or act as a manual
idle donor, and reports actual allocator/driver facts. The metadata-only watcher observes an
exact donor row in `Waiting` for an exact requester; it neither reaps rows nor credits memory.

The ordinary Cozy fixture controller will kill only its own donor process after that observed
state and a matching PID/birth identity. A deadline without the marker is a failed check, never
authorization to kill or evidence of donor death. Reaping is not evidence of physical release.

Source/build work is in progress. Real GPU proof is pending. All helpers belong to release-gate;
no production API, allocator, daemon or published tool is changed here.
