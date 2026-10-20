//! Analysis tests against a synthetic ISO 9660 image, so the suite needs no
//! network access and no multi-hundred-megabyte fixture.

use std::fs::OpenOptions;
use std::io::{Seek, SeekFrom, Write};
use std::path::Path;

use image2usb_iso::{Distro, analyze_path};

const SECTOR: usize = 2048;
const SECTORS: usize = 64;

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
        file.seek(SeekFrom::Start(0)).unwrap();
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

fn write_root_directory(builder: &mut Builder, sector: usize) {
    let mut offset = 0;
    let entries = vec![
        dir_record(&[0x00], sector as u32, SECTOR as u32, true),
        dir_record(&[0x01], sector as u32, SECTOR as u32, true),
        dir_record(b"EFI", 22, SECTOR as u32, true),
        dir_record(b"ISOLINUX", 23, SECTOR as u32, true),
        dir_record(b"CASPER", 24, SECTOR as u32, true),
        dir_record(b".DISK", 26, SECTOR as u32, true),
    ];
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
    catalog[4..4 + 8].copy_from_slice(b"TESTBOOT");
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

fn build_image(path: &Path, platform: u8, hybrid: bool) {
    let mut b = Builder::new();
    write_mbr(&mut b, true, hybrid);
    write_pvd(&mut b);
    write_boot_record(&mut b);
    write_terminator(&mut b);
    write_catalog(&mut b, platform);

    write_root_directory(&mut b, 20);
    write_subdirectory(&mut b, 22, 20, &[(b"BOOT", 25, SECTOR as u32, true)]);
    write_subdirectory(&mut b, 23, 20, &[(b"ISOLINUX.CFG", 32, 10, false)]);
    write_subdirectory(&mut b, 24, 20, &[]);
    write_subdirectory(&mut b, 25, 22, &[(b"BOOTX64.EFI", 31, 4, false)]);
    write_subdirectory(&mut b, 26, 20, &[(b"INFO", 33, 16, false)]);

    b.data[31 * SECTOR..31 * SECTOR + 4].copy_from_slice(b"MZ\x90\x00");
    b.data[32 * SECTOR..32 * SECTOR + 10].copy_from_slice(b"default 0\n");
    b.data[33 * SECTOR..33 * SECTOR + 16].copy_from_slice(b"Ubuntu 24.04 LTS");
    b.build(path);
}

#[test]
fn analyzes_bios_hybrid_image() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("bios.iso");
    build_image(&path, 0x00, true);

    let info = analyze_path(&path).unwrap();
    assert_eq!(info.volume_id.as_deref(), Some("TESTISO"));
    assert_eq!(info.distro, Distro::Ubuntu);
    assert!(info.mbr.as_ref().unwrap().isohybrid_bootable());
    let el = info.el_torito.as_ref().unwrap();
    assert!(el.bios);
    assert!(!el.uefi);
    assert!(info.dd_bios);
    assert!(info.dd_safe());
    assert!(info.boot_files.isolinux.is_some());
    assert!(
        info.boot_files
            .efi_boot
            .iter()
            .any(|p| p.ends_with("bootx64.efi"))
    );
    assert!(!info.has_joliet);
    assert!(!info.has_udf);
}

#[test]
fn analyzes_uefi_image() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("uefi.iso");
    build_image(&path, 0xEF, true);

    let info = analyze_path(&path).unwrap();
    let el = info.el_torito.as_ref().unwrap();
    assert!(!el.bios);
    assert!(el.uefi);
    assert!(info.dd_uefi);
    assert!(info.dd_safe());
}

#[test]
fn non_hybrid_image_requires_extraction() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("plain.iso");
    build_image(&path, 0x00, false);

    let info = analyze_path(&path).unwrap();
    assert!(!info.dd_bios);
    assert!(!info.dd_uefi);
    assert!(!info.dd_safe());
    assert!(!info.warnings.is_empty());
}

#[test]
fn rejects_non_iso_files() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("not.iso");
    std::fs::write(&path, vec![0u8; 200 * 1024]).unwrap();
    assert!(analyze_path(&path).is_err());
}
