# M5 — build baseline

Date: 2026-10-21. Revision: `056c02731e02de2e29ed232544a9da862fbd10a9`.
Host: macOS 15.8, Intel x86_64. Existing `.gitignore` changes and untracked
`specs/` were preserved. No device writes, dependency installation or boot tests.

## T001 — baseline recorded; cross-platform checks pending

| Command | Result |
| --- | --- |
| `cargo fmt --all --check` | PASS, exit 0 |
| `cargo clippy --workspace --all-targets -- -D warnings` | PASS, exit 0 |
| `cargo test --workspace` | PASS, exit 0; test counts not included in captured output |
| `cargo build --workspace` | PASS, exit 0 |
| `cargo check -p image2usb-core -p image2usb-iso -p image2usb-extract -p image2usb-cli --target x86_64-unknown-linux-gnu` | FAIL, exit 101; E0463, target standard library unavailable |
| `cargo check -p image2usb-core -p image2usb-iso -p image2usb-extract -p image2usb-cli --target x86_64-pc-windows-msvc` | FAIL, exit 101; E0463, target standard library unavailable |
| `rustup show` | FAIL, exit 127; `zsh:1: command not found: rustup` |

Native success is not cross-platform validation. T001 is complete as a baseline
record, not as certification of every platform. The user approved continuing
T002 without installing tools on this Mac. Missing target standard libraries
remain a local limitation; cross-checks must run in an equipped environment.
The user subsequently approved macOS-first delivery: native macOS proofs gate
the local T007 increment; Linux/Windows builds will be performed separately by
the user and remain pending, without blocking macOS implementation. Full
cross-platform completion still requires those results.
No toolchain changes were made automatically.

## Available tools

| Tool | Observed |
| --- | --- |
| cargo | `/opt/local/bin/cargo`, `cargo 1.98.0` |
| rustc | `/opt/local/bin/rustc`, `rustc 1.98.1 (48a229cea 2026-09-01) (built from a source tarball)` |
| rustup | Not found |
| clang / cc | `/usr/bin/clang`, `/usr/bin/cc` |
| cmake / make | `/usr/local/bin/cmake`, `/usr/bin/make` |
| pkg-config | `/usr/local/bin/pkg-config` |
| QEMU | `/opt/local/bin/qemu-system-x86_64` |
| fsck_msdos | `/sbin/fsck_msdos` |
| wimlib-imagex / mkntfs / ntfscp / sgdisk | Not found |

Presence on PATH does not establish tool version, native integration viability,
firmware availability or Secure Boot support. M5 remains incomplete; only the
synthetic US1 increment documented below is implemented.

## T002 — source pins recorded

`assets/windows-media/manifest.json` records SHA-256 and byte sizes computed
from HTTPS response bodies for all four resources on 2026-10-21. Archives were
read in memory, not extracted or executed. Source archive hashes are not hashes
of future compiled host helpers. Host binary hashes must be added after builds.

- libudfread 1.2.0: VideoLAN release archive; inspected COPYING and meson.build.
- NTFS-3G 2026.9.28: tag resolved to exact upstream GitHub commit; inspected
  COPYING and COPYING.LIB. Tuxera release archive returned HTTP 403, so the
  manifest pins the separately hashed commit archive, not the release tarball.
- wimlib 1.14.5: tag resolved to exact upstream GitHub commit; inspected COPYING.
  wimlib.net connection reset; pinned commit archive used instead.
- UEFI:NTFS: exact Rufus asset commit; upstream readme identifies loader 2.8,
  signed NTFS driver 1.9 and EfiFs 1.12. No PE signature or boot verification.

Recipe fields identify future T004/T006/T024 work, not existing build scripts.
Licenses remain subject to file-level audit in T003; exact corresponding source
for the image's components must be pinned before redistribution. Downloads and
hashes establish reproducible input selection, not authenticity or boot trust.

## T003 — licensing and source delivery documented

Added `assets/windows-media/README.md`: reviewed LGPL notice in udfread.c,
GPL-2.0-or-later notices in NTFS volume.c/mkntfs.c, wimlib COPYING, and image
provenance. Defined matching source bundles and LGPL relink delivery gates.
This does not certify licenses of every future compiled file or image component.

## T004 — partial, macOS bootstrap blocked

