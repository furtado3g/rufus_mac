# Windows media native resources

macOS-first implementation. `manifest.json` pins build inputs, not a claim of
working host binaries or Secure Boot compatibility. No runtime downloads or
user-installed wimlib/NTFS tools are permitted. Linux/Windows builds remain
pending and must not be reported as tested.

## Sources and license review

| Input | Reviewed notice | Distribution obligations |
| --- | --- | --- |
| libudfread 1.2.0 | `src/udfread.c`: LGPL-2.1-or-later; root `COPYING`: LGPL 2.1; `meson.build`: MIT | Include notices, complete matching source, modifications and build inputs. For static linking under LGPL, supply application objects and instructions sufficient to relink with a modified library; preserve replacement/debugging rights. |
| NTFS-3G 2026.9.28 | `libntfs-3g/volume.c`, `ntfsprogs/mkntfs.c`: GPL-2.0-or-later; root includes `COPYING` and `COPYING.LIB` | Deliver exact selected library/formatter sources, our callback adaptations, notices and build scripts under GPL-compatible terms. Do not infer a blanket LGPL grant from `COPYING.LIB`. |
| wimlib-imagex 1.14.5 | Root `COPYING`: helper GPL-3.0-or-later; LGPL option applies to libwim only where dependencies allow it | Deliver helper source, enabled dependency sources/notices, patches and build recipes. Build without FUSE/NTFS-3G integration. Audit actual linked libraries before packaging. |
| UEFI:NTFS image | Pinned Rufus readme identifies loader 2.8, signed NTFS driver 1.9 and EfiFs 1.12 | Preserve the image byte-for-byte. T024 must identify exact component sources, notices, signatures and architectures. Do not assume the image has one uniform license. |

This is an initial source review, not a completed audit of every compiled file
or embedded image component. Release remains blocked until that inventory is
complete. Project GPL-3.0-or-later does not waive third-party obligations.

## Source delivery policy

T037 must publish, alongside each binary release, a matching source bundle
containing the pinned input archives, dependency sources, license/copyright
files, local patches, build scripts, compiler/configuration metadata and the
application revision. Include LGPL relink materials where required. Record
SHA-256 for those bundles and for each compiled helper per host architecture.
An upstream URL alone is not the source-delivery mechanism. Do not publish
binary releases if the corresponding bundle or required relink materials are
missing. Preserve these materials for every distributed release.

## Build boundaries

Native macOS link check (after building the resources):

```sh
sh scripts/build-windows-media.sh
export IMAGE2USB_WINDOWS_MEDIA_PREFIX="$PWD/target/windows-media/$(uname -m)/install"
cargo test -p image2usb-platform --features windows-media --test media
cargo clippy -p image2usb-platform --all-targets --features windows-media -- -D warnings
```

The opt-in feature links native archives; it does not enable Windows media
creation. The prefix must be absolute. The developer script downloads and
compiles pinned sources under `target/`; the installed application never does.
Default builds remain unchanged on other platforms.

Current macOS recipe status: libudfread, libntfs-3g and wimlib-imagex 1.14.5
build on Intel. The script is incremental: `autoreconf` runs only while
`configure` is absent, and `configure` runs only while the build directory has
no `Makefile`. Delete the per-source build directory to force reconfiguration.
It also builds a developer-only `ntfsinfo` into the private prefix for read-only
validation. Neither `ntfsinfo` nor the fixture examples are shipped.

## Local mkntfs adaptations

`crates/platform/native/ntfs_format.c` compiles the pinned mkntfs sources with
a private device-operations implementation. The build script patches
`ntfsprogs/mkntfs.c` with exact-match assertions:

1. replace the default device allocation with
   `i2u_ntfs_device_alloc(opts.dev_name)`, which uses the current callback
   context instead of opening a host path;
2. reset the global allocation list, `g_upcaseinfo` and stale geometry during
   cleanup, because mkntfs is a one-shot program and the application may format
   more than one volume in a process.

The shim declares the replacement through
`crates/platform/native/ntfs_format_override.h`, resets getopt state between
calls, and is serialized by a Rust mutex. `-Dlibintl_setlocale=setlocale`
avoids a host `/usr/local/include/libintl.h` redirect to an unresolvable
libintl symbol; the resulting archive has no libintl runtime dependency.
Any change to these sources or flags requires re-auditing the patch and the
retained callback error/label/geometry behavior.

