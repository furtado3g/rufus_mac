# Third-party notices

image2usb is licensed under the GNU General Public License v3.0 or later.

This project embeds or redistributes the following third-party components.
Exact versions and corresponding source code are published with every release.

| Component | License | Use |
| --- | --- | --- |
| Rust standard library and crates listed in `Cargo.lock` | MIT / Apache-2.0 (mostly) | runtime |
| fatfs 0.3 | MIT | FAT32 formatting and file writing for extraction mode |
| syslinux | GPL-2.0-or-later | BIOS bootloader for extraction mode (planned) |
| GRUB 2 | GPL-3.0-or-later | BIOS/UEFI bootloader for extraction mode (planned) |
| UEFI:NTFS | GPL-3.0-or-later | NTFS boot support for UEFI (planned) |
| wimlib | LGPL-3.0-or-later | WIM splitting for Windows images (planned) |

At the time of writing, only Rust crates are bundled. The bootloader and WIM
components above are planned and will be added before the corresponding feature
is released.

Corresponding source code for GPL/LGPL components is available at:

- https://git.kernel.org/pub/scm/boot/syslinux/syslinux.git
- https://git.savannah.gnu.org/git/grub.git
- https://github.com/pbatard/uefi-ntfs
- https://wimlib.net/

Modifications, if any, are published in the `third_party/` directory of each
release archive together with build scripts.