Command: `sh scripts/build-windows-media.sh` (exit 1).

- All three source archives passed manifest size and SHA-256 checks.
- libudfread 1.2.0 compiled as a static library on macOS x86_64 using Apple
  clang 17.0.0, Meson 1.12.1 and Ninja 1.13.2.
- Installation was restricted to `target/windows-media/x86_64/install`, not a
  system prefix. No developer tools were installed.
- NTFS-3G `autoreconf -fi` failed: `AM_PATH_LIBGCRYPT` not found; Autoconf also
  reported undefined/overquoted `AC_MSG_ERROR` and `AC_MSG_WARN`. Its configure
  generation requires a resolution before resuming. Do not treat a partially
  generated configure script as a successful bootstrap.
- wimlib build was not reached. Rust link integration, formatter adaptation,
  runtime packaging, macOS ARM64 and CI remain pending. T004 stays unchecked;
  no dependent task started.

Runnable input check: `sh scripts/build-windows-media.sh --fetch-only`.
The script rejects unexpected archive sizes/hashes, links, special files and
traversal paths before extraction. Downloaded code is only executed during the
explicit developer build, never by the installed application.

### MacPorts retry

After the user installed MacPorts prerequisites, the recipe now prioritizes
`/opt/local/bin` and `/opt/local/sbin`, with `/opt/local/share/aclocal` in
`ACLOCAL_PATH`. The missing libgcrypt macro was resolved. NTFS-3G compiled as
a static library, but upstream `make install` failed in `install-exec-hook`,
attempting to move `.so` files to `/lib` despite the private prefix. No such
move succeeded. The recipe now overrides `rootlibdir` with the private library
directory; a new build is running. This is not yet a successful T004 result.

The private-prefix retry completed: libudfread, libntfs-3g and wimlib-imagex
compiled on macOS x86_64. `otool -L` on the helper listed only
`/usr/lib/libSystem.B.dylib`. However, wimlib's upstream version script
discovered the application's parent Git repository and reported
`056c0273-dirty`, not `1.14.5`. The recipe now bounds Git discovery at its work
directory and regenerates configure/build metadata on retries. Rebuild pending;
do not package the incorrectly versioned helper. T004 remains partial.

The bounded-Git rebuild succeeded: `wimlib-imagex 1.14.5 (using wimlib 1.14.5)`;
`otool -L` again lists only `/usr/lib/libSystem.B.dylib`. Observed helper size:
485224 bytes; SHA-256:
`7045a51f479cbd6e5a6bba2a5a7dc15bcec2f5e730fd73cba099b81d272dafc3`.
An isolated capture/verify/apply smoke test restored identical bytes. The first
test WIM lacked integrity information, so upstream skipped that integrity-table
check; it is not evidence for detecting WIM corruption. The recipe now includes
the same roundtrip with `capture --check`, to generate integrity information.
Rust integration, formatter callbacks and real Windows-media tests remain pending.

### Rust native link increment

Added opt-in `windows-media` to image2usb-platform, with explicit absolute
`IMAGE2USB_WINDOWS_MEDIA_PREFIX`. Its build script requires native macOS and
both static archives; default builds do not depend on these artifacts.

Executed with prefix `target/windows-media/x86_64/install` resolved absolutely:

- `cargo test -p image2usb-platform --features windows-media --test media`: PASS,
  one test. Calls libudfread version API (1.2.0) and reads NTFS library static
  provenance. Initial check exposed missing library linkage until the test
  referenced image2usb-platform; initial platform-name assertion was corrected
  to the existing `macos` spelling. Final run passed.
- `cargo clippy -p image2usb-platform --all-targets --features windows-media -- -D warnings`:
  PASS.

This proves basic native linkage, not UDF parsing, NTFS formatting, large-file
I/O or ABI callback safety. CI setup and remaining T004 work are still pending.

### CI macOS foundation configured

Added a separate `windows-media-macos` job on `macos-14`, installing MacPorts
2.12.7 from its versioned Sonoma package after checking the SHA-256 published
in the upstream GitHub release asset metadata. It installs developer tools,
builds pinned sources, runs the WIM roundtrip and native Rust test/Clippy.
Existing Linux/Windows/default CI jobs remain unchanged. The new job has not
been executed remotely; no CI PASS or Apple Silicon validation is claimed.
Developer port definitions are synced, not version-pinned; compiler/build-tool
reproducibility still requires recording the actual runner environment.

