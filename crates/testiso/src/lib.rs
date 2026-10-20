//! Builds a tiny synthetic ISO 9660 image for tests.
//!
//! The image contains an El Torito catalog, a hybrid MBR (optional), and the
//! same directory tree the extraction tests expect:
//!
//! ```text
//! EFI/BOOT/BOOTX64.EFI   (4 bytes)
//! ISOLINUX/ISOLINUX.CFG  (10 bytes)
//! CASPER/                (empty)
//! .DISK/INFO             (16 bytes)
//! ```

#![forbid(unsafe_code)]

use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;

const SECTOR: usize = 2048;
const SECTORS: usize = 64;

pub const EFI_BOOT: &[u8] = b"MZ\x90\x00";
pub const ISOLINUX_CFG: &[u8] = b"default 0\n";
pub const DISK_INFO: &[u8] = b"Ubuntu 24.04 LTS";

struct Builder {
    data: Vec<u8>,
}

impl Builder {
    fn new() -> Self {
        Self {
            data: vec![0u8; SECTOR * SECTORS],
        }
    }

    fn write_sector(&mut self, sector: usize, bytes: &[u8]) {
        let start = sector * SECTOR;
        self.data[start..start + bytes.len()].copy_from_slice(bytes);
    }

    fn build(self, path: &Path) {
        let mut file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(path)
            .unwrap();
        file.write_all(&self.data).unwrap();
    }
}

fn dir_record(name: &[u8], extent: u32, len: u32, is_dir: bool) -> Vec<u8> {
    let name_len = name.len();
    let mut record_len = 33 + name_len;
    if !record_len.is_multiple_of(2) {
        record_len += 1;
    }
    let mut record = vec![0u8; record_len];
    record[0] = record_len as u8;
    record[2..6].copy_from_slice(&extent.to_le_bytes());
    record[6..10].copy_from_slice(&extent.to_be_bytes());
    record[10..14].copy_from_slice(&len.to_le_bytes());
    record[14..18].copy_from_slice(&len.to_be_bytes());
    record[18] = 124; // 2024
    record[19] = 1;
    record[20] = 1;
    record[25] = if is_dir { 0x02 } else { 0x00 };
    record[28..30].copy_from_slice(&1u16.to_le_bytes());
    record[30..32].copy_from_slice(&1u16.to_be_bytes());
    record[32] = name_len as u8;
    record[33..33 + name_len].copy_from_slice(name);
    record
}

fn write_root_directory(builder: &mut Builder, sector: usize, extra: &[Vec<u8>]) {
    let mut offset = 0;
    let mut entries = vec![
        dir_record(&[0x00], sector as u32, SECTOR as u32, true),
        dir_record(&[0x01], sector as u32, SECTOR as u32, true),
        dir_record(b"EFI", 22, SECTOR as u32, true),
        dir_record(b"ISOLINUX", 23, SECTOR as u32, true),
        dir_record(b"CASPER", 24, SECTOR as u32, true),
        dir_record(b".DISK", 26, SECTOR as u32, true),
    ];
    entries.extend(extra.iter().cloned());
    for entry in entries {
        let start = sector * SECTOR + offset;
        builder.data[start..start + entry.len()].copy_from_slice(&entry);
        offset += entry.len();
    }
}

fn write_subdirectory(
    builder: &mut Builder,
    sector: usize,
    parent: u32,
    entries: &[(&[u8], u32, u32, bool)],
) {
    let mut offset = 0;
    let mut records = vec![
        dir_record(&[0x00], sector as u32, SECTOR as u32, true),
        dir_record(&[0x01], parent, SECTOR as u32, true),
    ];
    for (name, extent, len, is_dir) in entries {
        records.push(dir_record(name, *extent, *len, *is_dir));
    }
    for record in records {
        let start = sector * SECTOR + offset;
        builder.data[start..start + record.len()].copy_from_slice(&record);
        offset += record.len();
    }
}

fn write_pvd(builder: &mut Builder) {
    let mut pvd = [0u8; SECTOR];
    pvd[0] = 1;
    pvd[1..6].copy_from_slice(b"CD001");
    pvd[6] = 1;
    pvd[40..47].copy_from_slice(b"TESTISO");
    pvd[80..84].copy_from_slice(&(SECTORS as u32).to_le_bytes());
    pvd[128..130].copy_from_slice(&2048u16.to_le_bytes());
    let root = dir_record(&[0x00], 20, SECTOR as u32, true);
    pvd[156..156 + root.len()].copy_from_slice(&root);
    builder.write_sector(16, &pvd);
}

fn write_terminator(builder: &mut Builder) {
    let mut descriptor = [0u8; SECTOR];
    descriptor[0] = 255;
    descriptor[1..6].copy_from_slice(b"CD001");
    descriptor[6] = 1;
    builder.write_sector(18, &descriptor);
}

