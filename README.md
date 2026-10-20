# image2usb

Create bootable USB drives from ISO images on **macOS**, **Linux** and **Windows**.

image2usb is a portable, cross-platform alternative to Rufus written in Rust. It
ships as a self-contained executable / app bundle: no installer, no runtime, no
external tools required.

> Status: **pre-alpha** (Phase 0.3 — extraction). Device enumeration, ISO
> analysis, DD-mode writing and UEFI extraction are implemented. GPT, BIOS
> bootloaders and Windows media are still in progress.

See [ROADMAP.md](ROADMAP.md) for the detailed plan, development setup on another
machine, and per-milestone acceptance criteria.

## Features (roadmap)

- [x] Safe device enumeration (USB / SD only, system disks blocked)
- [x] ISO analysis: ISO9660, Joliet, El Torito (BIOS/UEFI), hybrid detection
- [x] DD mode: write a hybrid ISO byte-for-byte, with progress, cancel and SHA-256 verification
- [x] Extraction mode: MBR + FAT32 + files copied from the ISO tree (UEFI boot, Linux distros)
- [ ] GPT extraction and NTFS data partitions
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
```

`image2usb` must run with administrator/root privileges to write to a raw device.
The GUI and `list`/`analyze` work unprivileged.

On Windows the CLI manifest uses `asInvoker`, so `list` and `analyze` run without
a UAC prompt; start the terminal (or the executable) as Administrator before
`write`. The GUI manifest requests elevation at launch, so writes work directly.

### Extraction mode

`--mode extract` creates an MBR partition table with one FAT32 partition and
copies the ISO tree. It is the fallback for bootable images that cannot be
written 1:1 (non-hybrid ISOs):

```console
$ sudo image2usb write ubuntu-24.04.iso --device /dev/disk4 --mode extract --verify
```

Current limits (Phase 0.3):

- **UEFI boot only.** The image's own `EFI/BOOT/BOOT*.EFI` tree is copied, so
  modern UEFI firmware boots it. No syslinux/GRUB BIOS boot record is installed
  yet; hybrid images that must boot on legacy BIOS should use DD mode.
- **MBR + FAT32 only.** GPT and NTFS are rejected with an explicit error.
- **Linux/BSD distributions only.** Windows installation media needs wimlib and
  split-WIM support, which is not implemented yet.
- **Rock Ridge symlinks** are copied as empty regular files (the ISO reader does
  not resolve them yet). Boot files are regular files, so this does not affect
  UEFI boot, but a few convenience links may be empty.
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
