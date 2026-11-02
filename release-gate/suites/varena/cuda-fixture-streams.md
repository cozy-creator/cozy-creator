# Explicit-stream fixture successor

Owner: /root/async_integration_review. Branch codex/xs138-cuda-fixture-streams starts from
fetched master0888d1026 plus frozen source/evidence186ead3b7. This changes fixtures only;
prior sources/receipts stay intact. No local GPU operation or rental was performed.

NVIDIA's current [synchronization contract](https://docs.nvidia.com/cuda/cuda-driver-api/api-sync-behavior.html)
does not make pageable host-to-device return a destination-completion certificate, and
device memset can return before completion. Existing copy_from_host plus commit remains
safe because commit joins; it was not mislabeled broken. The unsafe fixture assumption
was using commit_completed immediately after pageable copy_from_host without that proof.

The successor initializes constant weights with cuMemsetD8Async on an owned nonblocking
stream, checks that stream's completion, then calls commit_completed. Gate initialization
uses cuStreamWriteValue32_v2 on the retained control stream and checks completion before
any waiter can observe it. Gate signal/wait entry points use the current CUDA13 aliases.

The standalone cache-pages helper owns a nonblocking setup stream. All its initialization,
ordinary release fences and corresponding completion checks use that stream. It has no
cuCtxSynchronize helper. Explicit gated consumer streams retain their existing separate
finish/event checks, so setup cannot release a gate or mask live-A/pending-B behavior.
Synchronous device-to-host readback is retained: that operation completes its copy before
returning, and the actual producers were already joined by their own stream/event proof.

The six async Fill cases, seven raw Source cases and cache/retirement/Plan cases keep their
assertions, byte patterns and operation scopes. No public C/Rust examples, production
allocator/Source code, Python API or global memory policy is edited. All affected GPU
cases require execution against this successor; compilation is not GPU qualification.
