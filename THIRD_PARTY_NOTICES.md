# Third-party notices

image2usb is licensed under the GNU General Public License v3.0 or later.

This project embeds or redistributes the following third-party components.
Exact versions and corresponding source code are published with every release.

| Component | License | Use |
| --- | --- | --- |
| Rust standard library and crates listed in `Cargo.lock` | MIT / Apache-2.0 (mostly) | runtime |
| fatfs 0.3 | MIT | FAT32 formatting and file writing for extraction mode |
| GRUB 2.14 i386-pc BIOS assets (`boot.img`, `core-mbr.img`, `core-gpt.img`) | GPL-3.0-or-later | legacy BIOS bootloader embedded and installed in the pre-partition gap |
| UEFI:NTFS image (`assets/uefi-ntfs/uefi-ntfs.img`) | Mixed: loader GPL-2.0-or-later; NTFS driver and EfiFs require file-level audit | NTFS boot support for UEFI; embedded only in opt-in `windows-media` builds |
| wimlib | LGPL-3.0-or-later | WIM splitting for Windows images (planned) |

At the time of writing, Rust crates, the GRUB BIOS assets and the pinned
UEFI:NTFS image are bundled. The UEFI:NTFS component licenses and corresponding
sources are not yet cleared for public redistribution; see
`assets/uefi-ntfs/README.md` and `assets/windows-media/manifest.json`. The WIM
component is still planned.

The GRUB cores embed a small module set (`biosdisk`, `part_msdos`, `part_gpt`,
`fat`, `search`, `configfile`, `normal`, `linux`, `echo`, `test`) and
`assets/grub-bios/early.cfg`. They do not depend on external GRUB modules at
runtime. The exact files, SHA-256 hashes and build commands are recorded in
[`assets/grub-bios/README.md`](assets/grub-bios/README.md). `boot.img` is the
unmodified Alpine Linux `grub-bios-2.14-r0` file; the cores were generated with
`grub-mkimage` from the matching GRUB 2.14 packages.

Corresponding source code for GPL/LGPL components is available at:

- GRUB upstream: https://git.savannah.gnu.org/cgit/grub.git
- Alpine GRUB packaging: https://gitlab.alpinelinux.org/alpine/aports/-/tree/master/main/grub
- UEFI:NTFS: https://github.com/pbatard/uefi-ntfs
- wimlib: https://wimlib.net/

The GRUB source and build configuration used for these assets are available
from the links above and are described in `assets/grub-bios/README.md`. If you
need a copy of the exact corresponding source, open an issue at
https://github.com/furtado3g/rufus_mac/issues. Modifications to GRUB itself are
not required; `early.cfg` and the placement code are part of image2usb and are
released under GPL-3.0-or-later.