Local `cargo fmt --all --check` and `git diff --check` passed after this change.
A full recipe rerun including the newly added WIM integrity-table smoke check
is running in the background; results are not yet recorded.

## T005 — partial read-only UDF wrapper

Added `crates/platform/src/media.rs`, macOS-only behind `windows-media`.
The wrapper owns its ISO handle, bounds block reads against captured length,
uses u64 byte offsets and catches callback panics. Native volume/file handles
are closed before the boxed callback input is released. Paths reject absolute,
empty, dot, parent, backslash and NUL components. Native pointers do not escape.
Direct upstream C ABI bindings currently avoid an unnecessary C shim.

`cargo test -p image2usb-platform --features windows-media` passed with the
absolute native prefix; the added check reads a sparse source above 4 GiB,
rejects out-of-range/truncated reads and rejects an invalid UDF image.
After removing an unused-mut warning, feature-enabled Clippy with `-D warnings`
passed. This is not a valid-UDF extraction test. Directory catalog support,
valid/fragmented UDF fixtures and further callback failure tests remain pending.

### T005 directory increment

Added bounded immediate-child enumeration (250000 entries per directory),
validated relative paths and strict UTF-8 names. MUTF-8 NUL/surrogate encodings
are rejected, not silently normalized. Unsupported entry types fail closed.
A native integration test creates an isolated UDF image via `/usr/bin/hdiutil
makehybrid -udf`, enumerates root and sources, reads install.wim, checks EOF
and rejects traversal/absolute paths. It initially failed compilation because
read_dir was absent; after implementation both integration tests passed.
Additional unit checks cover callback error retention and invalid paths.
Feature-enabled platform tests and Clippy passed.

### T005 completion — fragmented fixture, panic containment, EOF

- Callback panic containment is now shared with a `contain_io` helper and has a
  unit regression: a panicking closure is converted to a retained I/O error; no
  unwind crosses the C ABI. The formatter callbacks use the same policy.
- Upstream `udfread_readdir` was inspected directly: after a successful
  `udfread_opendir`, all directory entries are already loaded, and `readdir`
  only indexes that array. `NULL` therefore means EOF there; parser or I/O
  failures must have returned `NULL` from `opendir`, where the wrapper recovers
  the saved callback error. There is no second, unobservable mid-iteration
  parser-error path in this pinned version.
- A fragmented native fixture was created without root and without touching a
  physical device: `newfs_udf -b 2048` in a 64 MiB file, attached read/write as
  a raw disk image, a 16 MiB sparse `frag.bin` written with `A` bytes at
  1 MiB and `B` bytes at 9 MiB, then detached. Allocation was 2 MiB for a
  16 MiB logical file, so the file cannot be one contiguous physical extent.
  `fsck_udf -n` reported a clean hierarchy. The native `udf_probe` example
  listed the file and returned `A`/`B` at the expected offsets, zeros around
  the holes, and EOF at 16 MiB. libudfread printed
  `unregonized string encoding 254` while reading this macOS-authored volume;
  reads remained correct. The warning is upstream output, not a wrapper error.
- The fixture was reproduced as an automated test in
  `crates/platform/tests/media.rs` (`udf_fragmented_sparse_file_reads_back`):
  it formats a 64 MiB file with `newfs_udf`, attaches it read-write at a
  temporary mountpoint with `hdiutil`, writes a 16 MiB sparse `frag.bin` with
  `A` at 1 MiB and `B` at 9 MiB, verifies that host blocks stay below 8 MiB,
  detaches, runs `fsck_udf -n` (clean), and reads the markers, hole zeros and
  EOF through libudfread. The test uses no physical device and no root.
  It takes about 1.4 s on the validation host and is skipped outside macOS by
  the test file's platform gate; it is not silently skipped on macOS.

The read-only UDF wrapper is complete for the macOS increment. This does not
prove Windows ISO extraction or boot; those remain later tasks.

## T006 — NTFS formatter adapted to RawIo callbacks

