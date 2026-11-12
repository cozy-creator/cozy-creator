# Project data containment

Required by Paul, 2026-11-11. This is the implementation contract for the active audit, not a statement that the current RC already meets it.

## Ownership

All project data belongs in the project's directory. For an encrypted project, durable contents belong inside its encrypted filesystem. This includes run identities and history, inputs, outputs, logs and errors, retry records, derived caches, source, execution scratch, browser project preferences and private execution bookkeeping. An encrypted outside journal or a hash-only outside execution index is still a violation.

Calcifer may retain a project reference consisting of its opaque identity, location and opaque route code. Names, last-use times, walking state, run counters and project-derived cache entries are not part of that exception. Unrelated installation, account and machine state remains independent; it must not contain project/run associations or content.

Project ownership is explicit before any submission or storage write. A locked, unknown or unavailable project must refuse; it cannot fall through to global maintenance state. Project run numbers are scoped to the project, while existing opaque run identities remain stable.

## Required storage and lifecycle

- Keep the complete controller journal in the existing `project.db`, alongside project state. Private worker journals, objects, code, temporary files and caches must use the verified project view. No public/global journal receives a private row or pointer.
- Keep live routing and view caches in memory only. Do not persist a second run index, operation tombstone, request replay, project usage record or mixed project log globally.
- On lock or deletion, stop admission, quiesce the project's work and readers, commit its own history, close all owned processes/mappings/database handles, retire live views and then close the encrypted filesystem. Failed cleanup is a failure or pending state, never a successful lock/deletion receipt.
- Resume project work after restart only through its authorized project storage. A closed project contributes only its permitted reference entry. Moving/copying a project carries its complete retained history.
- Refuse execution or output paths that cannot enforce the boundary before accepting or uploading private data. Temporary-directory variables and process resource limits are not filesystem write confinement.
- Global diagnostics must contain no project/run identifiers, paths, payloads, source errors or arbitrary tool output. Retain project diagnostics through that project's storage or a transient authorized response.
- Project HTTP responses are non-cacheable, including ordinary project media, errors, ranges and replays. Application browser state must not persist project data outside the project. Browser history/profile ownership is an explicit policy decision, not something JavaScript can silently erase.

## Existing history and migration

Preserve existing records and identities. Copy a project's complete legacy record closure into its admitted storage, verify the durable result and record the import receipt there before retiring exactly matching global records. No global backup/quarantine may retain project data. A crash during migration must preserve the verified source or an idempotently recoverable committed project copy.

Locked, unavailable or ambiguously owned legacy records require a real ownership/unlock resolution. Do not discard them, guess their owner, or claim full containment while a retained legacy source still holds them. SQLite cleanup includes controlled WAL/journal/free-page copies; private filesystem cleanup includes managed temporary and rotated files.

Existing private payload bytes already live inside the project cipher; global SQL currently retains their pointers and other run metadata. Both the remaining metadata and genuinely outside plaintext copies must be addressed.

## Proof and limits

Use real CLI-created encrypted projects and a real local CPU function on an owned hosted fixture. Cover new and legacy storage, retry/restart/crash, move, lock, deletion, diagnostic failures, late browser responses and incapable workers. Scan raw and decoded application-controlled files/databases for private canaries and their IDs/derived forms, exempting only the project's own storage and exact reference columns. Receipts must not print private canary values or create their own outside copies.

The application can control its own writes and supported execution paths. It cannot retroactively guarantee forensic erasure of SSD sectors, OS swap/snapshots, independent backups, or records made by external applications. Deliberate exports, recovery copies and normal-browser history need an explicit boundary; they are not silently counted as cleanup successes.

Current implementation findings and exact interface proposals are recorded in the task audit receipts. A source-only audit does not qualify migration of live user data; the real migration and failure paths must pass first.
