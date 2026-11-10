# Local project mounts

`project-mount` owns optional encrypted project storage and the process that
keeps its readable view alive. The keybag is another module: it supplies only a
random 43-character internal unlock secret through stdin. No provider receives
the user's passphrase, recovery secret or Hub credentials. Ordinary projects
load no provider and require no driver.

Linux and Windows use securefs 2.0.0 lite format. Their releases embed a pinned
upstream executable and redistribution notices. Linux's dynamic provider needs
glibc 2.34+ and FUSE2; Windows needs WinFsp 2.1 and the Microsoft Visual C++ x64
runtime. The x64 helper runs under Windows11 emulation on ARM64. Windows read-only
requests are explicitly refused because this provider does not enforce them.

macOS uses its built-in `hdiutil` and an AES-256-encrypted, case-sensitive APFS
sparsebundle. No macFUSE extension, boot-policy change or additional driver is
required. The virtual volume is 16 TiB and grows with actual writes; that capacity
is not preallocated. The internal password is supplied through stdin, never
arguments, environment or a keychain-store operation. The format marker is
`.cozy-apfs-v1`; the image is `project.sparsebundle` under encrypted backing.

The **logical Automerge format is identical on every OS**, but physical storage
formats differ. A raw APFS vault copy requires macOS. A raw securefs copy needs a
compatible securefs provider. Wrong-format opens refuse clearly. Cross-OS
movement uses decoded project export/transfer; future portable logical backup
is independent of the local storage provider. Do not claim a common raw
ciphertext fixture opens on every OS.

The executable calls `project_mount::run_supervisor()` before normal CLI parsing.
An encrypted mount starts that private role in the same executable. It watches
the exact controller process birth and its pipe, and releases its provider/image
after controller death. Unix views retain a sealed original mountpoint inode;
Windows uses an unused drive root. Normal locks preserve busy handles. Emergency
cleanup acts only on the owned provider or image/device association.

Release builds run `scripts/prepare-securefs.py`; Linux/Windows then require
`COZY_SECUREFS_ARCHIVE` and `COZY_SECUREFS_REQUIRED=1`. The archive is verified
against `helper/manifest.json`; extraction needs no runtime network. Mac builds
use the OS-native provider. Explicit local `COZY_SECUREFS_PROVIDER` overrides
permit compatible rebuilt securefs providers; see [notices](helper/NOTICE.md).

The manual `dev-build` option `vault-provider` runs actual mounted file I/O,
SQLite transactions, media seek, normal lock/busy states, remount parity and
controller SIGKILL. Its [execution evidence](../../e2e/vault-provider/RESULTS.md)
states which backends actually passed. Compile success is not driver, filesystem
or installed-CLI qualification. Calcifer owns session idle/absolute expiry;
native APFS explicitly refuses an independent provider-idle timer.

The earlier FSKit route was exercised with signed modules explicitly enabled
only in the disposable test user, then restored. It failed actual file creation
with ENOSYS78 before write/fsync (run42804959021), so no fsync or data-durability
check was waived to make it pass. Native APFS replaces that incompatible route.
Debug builds alone can write explicitly requested provider stderr diagnostics
with `COZY_VAULT_PROVIDER_DIAGNOSTICS=<private-log-file>`; release builds expose
no diagnostic log channel.