`crates/platform/native/ntfs_format.c` adapts the pinned NTFS-3G `mkntfs`
sources to a private `ntfs_device_operations` implementation. The device name
is only a label: open/close never touch a host path. Reads, writes, sector
geometry, block counts and flushes go through the caller's `RawIo`; offsets are
u64 relative to the supplied partition view. The original callback error is
retained in Rust and returned instead of a generic EIO.

The formatter passes an explicit partition start sector and BIOS geometry
(default 255 heads / 63 sectors per track) so the boot sector contains real
hidden-sector/CHS values instead of mkntfs warning that Windows cannot boot.
`NtfsFormatOptions` requires the partition LBA and supports 512-4096-byte
logical sectors. The native shim rejects non-power-of-two sectors, unaligned or
smaller-than-2-MiB volumes, an out-of-range partition LBA, zero geometry and
invalid labels before invoking mkntfs.

Safety measures:

- A global mutex serializes mkntfs; it is not reentrant. A concurrency test
  formats two images from separate threads and verifies both boot sectors.
- Callbacks catch Rust panics and store an error. Unit tests inject a panic and
  a sync failure; the C ABI returns failure without unwinding.
- The C shim resets `optind`/`optreset` and the patched mkntfs cleanup resets
  its global allocation list, `g_upcaseinfo` and stale geometry. Without this,
  a second format in the same process could free an already-freed list. The
  patch is applied by the build script with exact-match assertions and is
  idempotent.
- mkntfs is compiled with `-Dlibintl_setlocale=setlocale` because the host's
  `/usr/local/include/libintl.h` otherwise redirects `setlocale` to an
  unresolvable libintl symbol. This is a build adaptation, not a runtime
  dependency; the resulting static archive links only system libraries.

Executed on 2026-10-21 with the absolute prefix:

| Check | Result |
| --- | --- |
| `cargo test -p image2usb-platform --features windows-media` | PASS: 4 unit + 5 integration tests |
| `cargo clippy -p image2usb-platform --all-targets --features windows-media -- -D warnings` | PASS |
| 512-byte-sector format, 16 MiB | PASS: OEM-ID `NTFS    `, hidden sectors 2048, backup boot sector valid, `sync` invoked |
| 4096-byte-sector format, 64 MiB | PASS: bytes/sector 4096, partition offset preserved |
| injected write error / sync error / invalid label / tiny or unaligned volume | PASS: rejected with the injected error retained; no process abort on repeated calls |
| `ntfsinfo -m` on a 64 MiB fixture | Volume `I2U NTFS`, NTFS 3.1, 512-byte sectors, 4096-byte clusters |
| `file` on the same fixture | OEM-ID `NTFS    `, heads 255, sectors/track 63, hidden sectors 2048 |
| macOS native driver, read-only mount | `diskutil` identified `File System Personality: NTFS`; volume mounted read-only and root was listable |

`ntfsinfo` is built into the private prefix as a developer-only read-only
validator. It is not packaged, invoked by the application or used for writes.
The fixture was generated by the developer-only `ntfs_fixture` example. No
physical device, Windows host, chkdsk, boot, signature or Secure Boot check was
performed; those remain T007/T024/T039 work.

## T008 — private helper, hash and workspace boundary

`crates/platform/src/media.rs` now provides `Helper` and `PrivateWorkspace`:

- `Helper::resolve(prefix, relative, expected_sha256)` requires an absolute
  prefix, a relative path made only of normal components, canonical containment
  under that prefix, a regular executable file and a SHA-256 supplied by the
  release manifest. Symlink escapes are rejected after canonicalization.
- `PrivateWorkspace::new(parent)` creates a unique `tempfile` directory and
  sets mode 0700 on Unix. `create_new_file(name)` accepts one normal component,
  creates with `O_EXCL` and mode 0600, and never opens an existing path.
- `Helper::run` re-checks the helper digest immediately before spawning, uses
  `Command` with an argument vector, an absolute program path, a cleared
  environment and `LC_ALL=C`, and redirects stdout/stderr to exclusive files in
  the workspace. No shell or `PATH` lookup is involved.
- Cancellation kills the child and calls `wait` before returning, so the
  workspace cannot be deleted while the helper is still running. Non-zero exits
  become `Error::Command` with the exit code and a bounded stderr tail.

