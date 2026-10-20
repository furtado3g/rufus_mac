# image2usb

Create bootable USB drives from ISO images on **macOS**, **Linux** and **Windows**.

image2usb is a portable, cross-platform alternative to Rufus written in Rust. It
ships as a self-contained executable / app bundle: no installer, no runtime, no
external tools required.

> Status: **pre-alpha** (Phase 0 — foundation). Device enumeration, ISO analysis
> and DD-mode writing are implemented. Extraction mode is in progress.

## Features (roadmap)

- [x] Safe device enumeration (USB / SD only, system disks blocked)
- [x] ISO analysis: ISO9660, Joliet, El Torito (BIOS/UEFI), hybrid detection
- [x] DD mode: write a hybrid ISO byte-for-byte, with progress, cancel and SHA-256 verification
- [ ] Extraction mode: MBR/GPT + FAT32 + UEFI boot files
- [ ] BIOS boot: syslinux, GRUB2, Windows BOOTMGR
- [ ] Windows 11 unattended options (TPM / Secure Boot / offline account bypass)
- [x] Graphical interface (egui): device picker, ISO analysis, DD write with progress
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
```

`image2usb` must run with administrator/root privileges to write to a raw device.
The GUI and `list`/`analyze` work unprivileged.

On Windows the CLI manifest uses `asInvoker`, so `list` and `analyze` run without
a UAC prompt; start the terminal (or the executable) as Administrator before
`write`. The GUI manifest requests elevation at launch, so writes work directly.

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
