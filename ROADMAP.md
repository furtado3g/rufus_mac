# image2usb roadmap

Status: **pre-alpha, UEFI boot validated; M2 extraction hardening and M3 GPT/ESP implemented**.
Last updated: 2026-10-20.
Main branch: `main` (see `git log`).

This is the working plan for the next milestones. Each milestone lists the goal,
the concrete tasks, the acceptance criteria and the main risks. A milestone is
only done when code, tests, documentation and validation evidence are all in the
repository and CI is green.

## Current state

| Area | State | Evidence / notes |
| --- | --- | --- |
| Workspace | Done | `core`, `platform`, `iso`, `extract`, `testiso`, `cli`, `gui` |
| Device safety | Done | USB/SD only, system disk refused, identity rechecked, explicit confirmation, failing offset reported |
| ISO analysis | Done | ISO 9660, Joliet, Rock Ridge names, El Torito, hybrid MBR/GPT, distro markers |
| DD mode | Done | 4 MiB chunks, progress, cancel, SHA-256 verify |
| Extraction | MBR/GPT + FAT32, UEFI | Real 3.0 GiB Ubuntu derivative: space preflight, `fsck_msdos` clean, 290/290 `md5sum.txt` matches, live desktop; MBR and GPT/ESP both boot; 4Kn file-backed and platform `fsck` tests |
| CLI | Done | `list`, `analyze`, `write`; `--mode auto|dd|extract`, `--scheme mbr|gpt`, `--device`, `--output`, `--yes`, `--dry-run`, `--verify`, `--label` |
| GUI | Done for DD/extract | egui/eframe, device picker, analysis, progress, Windows-extraction warning |
| Portable artifacts | Partial | Release workflow drafted; AppImage, signing and notarization pending |
| UEFI firmware boot | QEMU/OVMF **verified** | Extraction and DD control reached the live desktop; evidence in `docs/validation/uefi-2026-10-20.md`; physical hardware pending |
| Physical USB write | Pending | No real USB stick has been written; do not use the connected Expansion HDD |
| BIOS boot | Not implemented | Only UEFI boot files are copied |
| Windows media | Not implemented | Rejected with an explicit `Unsupported` error |
| GPT extraction | Done (single ESP) | `--scheme gpt`, protective MBR, primary/backup headers, CRC32, diskutil and OVMF validation |
| NTFS / second data partition | Not implemented | Single FAT32 ESP only; large-tree split is deferred |
| ARM64 targets | CI only | Windows ARM64 and Linux ARM64 paths not locally validated |
| Rock Ridge symlinks | Done (skip) | Direct `SL` and SUSP `CE` targets detected; symlinks skipped with a warning because FAT32 has none |

### Validated commands

```console
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Cross-checks that pass locally:

```console
cargo check -p image2usb-core -p image2usb-iso -p image2usb-extract -p image2usb-cli \
  --target x86_64-unknown-linux-gnu
cargo check -p image2usb-core -p image2usb-iso -p image2usb-extract -p image2usb-cli \
  --target x86_64-pc-windows-msvc
```

The GUI is compiled and tested natively on macOS; Linux/Windows GUI builds are
covered by the release workflow, not by local cross-checks.

## Start on another machine

### 1. Clone and toolchain

```console
git clone https://github.com/furtado3g/rufus_mac.git
cd rufus_mac
rustup show          # rust-toolchain.toml pins 1.98.1 + rustfmt + clippy
```

The repository uses Rust edition 2024 and `rust-version = 1.88`. Install the
toolchain with rustup; do not rely on a distribution-packaged Cargo.

Optional cross-check targets:

```console
rustup target add x86_64-unknown-linux-gnu x86_64-pc-windows-msvc
```

For macOS universal2 artifacts also add `aarch64-apple-darwin` and
`x86_64-apple-darwin`. ARM64 builds are produced on native CI runners.

### 2. Host prerequisites

**macOS (Intel and Apple Silicon)**

- Xcode Command Line Tools (`xcode-select --install`).
- `hdiutil`, `diskutil`, `fsck_msdos` (built in).
- QEMU + OVMF for boot tests: `sudo port install qemu` (MacPorts) or
  `brew install qemu`.

**Linux**

- Rust build essentials, `pkg-config`, `libdbus-1-dev`, `libxkbcommon-dev`,
  `libwayland-dev`, `libx11-dev`, `libgl1-mesa-dev` for the GUI.
- `qemu-system-x86`, `ovmf`, `dosfstools`, `mtools`, `util-linux` for boot and
  filesystem checks.
- Root privileges for raw device writes and loop devices.

**Windows**

- Visual Studio Build Tools with the C++ workload and the Windows SDK.
- Rust MSVC toolchain (`stable-x86_64-pc-windows-msvc`).
- QEMU for Windows + OVMF firmware for boot tests.
- Run the terminal as Administrator only for `write`; `list` and `analyze` work
  unprivileged (the CLI manifest uses `asInvoker`).

### 3. Smoke test

```console
cargo build --workspace
cargo test --workspace
```

Run the CLI against a real ISO:

```console
cargo run -p image2usb-cli -- list
cargo run -p image2usb-cli -- analyze ubuntu-24.04.iso
cargo run -p image2usb-cli -- write ubuntu-24.04.iso --device /dev/diskX --dry-run --mode extract
```

For a file target (no USB needed), create a pre-sized image first:

```console
# macOS
mkfile -n 4g /tmp/usb.img
# Linux
truncate -s 4G /tmp/usb.img

