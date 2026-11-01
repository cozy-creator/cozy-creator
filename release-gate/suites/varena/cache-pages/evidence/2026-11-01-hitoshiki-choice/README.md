# Completed Cache pages participate in native choice

Runs 25401 (red, native e545) and 25403 (green, native d3) use the identical c7f1b214
fixture/controller and parent a96/7ad SDK on Hitoshiki. Mixed-source build receipts
are preserved in ../choice-pair-build. Exactly two native files differ: working/cache.rs
and working/split.rs. Neither arm changes the request, legal menu or managed ceiling.

Both correctly choose half (4MiB) while 2MiB pages totalling 4MiB are live or pending
behind an actual blocked release fence. After observed completion, those Cache pages
remain physically mapped and reclaimable. Under the same 8MiB ceiling:

- Red 25401 still reports 4MiB available and chooses half. The expected larger-choice
  error and child exit 1 remain preserved; the run's Report is false.
- Green 25403 reports 8MiB available and chooses whole. Native split begin actually
  unmaps the completed Cache pages, the real 8MiB allocation is fully read back, its
  native working phase publishes 8MiB peak/backing peak, and all managed physical bytes,
  Cache reservations, pools and events return to zero before child exit 0.

Both native children load neither Python nor Torch. PID/birth/pidfd and full source/
binary identities were independently checked, along with exact raw-result extraction.
This proves the bounded preferred-choice correction, not callback completion or full
model performance. Parent a96 contains the primitive and deliberately excludes d3;
the executing native choice cores are the separately linked helper binaries.