Executed on macOS 15.8: 9 unit tests pass, including hash mismatch, invalid
absolute/traversal paths, symlink escape, non-executable/missing helper,
0700/0600 workspace modes, exclusive creation, pre-existing file preservation,
shell-injection-shaped arguments, non-zero exit diagnostics and cancellation of
a five-second child in under three seconds (the test asserts under three).

Not yet wired: the release/build recipe still has to publish pinned hashes for
the actual `wimlib-imagex` binaries per architecture, and Windows must add ACL
hardening instead of Unix modes. Until then, callers cannot invent a hash; the
API requires the real manifest value. No helper is downloaded at runtime.

## T009 — requested/resolved filesystem and Windows options

`crates/core/src/plan.rs` now separates the requested filesystem choice from
the concrete writer value:

- `FileSystemChoice::{Auto, Fat32, Ntfs}` is what the user requests.
- `FileSystem::{Fat32, Ntfs}` has no `Auto` and is the only value used by the
  writer and by the resolved plan.
- `WritePlan` records `requested_filesystem`, the concrete `filesystem`, a
  `filesystem_reason` and independent `WindowsOptions`.
- `WindowsOptions { bypass_tpm, bypass_secure_boot, offline_account }` defaults
  every switch to `false`; enabling one does not enable the others. The old
  generic `bypass_win11` field is gone.

Serde `default` is applied to `WriteOptions`, `WindowsOptions` and the new plan
fields, so missing configuration fields load with safe defaults. Five tests in
`plan.rs` cover the choices, the absence of an `Auto` writer value, independent
switches, missing-field defaults, JSON round-trips and a resolved plan whose
request is `auto` while the concrete writer value is `ntfs`.

`WriteOptions::default()` deliberately remains `Fat32` in this increment so the
current write path is not silently changed. The CLI/GUI `auto` default and the
real resolution from prepared Windows media belong to T017/T018/T023. If a
caller tries to send an unresolved `Auto` into extraction, the writer boundary
returns an explicit error rather than guessing.

## T010 — preparation and terminal progress state

`Phase::Preparing` now covers snapshot/preparation work before the target is
opened. `ProgressEvent` carries `target_incomplete` (serde default `false`) and
has explicit constructors:

- `failed_before_write` / `cancelled_before_write` leave the target untouched.
- `failed_after_write` / `cancelled_after_write` set `target_incomplete = true`.
- `done_after_flush` is the only success constructor; its name records the
  requirement that `RawIo::sync` completed before success is reported.
- `ejecting` is a separate non-terminal event. An ejection warning does not
  turn a flushed, complete write into a failed one.

`Phase::is_data` and `Phase::is_terminal` centralize phase decisions. The CLI
uses `is_data` for its progress bar; both CLI and GUI show the incomplete marker
through `ProgressEvent::summary`. Four tests cover Preparing serde/display,
terminal states, incomplete-target transitions, Done/ejection separation and
the serde default for an absent field.

These tests prove the contract, not a physical commit. Emitting these terminal
events from the real commit path is part of T019/T034. No device was written.

## T007 — NTFS writer and executable checks (macOS increment)

Not complete. The macOS-side writer and its executable checks are implemented;
Windows `chkdsk`, Linux/Windows host builds and physical-device/boot checks
remain.

### Writer API

`crates/platform/src/media.rs` and `native/ntfs_format.c` expose:

- `NtfsVolume::mount(&mut dyn RawIo)` and `unmount()`/`sync()`;
- `NtfsVolume::mkdir(path)` with all missing parents created;
- `NtfsVolume::create_file(path)` and `open_file(path)`;
- `NtfsFile::write_at`, `read_at`, `truncate` and `close`.

The volume is limited to the NTFS partition view supplied by the caller; every
offset is partition-relative. A global lock is held for the whole lifetime of
`NtfsVolume`, not just per call, because libntfs-3g uses process-global caches.
Do not format another volume from the same process while one is mounted; the
lock is not reentrant. `NtfsFile` borrows the volume, so unmount cannot race an
open file. Closing a file flushes its metadata; `sync()` asks the device to
flush. If native unmount fails, `Drop` leaks the native handle and callback box
rather than leaving a retained callback pointer to freed memory.

Paths reject absolute paths, `.`/`..`, empty or repeated separators, backslash,
NUL, controls, `<>:"|?*`, trailing dot/space and names longer than 255 UTF-16
units. Existing names are not overwritten; a duplicate create fails.