cargo run -p image2usb-cli -- write ubuntu-24.04.iso \
  --output /tmp/usb.img --yes --mode extract --verify
```

`--output` extraction requires an existing file of at least `ISO size + 16 MiB`
(and at least 64 MiB). The partition starts at 1 MiB and reserves 1 MiB at the
end.

### 4. Boot the result in QEMU (UEFI)

The full command is in
[`docs/compatibility.md`](docs/compatibility.md#booting-an-extracted-image-in-qemu-uefi).
Short version on an Intel Mac with MacPorts:

```console
QEMU_PREFIX=/opt/local/share/qemu
cp "$QEMU_PREFIX/edk2-i386-vars.fd" /tmp/ovmf-vars.fd
qemu-system-x86_64 -machine q35,accel=hvf -cpu host -m 4096 -smp 4 \
  -drive if=pflash,format=raw,readonly=on,file="$QEMU_PREFIX/edk2-x86_64-code.fd" \
  -drive if=pflash,format=raw,file=/tmp/ovmf-vars.fd \
  -drive if=none,id=usb,format=raw,snapshot=on,file=/tmp/usb.img \
  -device qemu-xhci -device usb-storage,drive=usb,bootindex=0 \
  -vga std -display vnc=127.0.0.1:1
```

Connect a VNC client to `127.0.0.1:1`. OVMF should load
`EFI/BOOT/BOOTX64.EFI`, show GRUB, and load `/casper/vmlinuz` and
`/casper/initrd.gz`. On Linux use `-accel kvm` and the distro OVMF paths.

### 5. Safety rules for physical tests

- Use a disposable USB stick; `image2usb list` shows which devices are safe.
- Never target the disk that hosts the running OS or a connected backup drive.
- Run `--dry-run` first and compare the reported device, size and model.
- Physical writes need root/Administrator privileges.
- Extraction modifies the target from byte 0; there is no undo.

## Milestones

### M1 — UEFI boot validation (QEMU/OVMF complete; physical pending)

**Goal:** prove that an extracted image boots under UEFI firmware in QEMU/OVMF,
not only that it mounts. Physical firmware is covered by M7.

**Tasks**

- [x] Install QEMU + OVMF on the development host (macOS 15.8 Intel, MacPorts
      QEMU 11.1.1).
- [x] Boot the 4 GiB image produced from the real Ubuntu derivative.
- [x] Capture the GRUB menu and the live desktop (screenshot and serial log).
- [x] Record the host, QEMU/OVMF version, image, commit and result in
      `docs/validation/uefi-2026-10-20.md`.
- [x] If OVMF does not see the partition, test MBR type `0xEF` (ESP) instead of
      `0x0C` and keep the variant that works on both OVMF and real firmware.
      Not needed: `0x0C` worked.
- [x] Verify that `/boot/grub/grub.cfg` is found and that `/casper/vmlinuz` and
      `/casper/initrd.gz` are loaded from the FAT32 partition; the live desktop
      is the end-to-end proof.
- [ ] Add a headless Linux CI smoke test with QEMU/TCG and OVMF that asserts a
      kernel banner on the serial console (see "CI smoke test" below).
- [x] Update the firmware matrix in `docs/compatibility.md`.

**Result:** PASS on 2026-10-20. Extraction and a DD control both reached the
live desktop. Both logs contain the same non-fatal `efi_wrap_10` GRUB warning,
so it is not an extraction regression.

**Acceptance**

- OVMF loads `EFI/BOOT/BOOTX64.EFI` from the generated MBR/FAT32 image.
- GRUB is visible and the default entry starts the kernel/initrd.
- The procedure is reproducible from a clean checkout with the documented
  commands.
- The result is recorded in the repository, including any firmware-specific
  workaround.

**Risks**

- OVMF may require partition type `0xEF`; real firmware may accept `0x0C` too.
- Secure Boot on physical hardware may reject unsigned GRUB; Ubuntu's signed
  `BOOTX64.EFI` should work, but custom assets need a shim.
- Rock Ridge symlinks are skipped rather than materialized as empty files; a
  distro that relies on a symlinked boot path still needs DD mode or a future
  target-resolution pass.

### M2 — Extraction hardening

**Goal:** make Linux extraction reliable across common distributions and edge
cases.

**Tasks**

- [x] Parse Rock Ridge `SL` entries and skip symlinks with a warning (FAT32 has
      no symlinks), or copy the target for file symlinks. Do not create empty
      files silently. Direct entries and SUSP `CE` continuation areas are
      handled.
- [x] Preflight free space: sum file sizes and estimated FAT/directory overhead
      against the partition size; fail before writing the partition table.
- [x] Split progress into `Formatting`, `Copying`, `Finalizing`, `Verifying` and
      make the ETA use the copy phase only.
- [ ] Make cancellation responsive during formatting/finalization. The
      post-processing pass now checks the token per directory/cluster and stops
      cleanly; `fatfs::format_volume` is still an atomic call, so formatting is
      only cancellable before and after it.
- [x] Add 4096-byte logical-sector and 4Kn device tests (file-backed; physical
      device remains in M7).
- [x] Harden FAT names: Unicode normalization (NFC/NFD), trailing dots/spaces,
      reserved names, very long paths/components, `.`/`..`, case-only collisions.
- [x] Add `fsck.vfat -n`/`fsck_msdos -n` checks to CI where the tool exists.
- [x] Add mount-and-compare validation for at least five distributions: Ubuntu,
      Debian, Fedora, Arch and Alpine. Mint/Pop!_OS remain optional follow-ups.
- [ ] Explicitly reject files larger than FAT32's 4 GiB limit with a clear
      message (today the ISO extent is `u32`, so this is defensive and not yet
      reachable; revisit with UDF support).
- [x] Treat UDF-only images as unsupported with a clear error; keep the warning
      for hybrid ISO/UDF images.
- [ ] Add a `--jobs` or read-ahead option only if benchmarks justify it; write
      throughput should be device-bound.

**Status (2026-10-20):** direct/CE symlink handling, FAT32 space preflight,
phase-split progress, 4Kn file-backed tests, NFC name normalization, UDF-only
rejection and platform `fsck` tests are implemented. Ubuntu, Alpine, Debian,
Fedora and Arch all extracted with `--verify`, mounted and passed `fsck`.
Ubuntu and GPT/ESP booted to the live desktop; Fedora reached Anaconda and Alpine
reached a login prompt under OVMF. Fedora also exposed FAT32's 11-character label
limit, so boot configs are now rewritten to the effective label. Remaining work:
physical 4Kn device, file-size guard for a future UDF path, fine-grained formatting
cancellation, and optional Mint/Pop!_OS coverage.

**Acceptance**

- [x] At least five Linux distributions extract, mount and pass `fsck` with no
  unexpected warnings. (Ubuntu/DESERT, Alpine 3.24.2, Debian 13.7.0, Fedora 44
  and Arch 2026.10.01; see the validation report.)
- A too-small target is rejected before the MBR is touched.
- Symlinks produce a warning, not a silent empty file.
- Existing synthetic and real-ISO tests remain green.

### M3 — GPT + EFI System Partition

**Goal:** support GPT-only images and firmware that expects an ESP.

**Tasks**

- [x] Add `PartitionScheme::Gpt` to the plan and CLI/GUI (`--scheme gpt`).
- [x] Write a protective MBR, primary GPT header, 128-entry partition array,
      backup GPT at the end, correct CRC32s and 1 MiB alignment.
- [x] Create one FAT32 ESP with type GUID
      `C12A7328-F81F-11D2-BA4B-00A0C93EC93B`; keep the volume label option.
- [x] Decide whether a second data partition is needed for large trees; if not,
      document the single-ESP layout. (Single ESP for now; documented in the
      extraction module and README.)
- [ ] Validate with `sgdisk --verify`, `gdisk`, `parted`, macOS `diskutil` and
      QEMU OVMF. `diskutil` and QEMU pass; the `sgdisk --verify` test runs in CI
      when `gdisk` is installed. `gdisk`/`parted` manual checks are still open.
- [x] Preserve MBR as the default until GPT has equivalent test coverage.
- [x] Add extraction tests for GPT and a QEMU boot test.

**Acceptance**

- `sgdisk --verify` reports no errors. (CI test installed with `gdisk`; local
  macOS does not have `sgdisk`.)
- The ESP mounts on macOS/Linux and boots under OVMF. (macOS diskutil mount and
  OVMF live desktop verified; Linux fsck covered by the platform test.)
- MBR extraction is unchanged and still default. (Existing MBR tests pass.)

**Status (2026-10-20):** implementation and real-image validation are complete
for the single-ESP layout. A later iteration can add a second data partition or
NTFS if large-tree support needs it.

### M4 — BIOS boot support

**Goal:** boot on legacy BIOS machines.

**Tasks**

- [ ] Choose the asset strategy: syslinux for isolinux-based distributions,
      GRUB for grub-based ones; embed only the needed modules.
- [ ] Add GPL notices and a written offer/source link for bundled assets; update
      `THIRD_PARTY_NOTICES.md`.
- [ ] Write MBR boot code and the partition VBR; install `ldlinux.sys`/`core.img`
      or the equivalent; generate a config from the ISO config, rewriting paths
      to the FAT32 layout.
- [ ] Build a hybrid MBR when both BIOS and UEFI must boot.
- [ ] Detect unsupported boot layouts and fall back to DD with a clear warning.
- [ ] Test with QEMU SeaBIOS and at least one physical legacy machine.
- [ ] Keep UEFI boot working on the same image.

**Acceptance**

- BIOS firmware shows the bootloader menu and loads the kernel/initrd.
- The same image still boots under UEFI.
- Asset licenses and source availability are documented.

**Risks**

- Secure Boot and signed bootloaders.
- Per-distro config differences (`isolinux.cfg`, `grub.cfg`, `loopback.cfg`).
- GPL asset redistribution and source-offer obligations.

### M5 — Windows installation media

**Goal:** create Windows 10/11 USB drives.

**Tasks**

- [ ] Integrate wimlib (LGPL) for WIM inspection and splitting; document
      dynamic vs. bundled linking and source availability.
- [ ] Implement the >4 GiB `install.wim` path: split into `install.swm` on
      FAT32, or create an NTFS data partition plus UEFI:NTFS.
- [ ] Copy the Windows boot chain (`bootmgr`, `EFI/Microsoft/Boot`, BCD) and
      ensure the removable fallback `EFI/BOOT/BOOTX64.EFI` exists.
- [ ] Detect Windows media even when distro markers are missing
      (`sources/install.wim|esd|swm`).
- [ ] Add Windows 11 options as opt-in: TPM/Secure Boot bypass and offline
      account (registry edits); keep defaults non-destructive.
- [ ] Test in QEMU OVMF and on physical UEFI hardware; include Windows 10/11
      x64 and ARM64 where available.
- [ ] Update the GUI warning and CLI hints; remove the current rejection.

**Acceptance**

- Windows 10 and 11 installers boot and reach the setup UI from a generated USB.
- Large WIM handling is documented and verified.
- No registry/boot modification happens unless the user opts in.

### M6 — Portable artifacts, signing and CI

**Goal:** ship downloadable artifacts for all supported platforms.

**Tasks**

- [ ] Windows: `.zip` with CLI (`asInvoker`) and GUI (`requireAdministrator`,
      `windows_subsystem = "windows"`), no installer, no console window for the
      GUI.
- [ ] macOS: universal2 `.app` in `.dmg` and `.zip`; ad-hoc signing fallback;
      Developer ID signing and notarization when credentials exist.
- [ ] Linux: `.tar.gz` and AppImage; document the glibc baseline (2.35) and
      required desktop libraries.
- [ ] CI matrix: macOS Intel + Apple Silicon, Windows x64 + ARM64, Ubuntu 22.04
      + 24.04 x86_64/aarch64; native runners per architecture.
- [ ] Add `cargo deny`, license audit, SBOM and checksums to the release job.
- [ ] Smoke-test each artifact on a clean VM (CLI `--version`, `list`,
      `analyze`, extraction to a file).
- [ ] Add a changelog and versioning policy.

**Acceptance**

- All artifacts run on clean VMs without a Rust toolchain.
- Checksums are published and match.
- macOS app passes Gatekeeper when signed/notarized; ad-hoc builds include
  instructions to open them.
- The release workflow is reproducible and documented.

### M7 — Hardware validation and 1.0 release

**Goal:** prove the tool on real machines and freeze the first stable release.

**Tasks**

- [ ] Firmware matrix: UEFI x86_64 (OVMF, physical), legacy BIOS, UEFI aarch64
      (AAVMF/Apple Silicon), Secure Boot on/off.
- [ ] USB 2.0/3.x, SD cards, 4Kn devices, large drives.
- [ ] Distros: Ubuntu, Debian, Fedora, Mint, Pop!_OS, Arch; Windows 10/11;
      FreeBSD if feasible.
- [ ] Test cancellation, unplug during write, permission errors and read-only
      devices.
- [ ] Fuzz the ISO parser and the FAT32 writer; add regression tests.
- [ ] Complete the user manual: device safety, mode selection, verification,
      troubleshooting, exit codes.
- [ ] Review `unsafe` usage (only `image2usb-platform`), dependency licenses and
      supply chain.
- [ ] Tag `v1.0.0` and publish signed artifacts.

**Acceptance**

- At least two physical UEFI machines and one legacy BIOS machine boot from a
  generated drive.
- No data-loss incident in the test log.
- All release gates below pass.

## Cross-cutting backlog

- Safety invariants: never write to the system disk, revalidate identity before
  the first byte, require explicit confirmation, keep a clear audit log.
- Error UX: stable exit codes, actionable hints, exact failing offset.
- Performance: benchmark DD and extraction on USB 2.0/3.0; avoid write
  amplification (the partition cache already keeps FAT/data/dir windows).
- Observability: `RUST_LOG`, `--verbose`, optional diagnostic bundle.
- Accessibility/localization: English and Portuguese UI at minimum.
- Security: no shell invocation for privileged operations, signed assets,
  checksum verification, no silent elevation.
- Documentation: keep `README.md`, `docs/compatibility.md`, `ROADMAP.md` and
  `THIRD_PARTY_NOTICES.md` in sync with every feature.

## Release gates

A release is blocked until all of these are true:

1. `cargo fmt --all --check`, Clippy with `-D warnings`, and
   `cargo test --workspace` pass.
2. Cross-checks for Linux x86_64 and Windows x86_64 pass; native ARM64 CI jobs
   pass.
3. UEFI boot is validated in QEMU/OVMF and on at least one physical machine.
4. Extraction passes `fsck` and the distro's own checksum manifest on the
   supported distro list.
5. No known high-severity safety or security issue is open.
6. Artifacts are checksummed and, where credentials exist, signed/notarized.
7. Documentation and third-party notices are updated.
8. A physical USB write and boot test has been recorded by someone other than
   the author (release candidate only).

## Open questions

- Should extraction default to MBR or GPT once both work? Windows may force GPT;
  Linux live images work with either.
- Which distros are officially supported for 1.0?
- Do we bundle syslinux/GRUB assets, or generate them from a build-time
  dependency? Licensing and reproducibility differ.
- Windows: split WIM on FAT32, NTFS + UEFI:NTFS, or both?
- Is Secure Boot support in scope for 1.0, or documented as a limitation?
- Which macOS minimum: 12 (Monterey) as currently documented, or newer?
- Do we need a GUI on Linux/Windows for 1.0, or is the CLI enough on those
  platforms?

## Validation log template

Copy this into `docs/validation/<date>-<firmware>.md`:

```markdown
# UEFI boot validation — YYYY-MM-DD

