# Local project key custody

This crate protects the secrets of an **opt-in encrypted project**. Ordinary projects do not
create a keybag or require a passphrase. The filesystem data format and its mount lifecycle are
owned by the mount provider; this crate does not implement a second filesystem cipher.

`Keybag::create` generates fresh project material and two independent age wrapping slots:

- the user's passphrase;
- a generated 256-bit recovery secret, with grouping and a transcription checksum.

Either slot opens the same project generation. The encrypted payload binds the project ID,
generation, filesystem secret and internal journal identity. The keybag is portable and stored
beside the encrypted backing directory as `project.vault.keys.json`, so a copied vault plus its
saved recovery secret can recover offline. No TensorHub key escrow, recovery email, account
service or personal-account keyring is required by this initial local implementation.

The file-key envelope is the standard [age v1 format](https://age-encryption.org/v1), supplied by
the maintained Rust `age` crate. Passphrase scrypt work is fixed and capped at log2(N)=18,
r=8, p=1 (256 MiB). Keybag and decrypted-payload sizes are bounded before use. Atomic writes
and OS permissions use `platform::durable`.

`ProjectSecret` is not serializable or cloneable, redacts Debug, and zeroizes its owned secrets
on drop. Its explicit byte closure is only for feeding the helper's anonymous stdin pipe.
Callers must not put it in process arguments, environment variables or logs. Recovery
disclosure is explicit through `RecoverySecret::expose` for the local save/print/QR flow.

The same managed generation includes an internal age X25519 journal identity. The keybag
contains its public recipient so private terminal results/events can be sealed while a
viewing session is locked. `ProjectSecret::open` requires the private identity and checks the
project, generation and caller's run/field context before returning zeroizing plaintext.
The caller pins the verified recipient when accepting work; a mutable unauthenticated peer
cannot choose a replacement recipient. This internal key is not another user-managed secret
or a file-sharing permission hierarchy.

Passphrase and recovery replacement only change their wrapping slots. Copied older keybags
and their credentials remain usable for the old material. `new_generation` creates fresh
filesystem and journal secrets; the caller must rewrite retained files and private journal
payloads, verify the replacement, durably preserve its recovery envelope and atomically
activate it. Creating a new keybag alone is not a completed rekey.

`SessionTimer` tracks explicit viewing activity and a separate optional absolute duration.
Status polling does not touch it. Expiration is latched and checks both monotonic and wall
time so a suspend/clock change cannot restore an expired session. It owns no mount or key:
Calcifer must stop private reads, safely handle writes and actually unmount the provider
before reporting the project locked.