### Executed checks (2026-10-21, macOS 15.8, Intel)

| Check | Result |
| --- | --- |
| `cargo test -p image2usb-platform --features windows-media` | PASS: 9 unit + 10 integration tests |
| `cargo clippy -p image2usb-platform --all-targets --features windows-media -- -D warnings` | PASS |
| 128 MiB writer fixture: two nested directories and `data.bin` | PASS; `ntfsinfo -F` reports data size 5,242,884 bytes; macOS native driver lists `sources/nested/data.bin` at that size |
| >4 GiB offset test (automated, 6 GiB sparse image) | PASS; logical file size 4,296,015,872 bytes (`0x100100000`), markers read at offset 0 and at 4 GiB + 1 MiB - 4, EOF at the size |
| >4 GiB fully populated file (manual, 6 GiB image) | PASS; `sources/big.bin` filled with 4,296,015,872 bytes through `RawIo` in about 16 s; `ntfsinfo -F` reports allocated/initialized size 4,296,015,872; macOS read-only mount reports 8,390,656 blocks (4 GiB) and returns `HEAD`, `0xa5` at 1 GiB, the tail marker at 4 GiB + 1 MiB - 8 and EOF correctly |
| Short-I/O callback wrapper capped at 4096 bytes | PASS for format, directory creation, 1 MiB write, read-back and unmount; `RawIo::sync` invoked |
| Injected write cancellation | PASS; `write_at` returns the original `cancelled write` error, the callback error is consumed, and the volume can still write, sync and unmount |
| Injected sync cancellation | PASS; `sync()` returns `cancelled sync`, then a retry succeeds and unmount succeeds |
| Automated fragmented UDF test | PASS; 16 MiB sparse file, clean `fsck_udf`, correct `A`/`B`, hole zeros and EOF |

The host image backing the automated >4 GiB fixture was sparse; the manual
fully populated run physically allocated about 4 GiB inside a 6 GiB host image.
That fully populated image was deleted after reading. These checks prove the macOS native writer and 64-bit
offset path only. They do not prove Windows `chkdsk` acceptance, NTFS boot,
Windows setup, Secure Boot, signatures, Linux/Windows builds or physical media.

Remaining T007 work: run the same writer checks on Linux/Windows, record Windows
`chkdsk /f` on a disposable image/device, and perform authorized physical-media
validation. T004 still owns the PowerShell/Linux recipes and remote CI.

## US1 synthetic increment — catalog, detection, FAT32 preparation/commit

Date: 2026-10-21. Host: macOS 15.8, Intel x86_64. No physical device,
real Windows ISO, OVMF, Secure Boot, Windows setup, Linux/Windows build or
remote CI was used. Fixtures are synthetic and are not boot evidence.

### Commands

| Command | Result |
| --- | --- |
| `cargo fmt --all -- --check` | PASS |
| `cargo clippy --workspace --all-targets -- -D warnings` | PASS |
| `cargo test --workspace` | PASS, 91 tests (CLI 4, core 22, extract unit 28, extract integration 17, iso unit 8, synthetic 12; GUI/platform/testiso have no default tests) |

The platform `windows-media` tests recorded earlier were not rerun in this
increment. Their macOS native linkage/format/writer results remain separate
and do not validate Windows extraction or boot.

### Implemented in the synthetic path

- ISO 9660/Joliet catalog now exposes `u64` offsets and lengths, checked extent
  arithmetic, source-size bounds, multi-extent record joining, a 250,000-entry
  ceiling, deterministic directory-before-file order, relative-path validation,
  and rejection of dangling/mismatched multi-extent sequences. A case-insensitive
  duplicate-path check is applied to Windows catalogs.
- Windows detection reports architecture from the fallback PE machine, EFI
  fallback path, `sources/idwbinfo.txt` build/edition when present, WIM/ESD/SWM
  format, installation parts/sizes and additive warnings. Synthetic tests cover
  x64, ARM64, unknown PE, missing BOOTMGR/BCD, ESD, SWM and multi-extent WIM.
