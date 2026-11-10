# Local project key custody

Encryption is optional per project. Ordinary projects need no keybag, password, account or
filesystem driver. This crate protects one random 256-bit project filesystem secret, encoded
as 43 printable bytes for the provider's anonymous stdin pipe. The filesystem provider owns
file encryption; private state, history and execution blobs live inside that same filesystem.
There is no second journal key or per-file sharing envelope.

`Keybag::create` makes two independent wrapping slots: the user's passphrase and a generated
256-bit recovery secret with grouping and a transcription checksum. Either opens the same
project key. Save recovery on another device, in a password manager or on paper. TensorHub
never receives it. The keybag is `project.vault.keys.json` beside the encrypted backing;
recovery requires both the saved credential and a copy of the encrypted project/keybag.

Keybag format 2 uses [RustCrypto Argon2id](https://docs.rs/argon2/0.6.0/argon2/) version 19,
256 MiB, three iterations, one lane, and a 32-byte output. Every slot has a fresh random
32-byte salt and 12-byte nonce. [AWS-LC AES-256-GCM](https://docs.rs/aws-lc-rs/1.18.2/aws_lc_rs/aead/)
authenticates the envelope; associated data binds format, suite, project ID, generation,
key identity, slot and fixed KDF costs. The decoded payload repeats its identity. Received
costs, salt/nonce/ciphertext lengths, credential lengths and the complete keybag size are
validated before KDF work. The earlier unreleased age prototype format is explicitly refused.

Credentials, derived keys and decoded payloads zeroize on drop and redact debug output.
Project secrets are not serializable; scoped duplication is explicit. Callers must never put
keys in arguments, environment variables or logs. Recovery disclosure is explicit for local
copy/download/print/QR. Atomic ciphertext writes use `platform::durable`.

Passphrase/recovery replacement changes only its wrapping slot. An older independently copied
keybag and credential still opens the old material. `new_generation` creates a fresh project
key; Calcifer must quiesce work, rewrite every retained file/history blob, verify it, durably
preserve the new keybag, switch storage, and retire the old managed ciphertext. Creating a new
keybag alone does not complete rekeying. Prior plaintext exports, ordinary shared-cache copies,
backups, and copies made by external native apps are outside that rewrite.

`SessionTimer` tracks explicit activity and an optional absolute duration. Polling never renews
it. Expiration is latched and checks monotonic and wall time. Calcifer revokes new reads and
streams, then releases private execution and database handles and normally unmounts. Busy
native applications report locking/busy; a readable native volume is never reported locked.
Native player I/O does not renew Calcifer's UI/CLI idle clock. Close native apps before
conversion/rekey, and do not browse intermediate conversion drives on Windows, whose pinned
provider does not enforce read-only mounts.
