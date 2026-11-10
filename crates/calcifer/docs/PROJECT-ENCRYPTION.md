# Optional local project encryption

All projects use canonical Automerge state inside `project.db`; media stay separate files.
Encryption is optional. Ordinary projects need no account, password or mount driver.
Multiplayer, encrypted hosted backup and trusted collaboration hosting are later phases.

## Basic use

Enable encryption before importing or generating private data:

```sh
cozy project new /absolute/path/MyProject
cozy project -p /absolute/path/MyProject encrypt --recovery-out /separate/device/recovery.txt --remove-plaintext
cozy project -p /absolute/path/MyProject vault status
cozy project -p /absolute/path/MyProject lock
cozy project -p /absolute/path/MyProject unlock
```

The CLI prompts for a passphrase. Automation uses `--passphrase-file FILE` or stdin (`-`),
never a literal secret argument. Recovery output must be a new file, created with owner-only
access before its first secret write. `unlock --recovery-file FILE` uses the independent saved
recovery secret. The WebUI offers local copy, download, print and QR. Save recovery off-device;
TensorHub never receives it. Recovery also needs a copy of the encrypted project/keybag: a
secret alone cannot recreate lost files.

Unlock reports a temporary readable view. Use that path in Explorer/Finder or a media player;
Calcifer and the WebUI resolve it automatically. The registered project folder holds encrypted
backing and a wrapped keybag. Do not edit backing files directly.

## Locking and copies

Unlock defaults to 15 minutes of UI/CLI inactivity. `--idle SECONDS` changes it (zero disables
idle expiry); `--max-duration SECONDS` adds an absolute limit. Polling and hover previews never
renew it. Intentional WebUI playback does; external native players do not.

Lock blocks new WebUI/CLI reads and viewing streams, then drains work and normally unmounts.
Active work or native handles can leave it **locking/busy**. Finish/cancel work or close native
apps and retry. A readable owned mount is never acknowledged as locked. Normal Calcifer
shutdown also closes views and refuses while they are busy.

Earlier exports, backups, source inputs, ordinary shared-cache copies and external apps' caches
are not erased. Without `--remove-plaintext`, conversion preserves the verified original and
reports its path. For an explicit plaintext copy, choose a new directory outside both backing
and readable view:

```sh
cozy project -p /absolute/path/MyProject vault export --output /absolute/path/PlainCopy
```

The existing `project push` protocol uploads plaintext and refuses encrypted projects. Hosted
encrypted backup is not implemented in this local phase. For now, copy backing and keybag
while locked when you need a ciphertext backup.

## Recovery changes and rekey

`vault passphrase` rewraps the passphrase slot. `vault recovery --recovery-out NEWFILE` replaces
the saved recovery credential. Neither changes file encryption or revokes independent older
copies. `vault rotate --recovery-out NEWFILE` prompts for a new passphrase and rewrites retained
files/history with a fresh project key. Close native apps first; old managed ciphertext is
retired only after replacement verification. Save the new recovery code. If interrupted before
receiving it, use the new passphrase to reopen the replacement and replace recovery again.
A durable receipt recovers interrupted directory cutover. Nested projects and unknown symlinks
refuse conversion rather than risk unrelated data.

## Platform and execution boundaries

Linux/Windows use pinned bundled securefs. Linux encryption needs FUSE2 and glibc 2.34+; ordinary
CLI requirements remain unchanged. Windows needs WinFsp 2.1 and the Microsoft Visual C++ x64
runtime; Windows 11 emulates the filesystem and ffmpeg/ffprobe helpers on ARM64.
macOS uses built-in encrypted APFS images,
without a kernel extension. Logical state/media are portable, but raw APFS backing needs macOS;
raw securefs backing needs its compatible provider. Cross-format moves currently require an
explicit decoded transfer. APFS does not claim authenticated per-file encryption semantics.

Do not browse intermediate Windows conversion drives: this provider does not enforce read-only
mounts. Forcibly killing both controller and native guardian can leave an APFS image attached
until ejected or the OS restarts. Normal lock verifies complete cleanup.

Private local execution protects source, inputs, outputs, logs, caches and temporary files.
Only kernel IPC sockets use an owner-protected temporary endpoint: the encrypted regular-file
provider cannot host them, and their message bytes are not stored in the socket inode.
Unsupported private local model-file imports and warm sets refuse shared-cache operations.
Already-trusted remote workers retain ordinary plaintext execution/scratch over pinned TLS;
they receive neither the local project storage key nor recovery secret.