- Preparation copies the whole source ISO to a private `0700` workspace with a
  `0600` snapshot file, hashes the snapshot while copying, compares source
  metadata before/after, analyzes only the snapshot and rejects incomplete
  boot chains, unknown PE machine, links, case-ambiguous paths and ambiguous
  installation-image sets before destination I/O. Cancellation and ENOSPC
  mapping are unit-tested. The snapshot stabilizes the bytes for one operation;
  it does not authenticate the ISO or guarantee detection of adversarial
  same-size changes during capture.
- Windows FAT32 commit checks the FAT32 file limit and destination capacity
  before partitioning, writes MBR or GPT, formats FAT32, copies every catalog
  entry from the snapshot, avoids Linux GRUB/boot-config rewriting, flushes,
  syncs and rereads SHA-256 when `--verify` is selected. A synthetic commit
  produces BOOTMGR, BCD, BOOTX64.EFI and install.wim and verifies at readback.
- CLI exposes `--filesystem auto|fat32|ntfs`, prints the requested/resolved
  filesystem and reason, resolves non-hybrid Windows `auto` to extraction,
  performs dry-run preparation without creating the output, rejects explicit
  filesystem with DD, rejects Linux NTFS and refuses a source/symlink/hard-link
  destination even with `--yes`. A source replaced after preparation does not
  change the committed tree in the synthetic test.
- GUI has a filesystem selector, specific Windows compatibility messages, a
  resolved-filesystem plan and invalidates confirmation when source, device,
  mode, scheme, filesystem or verification changes.

### Explicitly incomplete / unsupported

- UDF-only Windows media still fails analysis with a specific error; the shared
  catalog has not consumed `libudfread`. Multi-extent ISO/Joliet is supported.
- NTFS formatting/writing primitives exist but Windows NTFS commit, split WIM,
  ESD-to-NTFS selection and UEFI:NTFS asset/layout are not implemented. NTFS is
  refused before preparation/writing.
- No WIM split helper hash/recipe is wired into extraction. No `autounattend`
  or Windows 11 tweak is generated; defaults remain off.
- GUI preparation still occurs in the extraction worker after the destination
  is opened (preparation itself performs no destination write); it is not yet a
  separate pre-confirmation step, and GUI transition tests are absent.
- Corrupt-readback behavior is implemented through `verify_manifest` but does
  not yet have an independent corrupt-device regression. Real ENOSPC and
  snapshot-cleanup observation on a full filesystem are also pending.
- Windows 10/11 boot, setup, Secure Boot on/off, signatures, DB/DBX, chkdsk,
  physical media, Linux/Windows host builds and cross-platform packaging remain
  open gates (T019/T028/T035/T036/T037/T039/T041).

## US2 synthetic increment — NTFS data + UEFI:NTFS ESP

Date: 2026-10-22. Host: macOS 15.8, Intel x86_64. No physical device, real
Windows ISO, OVMF/Windows setup, Secure Boot, Linux/Windows build or remote CI
was used. The tests prove the native formatting/writing path and partition
readback for synthetic Windows-shaped media; they are not boot evidence.

### Asset provenance

`assets/uefi-ntfs/uefi-ntfs.img` is the exact 1 MiB image from the pinned Rufus
commit recorded in `assets/windows-media/manifest.json`:

```text
sha256 72683fa1250eeea772d3399277b434d4e55ba8dd0dc926e52d817e701fc2eb9e
size   1048576
```

The image is embedded with `include_bytes!`, and a test checks its length and
SHA-256. It must not be mounted read-write on macOS: that adds `.fseventsd`
metadata and changes the bytes. PE signatures, the DB/DBX trust path and
component source delivery remain pending T024/T003 work; the asset is not yet
approved for a public release.

### Layout and commit

- MBR: NTFS data partition type `0x07` first, fixed 1 MiB UEFI:NTFS partition
  type `0xEF` at the end, 1 MiB alignment and reserved gaps.
- GPT: Microsoft Basic Data entry first, EFI System Partition second, primary
  and backup headers/entry arrays with CRC32. The same two partitions are used.
- 512-byte logical sectors only in this increment. The code refuses 4Kn before
  touching the target because the asset BPB and firmware behavior are not yet
  validated at 4096-byte logical sectors.
- NTFS is formatted through `format_ntfs` with the real partition LBA. The
  Windows catalog is copied at 64-bit file offsets, then the pinned FAT image is
  written byte-for-byte to the ESP. NTFS is synced/unmounted and the asset is
  read back and compared with the embedded bytes.
