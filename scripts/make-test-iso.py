#!/usr/bin/env python3
"""Create a tiny synthetic ISO 9660 image for manual testing.

This is NOT a bootable image. It only contains the metadata that image2usb's
analyzer looks at: a primary volume descriptor, an El Torito catalog, a hybrid
MBR and a few Ubuntu marker files.

Usage: scripts/make-test-iso.py /tmp/test.iso [--uefi]
"""
import struct
import sys

SECTOR = 2048
SECTORS = 64


def record(name: bytes, extent: int, length: int, is_dir: bool) -> bytes:
    name_len = len(name)
    rec_len = 33 + name_len
    if rec_len % 2:
        rec_len += 1
    out = bytearray(rec_len)
    out[0] = rec_len
    out[2:6] = struct.pack("<I", extent)
    out[6:10] = struct.pack(">I", extent)
    out[10:14] = struct.pack("<I", length)
    out[14:18] = struct.pack(">I", length)
    out[18] = 124
    out[19] = 1
    out[20] = 1
    out[25] = 0x02 if is_dir else 0x00
    out[28:30] = struct.pack("<H", 1)
    out[30:32] = struct.pack(">H", 1)
    out[32] = name_len
    out[33:33 + name_len] = name
    return bytes(out)


def main() -> None:
    path = sys.argv[1] if len(sys.argv) > 1 else "/tmp/image2usb-test.iso"
    uefi = "--uefi" in sys.argv[2:]
    data = bytearray(SECTOR * SECTORS)

    # Hybrid MBR
    data[0:440] = b"\x90" * 440
    data[446] = 0x80
    data[450] = 0x17
    data[454:458] = struct.pack("<I", 1)
    data[458:462] = struct.pack("<I", 63)
    data[510:512] = b"\x55\xaa"

    # Primary volume descriptor
    pvd = bytearray(SECTOR)
    pvd[0] = 1
    pvd[1:6] = b"CD001"
    pvd[6] = 1
    pvd[40:47] = b"TESTISO"
    pvd[80:84] = struct.pack("<I", SECTORS)
    pvd[128:130] = struct.pack("<H", 2048)
    root = record(b"\x00", 20, SECTOR, True)
    pvd[156:156 + len(root)] = root
    data[16 * SECTOR:17 * SECTOR] = pvd

    # El Torito boot record
    boot = bytearray(SECTOR)
    boot[1:6] = b"CD001"
    boot[6] = 1
    boot[7:30] = b"EL TORITO SPECIFICATION"
    boot[71:75] = struct.pack("<I", 21)
    data[17 * SECTOR:18 * SECTOR] = boot

    # Terminator
    term = bytearray(SECTOR)
    term[0] = 255
    term[1:6] = b"CD001"
    term[6] = 1
    data[18 * SECTOR:19 * SECTOR] = term

    # El Torito catalog
    catalog = bytearray(SECTOR)
    catalog[0] = 1
    catalog[1] = 0xEF if uefi else 0x00
    catalog[4:12] = b"TESTBOOT"
    catalog[30] = 0x55
    catalog[31] = 0xAA
    checksum = (-sum(struct.unpack("<16H", catalog[:32]))) & 0xFFFF
    catalog[28:30] = struct.pack("<H", checksum)
    catalog[32] = 0x88
    catalog[38:40] = struct.pack("<H", 4)
    catalog[40:44] = struct.pack("<I", 30)
    data[21 * SECTOR:22 * SECTOR] = catalog

    # Directories
    def write_dir(sector: int, parent: int, entries: list) -> None:
        offset = 0
        items = [record(b"\x00", sector, SECTOR, True), record(b"\x01", parent, SECTOR, True)]
        items += entries
        for item in items:
            start = sector * SECTOR + offset
            data[start:start + len(item)] = item
            offset += len(item)

    write_dir(20, 20, [
        record(b"EFI", 22, SECTOR, True),
        record(b"ISOLINUX", 23, SECTOR, True),
        record(b"CASPER", 24, SECTOR, True),
        record(b".DISK", 26, SECTOR, True),
    ])
    write_dir(22, 20, [record(b"BOOT", 25, SECTOR, True)])
    write_dir(23, 20, [record(b"ISOLINUX.CFG", 32, 10, False)])
    write_dir(24, 20, [])
    write_dir(25, 22, [record(b"BOOTX64.EFI", 31, 4, False)])
    write_dir(26, 20, [record(b"INFO", 33, 16, False)])

    data[31 * SECTOR:31 * SECTOR + 4] = b"MZ\x90\x00"
    data[32 * SECTOR:32 * SECTOR + 10] = b"default 0\n"
    data[33 * SECTOR:33 * SECTOR + 16] = b"Ubuntu 24.04 LTS"

    with open(path, "wb") as handle:
        handle.write(data)
    print(path)


if __name__ == "__main__":
    main()
