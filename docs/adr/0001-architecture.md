# ADR 0001 — Architecture of image2usb

Status: accepted (phase 0)

## Context

Rufus is Windows-only because it relies on Windows APIs (`\\.\PhysicalDriveN`,
`FormatEx`, `bcdboot`, registry hive edits). image2usb must run on macOS, Linux
and Windows and remain a portable executable, so the implementation is split
into a platform-independent core and thin native backends.

## Decision

1. **Workspace of small crates.** `core` owns the domain model, safety
   invariants and the write pipeline. `platform` implements raw device access.
   `iso`, `partition`, `fs` and `boot` are added as the pipeline grows.
   `cli` and `gui` are front ends over the same `core` API.
2. **`RawIo` is offset-addressed** (`read_at`/`write_at`) instead of
   `Read + Seek`, because every native API (pread/pwrite, SetFilePointerEx) is
   offset based and because it makes sector alignment explicit.
3. **Two write modes.** DD copies an image 1:1 and requires zero filesystem
   knowledge; extraction builds a partition table, a filesystem and boot files.
   The analysis engine decides which mode is applicable before anything is
   written.
4. **Safety is a library concern.** The `check_target` invariant is enforced by
   the core, not by the front ends, and the platform backend re-enumerates the
   device and compares an identity token before opening it.
5. **No `unsafe` outside `platform`.** The core, ISO parser, partition and
   filesystem crates are `#![forbid(unsafe_code)]`.
6. **Bundled assets, no system tools.** Bootloader payloads are embedded in the
   binary. External commands are limited to OS utilities that ship with the
   system (`diskutil`, `lsblk`) and are invoked without a shell.

## Consequences

- The core can be tested against regular files and disk images on any OS.
- Windows/Linux backends need CI coverage; macOS cannot compile them natively.
- Bundling GPL components (syslinux, GRUB, UEFI:NTFS) forces GPL-3.0-or-later
  for the whole distribution.
- The GUI can be replaced without touching the write pipeline.

## Alternatives considered

- **Rust + Tauri**: nicer UI, but WebView dependencies complicate portable
  distribution and add a frontend toolchain.
- **Shelling out to `dd`/`diskutil`/`dd for Windows`**: not portable, not
  observable and impossible to make transactional or safe.
- **A single monolithic crate**: simpler at first, but the platform layer would
  leak `#[cfg]` everywhere and block future extraction-mode work.
