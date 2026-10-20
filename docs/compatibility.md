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
| UEFI x86_64 (OVMF) | QEMU smoke test | phase 0.1 |
| Legacy BIOS (SeaBIOS) | QEMU smoke test | phase 0.4 |
| UEFI aarch64 (AAVMF) | QEMU | phase 0.3 |
| UEFI-only laptop | physical hardware | phase 0.1 |
| Legacy-only desktop | physical hardware | phase 0.4 |
| Apple Silicon Mac (UEFI) | physical hardware | phase 0.1 |

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
FAT32 mounts, `fsck_msdos` clean, file hashes match after read-back). Booting it
on real firmware is still on the verification list below.

## QEMU smoke test (planned for phase 0.1)

```console
$ qemu-system-x86_64 -m 2G -bios /usr/share/ovmf/OVMF.fd \
    -drive file=usb.img,format=raw -serial file:boot.log -display none
```

The CI job asserts that the installer kernel prints its banner on the serial
console within a timeout. A booting GRUB menu alone is not considered success.
