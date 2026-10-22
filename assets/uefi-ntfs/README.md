# UEFI:NTFS boot image

`uefi-ntfs.img` is copied byte-for-byte from the Rufus repository at commit
`2350489fe902bdc71d37f255a7684fe06a0ab2d0`. It is a 1 MiB FAT disk image
containing the signed UEFI:NTFS chainloader used to boot the Windows
installation files from an NTFS data partition.

- Upstream URL: <https://raw.githubusercontent.com/pbatard/rufus/2350489fe902bdc71d37f255a7684fe06a0ab2d0/res/uefi/uefi-ntfs.img>
- SHA-256: `72683fa1250eeea772d3399277b434d4e55ba8dd0dc926e52d817e701fc2eb9e`
- Size: `1048576` bytes
- Contents: `EFI/Boot/boot{ia32,x64,aa64,arm,riscv64}.efi`,
  `EFI/Rufus/ntfs_*.efi`, `EFI/Rufus/exfat_*.efi` and the upstream `README.txt`
- Upstream project: <https://github.com/pbatard/uefi-ntfs>

## Redistribution status

The image bundles the UEFI:NTFS loader, a signed NTFS driver and EfiFs. Their
licenses and corresponding-source obligations require the file-level audit in
`assets/windows-media/manifest.json` before a public release. The pinned image
is used here for local integration work and must not be presented as a
completed license review.

## Do not mount the image read-write

macOS writes `.fseventsd` metadata when this image is mounted read-write,
which changes the pinned bytes and SHA-256. Verify the downloaded file before
copying it into the repository, and never attach it with write access:

```sh
shasum -a 256 assets/uefi-ntfs/uefi-ntfs.img
# 72683fa1250eeea772d3399277b434d4e55ba8dd0dc926e52d817e701fc2eb9e
```

The extractor embeds the image with `include_bytes!`; it does not read this
path at runtime.