## NTFS writer API

`crates/platform/src/media.rs` exposes `NtfsVolume` and `NtfsFile` behind
`windows-media`. The API mounts only the caller's partition-sized `RawIo`, so
offsets are partition-relative and no host path is opened. It supports directory
creation (including parents), file creation/open, positional read/write,
truncation, close, device sync and unmount.

The process-wide NTFS lock is held for the lifetime of a mounted volume because
libntfs-3g uses global caches. Do not call `format_ntfs` while a volume is
mounted in the same process: the lock is not reentrant and would deadlock.
`NtfsFile` borrows its volume so the volume cannot be unmounted under an open
file. Paths reject absolute components, traversal, backslash, controls,
`<>:"|?*`, trailing dot/space and names over 255 UTF-16 units. Creation does not
overwrite an existing name. If forced native unmount fails, `Drop` deliberately
leaks the native handle and callback box instead of leaving a retained pointer
to freed Rust memory.

The macOS tests cover nested directories and a 5 MiB file, a fully populated
4,296,015,872-byte file, a sparse file above 4 GiB with reads at 64-bit offsets,
4096-byte-capped short I/O, injected write/sync cancellation, flush and
unmount. They do not establish Windows `chkdsk` acceptance or boot behavior.

## Private helper boundary

`Helper::resolve` takes an absolute private prefix and a relative helper path,
rejects traversal and symlink escapes, checks that the result is a regular
executable and verifies a caller-supplied SHA-256. The digest is checked again
immediately before execution. `Helper::run` uses an absolute program path and
separate arguments with a cleared environment; it never invokes a shell or
looks up `PATH`.

`PrivateWorkspace` creates a unique `tempfile` directory with owner-only mode,
exclusive 0600 files and no deletion of pre-existing paths. Helper stdout and
stderr go to files inside that workspace. Cancellation kills and waits for the
child before returning, so workspace cleanup cannot race a live helper. The
build/release recipe still needs to publish the per-architecture hashes for the
actual `wimlib-imagex`; Windows ACL hardening remains part of the Windows host
work. The API deliberately has no default or development hash.

## Developer-only fixtures

```sh
cargo run -p image2usb-platform --features windows-media \
  --example ntfs_fixture -- target/windows-media/ntfs-fixture.img 64
target/windows-media/x86_64/install/bin/ntfsinfo -m \
  target/windows-media/ntfs-fixture.img
cargo run -p image2usb-platform --features windows-media \
  --example ntfs_tree -- target/windows-media/ntfs-tree.img 128
target/windows-media/x86_64/install/bin/ntfsinfo -F \
  /sources/nested/data.bin target/windows-media/ntfs-tree.img
cargo run -p image2usb-platform --features windows-media \
  --example ntfs_tree -- target/windows-media/ntfs-large.img 6144 --large
cargo run -p image2usb-platform --features windows-media \
  --example udf_probe -- target/windows-media/udf-sparse.img
```

`ntfs_tree --large` creates a sparse 4 GiB logical file with markers at both
ends. Adding `--fill` writes the whole 4 GiB payload and physically allocates
it; use that only when the host has space and the extra I/O is intended. The
automated UDF fragment test invokes `newfs_udf` and `hdiutil` against a temporary
disk image; it does not use a physical device or root.

These examples only open the given regular file. They never enumerate or write
physical devices. `ntfsinfo` is GPL NTFS-3G tooling and remains a development
artifact under `target/`.

- Build-time downloads are verified against the manifest before extraction or
  execution. Commit archive hashes differ from release-tarball hashes.
- Generated sources, libraries and helpers belong under ignored `target/`.
- T004 builds libudfread with Meson, NTFS-3G without its FUSE driver, and wimlib
  without FUSE or NTFS-3G integration. Build tools are developer prerequisites,
  not end-user dependencies. Never install into a system prefix.
- T006 keeps mkntfs on bounded callbacks; a standalone formatter executable is
  not a substitute. Do not add path-based or FUSE-based formatting. The
  callback adaptation, cleanup reset and getopt reset are required for repeated
  in-process formats.
- T008 must resolve packaged helpers by absolute path and verify their hashes
  in a private workspace. Build success alone does not satisfy this gate.
- T024 copies the signed boot image unchanged; inspect BPB/logical sector
  compatibility and PE signatures independently. Firmware DB/DBX acceptance
  requires real boot evidence.