- Host: macOS 15.8, Intel x86_64
- QEMU/OVMF: qemu 11.1.1, edk2-x86_64-code.fd
- Commit: <sha>
- Image: <path>, <size>, generated with `--mode extract --verify`
- ISO: <name>, <size>, <sha256>
- Command: <qemu command or link to it>
- Result: OVMF loaded EFI/BOOT/BOOTX64.EFI; GRUB menu shown; default entry
  loaded /casper/vmlinuz and /casper/initrd.gz.
- Evidence: <screenshot/log path>
- Notes: <firmware quirks, workarounds>
```

## Suggested immediate order

1. **M1** QEMU/OVMF validation is complete; keep physical-hardware testing in
   M7.
2. Finish the remaining **M2** hardening (multi-distro mount/compare and
   fine-grained formatting cancellation). Symlinks, free-space preflight, 4Kn
   file-backed tests, FAT name edge cases and CI `fsck` are already implemented.
3. **M3** (GPT/ESP) is implemented and boot-tested; finish optional `gdisk`/`parted`
   checks before calling it fully closed.
4. Choose between **M4** (BIOS) and **M5** (Windows) based on user demand; BIOS
   is smaller, Windows is the larger Rufus parity gap.
5. Finish **M6** once the feature set is stable; signing credentials are the
   long-lead item.
6. Run **M7** on real hardware before tagging 1.0.
