# Bundled filesystem provider

Cozy embeds the unmodified securefs 2.0.0 release executable. Its MIT license is in
`LICENSE.securefs.md`; upstream source and build scripts are available at
<https://github.com/netheril96/securefs/tree/v2.0.0>.

The upstream build manifest names Argon2, Crypto++, TCLAP, SQLite, Abseil,
uni-algo and Protocol Buffers. The upstream build also supports mimalloc and
uses doctest for its own build tests. Their redistribution notices are retained
in `licenses/`; `licenses/sources.json` records the source and digest of each
notice. SQLite is dedicated to the public domain:
<https://www.sqlite.org/copyright.html>.

The Linux archive deliberately uses the **dynamic** upstream build. FUSE2,
glibc and the C++ runtime are independently installed system libraries, not
bundled copies. The bundled Linux helper requires glibc 2.34 or newer; ordinary
Cozy projects retain Cozy's existing platform floor. The static musl/FUSE
archive is not part of this distribution.

WinFsp is a separately installed Windows filesystem driver; its own license
and installation terms apply. Native macOS storage uses Apple's system hdiutil
and APFS implementation, not this bundled executable or a macFUSE driver. Cozy
never silently changes boot/security policy.

The optional `COZY_SECUREFS_PROVIDER` environment variable selects an explicit
local securefs executable before the bundled one. It permits using a locally
rebuilt provider or relinking a modified system library; the supported
securefs command/format version is still checked. It is a local executable
choice, never a path supplied by TensorHub or a project invitation. No runtime
network download of executable code is performed.
