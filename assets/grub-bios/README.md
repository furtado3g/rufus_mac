# GRUB BIOS boot assets

These are the legacy BIOS boot assets used by extraction mode. They are written
outside the FAT32 partition, in the free space between the partition table and
the first partition.

| File | SHA-256 |
| --- | --- |
| `boot.img` | `6343b7e9f06388566ea5b6e8a3535fbaec1f695a0b3793caee5386237d4d3450` |
| `core-mbr.img` | `5ad12325bfc9396b6fd09f81fa7836e6450e0d56fbed5f3f817056f5ea2c3fa8` |
| `core-gpt.img` | `7993cfc097b1fa8fcd5231b7645b44bbb7bac38dd180fe42370a3c65e7aa2f18` |

`core-mbr.img` uses the GRUB prefix `(hd0,msdos1)/boot/grub`; `core-gpt.img`
uses `(hd0,gpt1)/boot/grub`. Both embed `early.cfg`, which searches for either
`/boot/grub/grub.cfg` or `/boot/grub2/grub.cfg` on the FAT32 partition and then
loads the distribution's own GRUB configuration.

## Provenance

- GRUB version: 2.14
- Source package: Alpine Linux 3.24.2 `grub-2.14-r0` and
  `grub-bios-2.14-r0`, `x86_64` repository:
  <https://dl-cdn.alpinelinux.org/alpine/v3.24/main/x86_64/>
- Build environment: Alpine Linux 3.24.2 live system in QEMU, with the package
  installed with `apk add grub grub-bios`.
- `boot.img` is the unmodified `/usr/lib/grub/i386-pc/boot.img` from that
  package.

The two core images were generated on 2026-10-20 with:

```console
grub-mkimage -O i386-pc -o core-mbr.img \
  -p '(hd0,msdos1)/boot/grub' -c early.cfg \
  biosdisk part_msdos part_gpt fat search configfile normal linux echo test

grub-mkimage -O i386-pc -o core-gpt.img \
  -p '(hd0,gpt1)/boot/grub' -c early.cfg \
  biosdisk part_msdos part_gpt fat search configfile normal linux echo test
```

The exact embedded configuration is [`early.cfg`](early.cfg).

## Runtime placement

`image2usb` writes `boot.img` to LBA 0 while preserving the target disk
signature and partition table. It writes a padded core image at LBA 34 (the
first sector after the 128-entry GPT array), sets the boot image's kernel sector
to 34, and patches the core's diskboot blocklist to start at LBA 35. This works
for both MBR and GPT layouts because the partition starts at 1 MiB.

## License and source

GRUB is free software, licensed under the GNU General Public License v3.0 or
later. The generated glue configuration and the placement code in image2usb are
also GPL-3.0-or-later. See the repository root [`LICENSE`](../../LICENSE) and
[`THIRD_PARTY_NOTICES.md`](../../THIRD_PARTY_NOTICES.md).

Corresponding GRUB source code:

- GRUB 2.14 release tarball: <https://ftp.gnu.org/gnu/grub/grub-2.14.tar.xz>
- Upstream: <https://git.savannah.gnu.org/cgit/grub.git>
- Alpine packaging: <https://gitlab.alpinelinux.org/alpine/aports/-/tree/master/main/grub>

The exact build commands and configuration are recorded above. A source archive
for the Alpine package is available from the Alpine repository and is included
in release source offers. If you need a copy of the corresponding source, open
an issue at <https://github.com/furtado3g/rufus_mac/issues>.