- With `--verify`, every NTFS file is reopened, read back and hashed. Space is
  checked before the partition table is written using the tree size plus a
  conservative NTFS metadata margin.
- Invalid NTFS component names are rejected before partitioning. The source
  remains the private Windows snapshot used by the FAT32 path.

### Commands

Run with the absolute native prefix:

```sh
export IMAGE2USB_WINDOWS_MEDIA_PREFIX="$PWD/target/windows-media/x86_64/install"
cargo test -p image2usb-extract --features windows-media
cargo clippy -p image2usb-extract --features windows-media --tests -- -D warnings
cargo test -p image2usb-cli -p image2usb-gui \
  --features image2usb-cli/windows-media,image2usb-gui/windows-media
```

| Check | Result |
| --- | --- |
| `cargo check --workspace --locked` (default features) | PASS |
| `cargo test --workspace --locked` (default features) | PASS: 4 CLI, 22 core, 28+17 extract, 8+12 ISO, GUI/platform/testiso 0 |
| `cargo clippy --workspace --all-targets --locked -- -D warnings` | PASS |
| extract feature tests | PASS: 33 unit, 17 integration, 2 NTFS/UEFI:NTFS |
| extract feature Clippy with `-D warnings` | PASS |
| CLI/GUI feature tests | PASS: 4 CLI tests, 0 GUI tests |
| `cargo fmt --all --check` | PASS |

The NTFS integration test builds a synthetic Windows-shaped ISO, commits it to
a 96 MiB file-backed target with `--verify` for both MBR and GPT, checks both
partition tables, checks the ESP equals the pinned asset, and reads BOOTMGR,
EFI/BOOT/BOOTX64.EFI, SOURCES/INSTALL.WIM and SOURCES/IDWBINFO.TXT back through
`NtfsVolume`.

An independent host probe attached a generated GPT image read-only with
`hdiutil`. macOS reported:

```text
GUID_partition_scheme
Microsoft Basic Data  IMAGE2USB
EFI                   RUFUS_BOOT
```

The NTFS volume mounted read-only and exposed `BOOT/BCD`, `BOOTMGR`,
`EFI/BOOT/BOOTX64.EFI`, `SOURCES/IDWBINFO.TXT` and the 4096-byte
`SOURCES/INSTALL.WIM` fixture; the ESP mounted as `RUFUS_BOOT` with the upstream
`EFI` tree. `diskutil verifyVolume` could not start an NTFS fsck on this host,
so this is not an NTFS consistency check. The probe used no physical device.

The host-only package path also built successfully:

```sh
IMAGE2USB_WINDOWS_MEDIA_PREFIX="$PWD/target/windows-media/x86_64/install" \
  sh scripts/package-macos.sh 0.1.0 --native
```

It produced `dist/Image2USB-0.1.0-macos-x86_64-ntfs.dmg`. Both embedded
executables are thin `x86_64`, the bundle passes `codesign --verify --deep
--strict` with an ad-hoc signature, `image2usb-cli --version` prints `0.1.0`,
`image2usb-cli list --json` returns `[]` without touching a device, and the
binary contains the NTFS/UEFI:NTFS feature strings. This is packaging evidence,
not boot evidence, and the universal app remains without NTFS support.

### Explicitly incomplete / unsupported

- No real Windows 10/11 ISO or file larger than 4 GiB was committed. The
  >4 GiB NTFS primitive evidence remains the separate platform test; a real
  `install.wim` write/readback and independent `chkdsk` are still required.
- No Windows setup boot, OVMF test, Secure Boot on/off test, PE signature
  verification or DB/DBX review was performed. The UEFI:NTFS asset is embedded
  for local integration only until T003/T024/T037 close.
- 4Kn NTFS/UEFI:NTFS is refused explicitly. 512-byte MBR/GPT only.
- Apple Silicon native libraries, Linux/Windows builds and remote CI were not
  exercised. The feature is macOS-only and opt-in; the packaged universal app
  does not include it unless built natively with the prefix.
- WIM split, automatic FAT32/NTFS selection after helper inspection, real
  Windows large-file boot and physical-media validation remain open
  (T020/T022/T023/T028/T039).
