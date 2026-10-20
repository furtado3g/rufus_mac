# Compatibility and verification

## Supported targets

| OS | Minimum | Architectures | Artifact |
| --- | --- | --- | --- |
| macOS | 12 (Monterey) | x86_64, aarch64 (universal2) | `.app` in `.dmg` / `.zip` |
| Windows | 10 1909 | x64, ARM64 | `.zip` with `.exe` |
| Linux | glibc 2.35 (Ubuntu 22.04) | x86_64, aarch64 | `.tar.gz`, AppImage (0.5) |

## Firmware matrix

| Target | Verified by | Status |
| --- | --- | --- |
| UEFI x86_64 (OVMF) | QEMU boot, extraction + DD control | **verified 2026-10-20** (live desktop) |
| Legacy BIOS (SeaBIOS) | QEMU smoke test | extraction not implemented; DD only |
| UEFI aarch64 (AAVMF) | QEMU | untested (CI build only) |
| UEFI-only laptop | physical hardware | pending |
| Legacy-only desktop | physical hardware | pending; DD only |
| Apple Silicon Mac (UEFI) | physical hardware | pending |

The QEMU/OVMF extraction and DD control run, including screenshots, serial logs
and hashes, is recorded in
[validation/uefi-2026-10-20.md](validation/uefi-2026-10-20.md).

## Verifying a written device without booting

1. Compare hashes: `image2usb write --verify` reads the device back and compares
   every byte, reporting the first differing offset. In extraction mode it reads
   every copied file back and compares its SHA-256.
2. Inspect the partition table: `sudo fdisk -l /dev/sdX`,
   `sudo sgdisk --verify /dev/sdX`, `diskutil list diskX`.
3. Mount read-only and compare file trees: `mtools`/`7z l` for FAT32,
   `findmnt` for ISO 9660.
4. Check the generated FAT32 volume with `fsck_msdos -n /dev/rdiskN` (macOS) or
   `fsck.vfat -n /dev/sdXN` (Linux).

Phase 0.3 extraction has been validated structurally (MBR recognized by macOS,
FAT32 mounts, `fsck_msdos` clean, file hashes match after read-back). A 3.0 GiB
Ubuntu derivative extracted to a local image at about 103 MiB/s and verified at
about 123 MiB/s; all 290 files listed in the ISO's own `md5sum.txt` matched.
The same image boots through OVMF/GRUB to the live desktop in QEMU; physical
firmware and BIOS boot are still on the verification list.

## Booting an extracted image in QEMU (UEFI)

This validates the whole chain (MBR → FAT32 → `EFI/BOOT/BOOT*.EFI` → GRUB)
without touching a physical USB drive. Install QEMU/OVMF first. On an Intel Mac
the recommended setup is MacPorts (`sudo port install qemu`; Homebrew's `qemu`
also works), and on Debian/Ubuntu it is `qemu-system-x86` + `ovmf`:

```console
$ QEMU_PREFIX=/opt/local/share/qemu          # Homebrew: "$(brew --prefix)/share/qemu"
$ cp "$QEMU_PREFIX/edk2-i386-vars.fd" /tmp/ovmf-vars.fd
$ qemu-system-x86_64 \
    -machine q35,accel=hvf -cpu host -m 4096 -smp 4 \
    -drive if=pflash,format=raw,readonly=on,file="$QEMU_PREFIX/edk2-x86_64-code.fd" \
    -drive if=pflash,format=raw,file=/tmp/ovmf-vars.fd \
    -drive if=none,id=usb,format=raw,snapshot=on,file=/tmp/real-extract.img \
    -device qemu-xhci -device usb-storage,drive=usb,bootindex=0 \
    -vga std -display vnc=127.0.0.1:1
```

Connect a VNC client to `127.0.0.1:1`. OVMF enumerates the FAT32 partition,
loads `EFI/BOOT/BOOTX64.EFI`, and GRUB reads `/boot/grub/grub.cfg` and the
`/casper` kernel/initrd. On Linux replace `hvf` with `kvm` and point the pflash
drives at `/usr/share/OVMF/OVMF_CODE.fd` and `OVMF_VARS.fd`; use `-accel tcg`
when hardware acceleration is unavailable.

## CI smoke test (planned)

The CI job will boot the same image with OVMF in headless mode and assert that
the installer kernel prints a banner on the serial console within a timeout:

```console
$ qemu-system-x86_64 -m 2G -bios /usr/share/ovmf/OVMF.fd \
    -drive file=usb.img,format=raw -serial file:boot.log -display none
```

A booting GRUB menu alone is not considered success for CI; the kernel banner is.
The manual VNC procedure above is enough to verify that the extracted FAT32 tree
is found by firmware and that GRUB loads.
