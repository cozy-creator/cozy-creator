# Local project mounts

`project-mount` owns securefs 2.0.0 lite-format mounts. The keybag is another
module: it supplies only the random 43-character internal unlock secret through
stdin. The helper never receives a user's passphrase, recovery secret or Hub
credentials. Ordinary projects do not load a helper or require a driver.

The executable calls `project_mount::run_supervisor()` before normal CLI parsing.
An encrypted mount starts that private role in the same executable. It watches
the exact controller process birth and its pipe, stops the foreground provider
after controller death, and protects the old Unix mountpoint inode against
plaintext writes. Windows uses an unused drive root instead of a writable
directory below a missing mount. A normal Unix lock uses a non-lazy OS unmount
and reports `Busy` while handles prevent it. Emergency crash cleanup may detach
an inaccessible mount only after the key-holding provider has exited.

Release builds run `scripts/prepare-securefs.py` and set
`COZY_SECUREFS_ARCHIVE` plus `COZY_SECUREFS_REQUIRED=1`. The build verifies the
archive against `helper/manifest.json`; the release executable contains the
helper and redistribution notices, and installs them in the user's protected
tools directory without a runtime download. Development builds may use an
explicitly installed provider. See [provider notices](helper/NOTICE.md).

Encrypted-project prerequisites:

- Linux: glibc 2.34+ for the bundled helper, FUSE2 and `/dev/fuse` access.
- macOS: 15.4+ and macFUSE 5.5+ with the FSKit extension enabled. Read-only
  FSKit views additionally require macOS 26.4+. No kernel-extension fallback
  silently changes the user's boot policy.
- Windows: WinFsp 2.1 and the Microsoft Visual C++ x64 runtime. Read-only
  mount requests are explicitly refused because this provider does not enforce
  that mode on Windows; ordinary local encrypted projects are writable. The upstream
  x64 provider runs under Windows 11's x64 emulation on ARM64.

The manual `dev-build` option `vault-provider` builds and runs the real fixture
in [`e2e/vault-provider`](../../e2e/vault-provider). It writes/reads/seeks media,
checks normal and idle locking, remount parity, mountpoint fallback prevention
and controller SIGKILL. Run it separately on each supported OS/architecture;
compilation is not a claim that the driver or installed CLI has been qualified.

Debug builds alone can write explicitly requested provider stderr diagnostics
with `COZY_VAULT_PROVIDER_DIAGNOSTICS=<private-log-file>`; the platform fixture
uses this only with synthetic data. Release builds do not expose that channel.
Filesystem status/unmount controls have bounded acknowledgments; a timeout
reports unconfirmed/pending status and never authorizes killing a live mount.

The macOS CI fixture explicitly enables only the two installed signed macFUSE
FSKit modules for its disposable runner user, records those exact IDs, and
restores the original approval list afterward. This is test setup, not product
behavior. End users enable macFUSE under System Settings → General → Login Items
& Extensions → By Category → File System Extensions. PluginKit registration
alone does not grant FSKit approval. See the upstream [activation issue](https://github.com/macfuse/macfuse/issues/1194).