fn write_boot_record(builder: &mut Builder) {
    let mut record = [0u8; SECTOR];
    record[0] = 0;
    record[1..6].copy_from_slice(b"CD001");
    record[6] = 1;
    record[7..30].copy_from_slice(b"EL TORITO SPECIFICATION");
    record[71..75].copy_from_slice(&21u32.to_le_bytes());
    builder.write_sector(17, &record);
}

fn write_catalog(builder: &mut Builder, platform: u8) {
    let mut catalog = [0u8; SECTOR];
    catalog[0] = 0x01;
    catalog[1] = platform;
    catalog[4..12].copy_from_slice(b"TESTBOOT");
    catalog[30] = 0x55;
    catalog[31] = 0xAA;
    let sum: u32 = catalog[..32]
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| u16::from_le_bytes(*c) as u32)
        .sum();
    let checksum = 0u16.wrapping_sub(sum as u16);
    catalog[28..30].copy_from_slice(&checksum.to_le_bytes());

    catalog[32] = 0x88;
    catalog[38..40].copy_from_slice(&4u16.to_le_bytes());
    catalog[40..44].copy_from_slice(&30u32.to_le_bytes());
    builder.write_sector(21, &catalog);
}

fn write_mbr(builder: &mut Builder, bootable: bool, hybrid: bool) {
    let mut mbr = [0u8; 512];
    if hybrid {
        mbr[..440].fill(0x90);
        let base = 446;
        mbr[base] = if bootable { 0x80 } else { 0x00 };
        mbr[base + 4] = 0x17;
        mbr[base + 8..base + 12].copy_from_slice(&1u32.to_le_bytes());
        mbr[base + 12..base + 16].copy_from_slice(&63u32.to_le_bytes());
    }
    mbr[510] = 0x55;
    mbr[511] = 0xAA;
    builder.data[..512].copy_from_slice(&mbr);
}

pub fn build_image(path: &Path, platform: u8, hybrid: bool) {
    build_image_with_root_extra(path, platform, hybrid, Vec::new());
}

/// Build the standard fixture plus a Rock Ridge symlink at the root.
pub fn build_image_with_symlink(path: &Path, platform: u8, hybrid: bool) {
    build_image_with_root_extra(
        path,
        platform,
        hybrid,
        vec![symlink_record(b"BOOTLINK", "EFI/BOOT/BOOTX64.EFI")],
    );
}

/// Build the standard fixture plus a file record whose declared size is larger
/// than any test target. Extraction must reject it during preflight, before it
/// writes the MBR.
pub fn build_image_with_large_file(path: &Path, len: u32) {
    build_image_with_root_extra(
        path,
        0xEF,
        true,
        vec![dir_record(b"HUGE.BIN", 34, len, false)],
    );
}

fn build_image_with_root_extra(path: &Path, platform: u8, hybrid: bool, extra: Vec<Vec<u8>>) {
    let mut b = Builder::new();
    write_mbr(&mut b, true, hybrid);
    write_pvd(&mut b);
    write_boot_record(&mut b);
    write_terminator(&mut b);
    write_catalog(&mut b, platform);

    write_root_directory(&mut b, 20, &extra);
    write_subdirectory(&mut b, 22, 20, &[(b"BOOT", 25, SECTOR as u32, true)]);
    write_subdirectory(&mut b, 23, 20, &[(b"ISOLINUX.CFG", 32, 10, false)]);
    write_subdirectory(&mut b, 24, 20, &[]);
    write_subdirectory(&mut b, 25, 22, &[(b"BOOTX64.EFI", 31, 4, false)]);
    write_subdirectory(&mut b, 26, 20, &[(b"INFO", 33, 16, false)]);

    b.data[31 * SECTOR..31 * SECTOR + EFI_BOOT.len()].copy_from_slice(EFI_BOOT);
    b.data[32 * SECTOR..32 * SECTOR + ISOLINUX_CFG.len()].copy_from_slice(ISOLINUX_CFG);
    b.data[33 * SECTOR..33 * SECTOR + DISK_INFO.len()].copy_from_slice(DISK_INFO);
    b.build(path);
}

fn symlink_record(name: &[u8], target: &str) -> Vec<u8> {
    let name_len = name.len();
    let mut record = dir_record(name, 0, 0, false);
    debug_assert_eq!(record.len(), (33 + name_len).next_multiple_of(2));

    // SP marks the System Use area as SUSP/Rock Ridge.
    record.extend_from_slice(&[b'S', b'P', 7, 1, 0xBE, 0xEF, 0]);

    // A single normal component; the target itself may contain `/`.
    let mut sl = vec![b'S', b'L', 0, 1, 0];
    sl.push((target.len() + 2) as u8);
    sl.push(0);
    sl.extend_from_slice(target.as_bytes());
    sl[2] = sl.len() as u8;
    record.extend_from_slice(&sl);
    if !record.len().is_multiple_of(2) {
        record.push(0);
    }
    record[0] = record.len() as u8;
    record
}
