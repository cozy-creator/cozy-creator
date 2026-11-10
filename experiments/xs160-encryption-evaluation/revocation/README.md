# Local PRE revocation probe

Synthetic local proof for the project-sharing research. This executes real `recrypt` 0.16.0 and AES-GCM 0.10.3, while explicitly simulating an authorization server with a local map. It does not connect to TensorHub or use real project/user data. It does not implement a production wire format, identity binding, key vault, rollback protection, or the complete IronCore group protocol.

Run from this directory:

```bash
CARGO_TARGET_DIR="$PWD/target" cargo +1.99.1 run --locked --release --jobs 2
CARGO_TARGET_DIR="$PWD/target" cargo +1.99.1 run --locked --release --jobs 2 --features api480
```

`src/main.rs` has all assertions. `Cargo.lock` fixes the dependency cohort. `run256.stdout` and `run480.stdout` contain passes and payload hashes; corresponding `.stderr` files record the build/run. `results.json` includes compiler details, source/lock hashes, executed commands and exit statuses. `provenance.json` captures registry and upstream metadata. Third-party audit/source reading evidence is kept locally outside this published branch; citations are in the tracker research report.

The first compile failed because `NonEmptyVec` has no `.iter()` method; `run.failed-first-compile.stderr` preserves it. The corrected code uses `.to_vec().iter()`. `run.stdout` is the empty stdout from that failed attempt.

Validated:

- Member Alice decrypts with her own key; ungranted Bob cannot request a transform or directly decrypt the group capsule.
- Granting Bob access transforms only the small key capsule; all 4 MiB of ciphertext stays byte-identical.
- The simulated honest proxy rejects subsequent Bob requests after removing the grant.
- Bob's cached transformed capsule or cached DEK still decrypts the old object offline.
- A fresh independent object key is not opened by Bob's old DEK.
- A dishonest proxy retaining Bob's transform key enables him to decrypt a fresh capsule under the same project key.
- A fresh project keypair excludes the old transform key from new-epoch objects, while a new Alice grant works.
- The published augmentation primitives make administrator/server shares individually insufficient to decrypt, while the augmented transform permits Alice to decrypt.

The component-byte counts sum the public API's cryptographic field lengths, excluding container/wire-format framing. The 256-bit API run is a functional comparison, not a recommended current security parameter. The 480-bit run proves availability of that API, not its current cryptanalytic strength.

Publish source, lock, logs, results and provenance if desired; `target/` is only a local build cache.

Product decision after the experiment: forward-only exclusion is sufficient for v1, and old accessed content may remain readable. Use project-key epochs with lazy re-encryption of changed content; PRE is an alternative for later online key-delivery requirements, not a required product dependency.
