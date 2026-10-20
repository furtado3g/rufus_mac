# image2usb

Create bootable USB drives from ISO images on **macOS**, **Linux** and **Windows**.

image2usb is a portable, cross-platform alternative to Rufus written in Rust. It
ships as a self-contained executable / app bundle: no installer, no runtime, no
external tools required.

> Status: **pre-alpha** (M3 — GPT + ESP). Device enumeration, ISO analysis,
> DD-mode writing and UEFI extraction are implemented. Both MBR and GPT/ESP
> images boot to a live desktop under QEMU/OVMF. BIOS bootloaders, Windows media
> and physical-hardware validation are still in progress.

See [ROADMAP.md](ROADMAP.md) for the detailed plan, development setup on another
machine, and per-milestone acceptance criteria.

## Features (roadmap)

- [x] Safe device enumeration (USB / SD only, system disks blocked)
- [x] ISO analysis: ISO9660, Joliet, El Torito (BIOS/UEFI), hybrid detection
- [x] DD mode: write a hybrid ISO byte-for-byte, with progress, cancel and SHA-256 verification
- [x] Extraction mode: MBR or GPT + FAT32 + files copied from the ISO tree (UEFI boot, Linux distros)
- [x] GPT with a single EFI System Partition (ESP)
- [ ] NTFS data partitions and a second partition for large trees
- [ ] BIOS boot: syslinux, GRUB2, Windows BOOTMGR
- [ ] Windows installation media (wimlib / split-WIM)
- [ ] Windows 11 unattended options (TPM / Secure Boot / offline account bypass)
- [x] Graphical interface (egui): device picker, ISO analysis, DD and extraction writes
- [ ] Signed portable artifacts

## Portable artifacts

| Platform | Artifact |
| --- | --- |
| Windows x64 / ARM64 | `image2usb-<version>-windows-<arch>.zip` |
| macOS (Intel + Apple Silicon) | `Image2USB.app` inside `.dmg` and `.zip` |
| Linux x86_64 / aarch64 | `.AppImage` and `.tar.gz` |

## CLI

```console
$ image2usb list
$ image2usb analyze ubuntu-24.04.iso
$ sudo image2usb write ubuntu-24.04.iso --device /dev/disk4 --verify
$ sudo image2usb write ubuntu-24.04.iso --device /dev/disk4 --mode extract --verify
$ sudo image2usb write ubuntu-24.04.iso --device /dev/disk4 --mode extract --scheme gpt --verify
```

`image2usb` must run with administrator/root privileges to write to a raw device.
The GUI and `list`/`analyze` work unprivileged.

On Windows the CLI manifest uses `asInvoker`, so `list` and `analyze` run without
a UAC prompt; start the terminal (or the executable) as Administrator before
`write`. The GUI manifest requests elevation at launch, so writes work directly.

### Extraction mode

`--mode extract` creates one FAT32 partition and copies the ISO tree. MBR is the
default; `--scheme gpt` writes a protective MBR, primary/backup GPT and one EFI
System Partition (ESP). It is the fallback for bootable images that cannot be
written 1:1 (non-hybrid ISOs):

```console
$ sudo image2usb write ubuntu-24.04.iso --device /dev/disk4 --mode extract --verify
```

Current limits (M3):

- **UEFI boot only.** The image's own `EFI/BOOT/BOOT*.EFI` tree is copied, so
  modern UEFI firmware boots it. No syslinux/GRUB BIOS boot record is installed
  yet; hybrid images that must boot on legacy BIOS should use DD mode.
- **FAT32 only; one ESP.** GPT mode writes a single EFI System Partition. NTFS
  and a separate data partition for very large trees are not implemented yet.
- **Linux/BSD distributions only.** Windows installation media needs wimlib and
  split-WIM support, which is not implemented yet.
- **UDF-only images are rejected.** Hybrid ISO/UDF images are extracted from
  their ISO 9660/Joliet tree with a warning; UDF-only media needs DD mode or a
  future UDF reader.
- **FAT32 labels are limited to 11 characters.** When the ISO volume ID is
  longer, extraction rewrites `LABEL=` and GRUB `search` references in boot
  configuration files to the actual FAT label (for example Fedora's
  `Fedora-E-dvd-x86_64-44` becomes `FEDORA-E-DV`).
- **Rock Ridge symlinks** are detected from direct `SL` entries and from SUSP
  `CE` continuation areas, then skipped with a warning because FAT32 has no
  symlinks. Use DD mode if a boot path depends on a symlink.
- **Space is checked before the partition table is touched.** Extraction
  estimates the FAT32 data area, directory-entry slots and long-file-name
  overhead and rejects a target that is too small with an explicit error.
- **4Kn and Unicode names:** file-backed 4 KiB logical-sector targets are tested,
  and FAT names are normalized to NFC and sanitized for reserved names, trailing
  dots/spaces and case-only collisions.
- **`--output` is for tests.** Extraction writes into an existing pre-sized file
  of at least `ISO size + 16 MiB` (never less than 64 MiB); create it with
  `mkfile 256m` (macOS) or `truncate -s 256M` (Linux). Writing to a real device
  needs no extra option.

With `--verify`, every extracted file is read back from FAT32 and hashed; the
command fails at the first size or content mismatch. The generated volume is
also compatible with `hdiutil`, `fsck_msdos` and Windows' FAT driver.

### GUI

```console
$ image2usb-gui
```

The GUI has the same safety rules as the CLI: only removable devices are
offered, the plan is shown before writing, and the write is re-validated before
the first byte is written.

## Building

```console
$ cargo build --release
$ target/release/image2usb --help
```

Requires Rust 1.88+ (edition 2024).

## Safety

Writing an ISO to the wrong device destroys data. image2usb:

1. never lists internal/system disks as targets;
2. re-validates model, serial and size before opening a device;
3. refuses to write without explicit confirmation (`--yes` or typing the device serial);
4. reports the exact byte offset of any write error.

## License

GPL-3.0-or-later. See [LICENSE](LICENSE) and [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).
