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
pub const GRUB_CFG: &[u8] = b"set timeout=1\nmenuentry \"Test image\" {\n    echo test\n}\n";

struct Builder {
    data: Vec<u8>,
}

impl Builder {
    fn new() -> Self {
        Self::with_sectors(SECTORS)
    }

    fn with_sectors(sectors: usize) -> Self {
        Self {
            data: vec![0u8; SECTOR * sectors],
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
    let mut entries = vec![
        dir_record(&[0x00], sector as u32, SECTOR as u32, true),
        dir_record(&[0x01], sector as u32, SECTOR as u32, true),
        dir_record(b"EFI", 22, SECTOR as u32, true),
        dir_record(b"ISOLINUX", 23, SECTOR as u32, true),
        dir_record(b"CASPER", 24, SECTOR as u32, true),
        dir_record(b".DISK", 26, SECTOR as u32, true),
    ];
    entries.extend(extra.iter().cloned());
    write_directory_records(builder, sector, &entries);
}

fn write_directory_records(builder: &mut Builder, sector: usize, records: &[Vec<u8>]) {
    let mut offset = 0;
    for entry in records {
        let start = sector * SECTOR + offset;
        builder.data[start..start + entry.len()].copy_from_slice(entry);
        offset += entry.len();
    }
}

fn write_subdirectory(
    builder: &mut Builder,
    sector: usize,
    parent: u32,
    entries: &[(&[u8], u32, u32, bool)],
) {
    let mut records = vec![
        dir_record(&[0x00], sector as u32, SECTOR as u32, true),
        dir_record(&[0x01], parent, SECTOR as u32, true),
    ];
    for (name, extent, len, is_dir) in entries {
        records.push(dir_record(name, *extent, *len, *is_dir));
    }
    write_directory_records(builder, sector, &records);
}

fn write_pvd(builder: &mut Builder) {
    write_pvd_for(builder, SECTORS, "TESTISO");
}

fn write_pvd_for(builder: &mut Builder, sectors: usize, volume_id: &str) {
    let mut pvd = [0u8; SECTOR];
    pvd[0] = 1;
    pvd[1..6].copy_from_slice(b"CD001");
    pvd[6] = 1;
    pvd[40..40 + volume_id.len()].copy_from_slice(volume_id.as_bytes());
    pvd[80..84].copy_from_slice(&(sectors as u32).to_le_bytes());
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

/// Build the standard fixture plus `boot/grub/grub.cfg`, so extraction can
/// exercise the legacy BIOS installer.
pub fn build_image_with_grub(path: &Path, platform: u8, hybrid: bool) {
    build_image_with_root_extra_and_grub(path, platform, hybrid, Vec::new(), true);
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
    // The declared extent must exist in the source so the catalog can validate
    // it. Keep the tail sparse: this fixture is about destination capacity,
    // not about allocating hundreds of megabytes in the test suite.
    let file = OpenOptions::new().write(true).open(path).unwrap();
    file.set_len(34 * SECTOR as u64 + u64::from(len)).unwrap();
}

fn build_image_with_root_extra(path: &Path, platform: u8, hybrid: bool, extra: Vec<Vec<u8>>) {
    build_image_with_root_extra_and_grub(path, platform, hybrid, extra, false);
}

fn build_image_with_root_extra_and_grub(
    path: &Path,
    platform: u8,
    hybrid: bool,
    mut extra: Vec<Vec<u8>>,
    with_grub: bool,
) {
    let mut b = Builder::new();
    write_mbr(&mut b, true, hybrid);
    write_pvd(&mut b);
    write_boot_record(&mut b);
    write_terminator(&mut b);
    write_catalog(&mut b, platform);

    if with_grub {
        extra.push(dir_record(b"BOOT", 35, SECTOR as u32, true));
    }
    write_root_directory(&mut b, 20, &extra);
    write_subdirectory(&mut b, 22, 20, &[(b"BOOT", 25, SECTOR as u32, true)]);
    write_subdirectory(&mut b, 23, 20, &[(b"ISOLINUX.CFG", 32, 10, false)]);
    write_subdirectory(&mut b, 24, 20, &[]);
    write_subdirectory(&mut b, 25, 22, &[(b"BOOTX64.EFI", 31, 4, false)]);
    write_subdirectory(&mut b, 26, 20, &[(b"INFO", 33, 16, false)]);
    if with_grub {
        write_subdirectory(&mut b, 35, 20, &[(b"GRUB", 36, SECTOR as u32, true)]);
        write_subdirectory(
            &mut b,
            36,
            35,
            &[(b"GRUB.CFG", 37, GRUB_CFG.len() as u32, false)],
        );
        b.data[37 * SECTOR..37 * SECTOR + GRUB_CFG.len()].copy_from_slice(GRUB_CFG);
    }

    b.data[31 * SECTOR..31 * SECTOR + EFI_BOOT.len()].copy_from_slice(EFI_BOOT);
    b.data[32 * SECTOR..32 * SECTOR + ISOLINUX_CFG.len()].copy_from_slice(ISOLINUX_CFG);
    b.data[33 * SECTOR..33 * SECTOR + DISK_INFO.len()].copy_from_slice(DISK_INFO);
    b.build(path);
}

/// Variants used by Windows-preparation regression tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowsFixture {
    X64Wim,
    Arm64Wim,
    MissingBootmgr,
    MissingBcd,
    UnknownArchitecture,
    MultiExtentWim,
    Swm,
    Esd,
    CaseCollision,
    Symlink,
}

/// Build a small Windows-shaped ISO 9660/Joliet image.
///
/// The fixture has no real WIM payload. It exercises the boot-chain,
/// architecture and catalog paths; it is never used as evidence that Windows
/// setup boots.
pub fn build_windows_image(path: &Path, fixture: WindowsFixture) {
    const SECTORS: usize = 128;
    let mut b = Builder::with_sectors(SECTORS);
    write_mbr(&mut b, false, false);
    write_pvd_for(&mut b, SECTORS, "WIN11");
    write_boot_record(&mut b);
    write_terminator(&mut b);
    write_catalog(&mut b, 0xEF);

    let (fallback_name, machine) = match fixture {
        WindowsFixture::Arm64Wim => (b"BOOTAA64.EFI".as_slice(), 0xAA64u16),
        WindowsFixture::UnknownArchitecture => (b"BOOTX64.EFI".as_slice(), 0x1234u16),
        _ => (b"BOOTX64.EFI".as_slice(), 0x8664u16),
    };
    let include_bootmgr = fixture != WindowsFixture::MissingBootmgr;
    let include_bcd = fixture != WindowsFixture::MissingBcd;

    let mut root = vec![
        dir_record(&[0x00], 20, SECTOR as u32, true),
        dir_record(&[0x01], 20, SECTOR as u32, true),
        dir_record(b"EFI", 22, SECTOR as u32, true),
        dir_record(b"BOOT", 38, SECTOR as u32, true),
        dir_record(b"SOURCES", 39, SECTOR as u32, true),
    ];
    if include_bootmgr {
        root.push(dir_record(b"BOOTMGR", 30, 512, false));
    }
    write_directory_records(&mut b, 20, &root);

    write_subdirectory(&mut b, 22, 20, &[(b"BOOT", 25, SECTOR as u32, true)]);
    write_subdirectory(&mut b, 25, 22, &[(fallback_name, 31, 512, false)]);

    let mut boot = Vec::new();
    if include_bcd {
        boot.push((b"BCD".as_slice(), 40u32, 64u32, false));
    }
    write_subdirectory(&mut b, 38, 20, &boot);

    let idwbinfo = format!(
        "[BUILDINFO]\nBuildArch={}\nBuildNumber=19041\nBuildVersion=10.0.19041.1\n\
         EditionID=Professional\n",
        if fixture == WindowsFixture::Arm64Wim {
            12
        } else {
            9
        }
    );
    b.data[41 * SECTOR..41 * SECTOR + idwbinfo.len()].copy_from_slice(idwbinfo.as_bytes());

    let mut sources = vec![(
        b"IDWBINFO.TXT".as_slice(),
        41u32,
        idwbinfo.len() as u32,
        false,
    )];
    match fixture {
        WindowsFixture::Swm => {
            sources.push((b"INSTALL.SWM".as_slice(), 43, SECTOR as u32, false));
            sources.push((b"INSTALL2.SWM".as_slice(), 44, SECTOR as u32, false));
        }
        WindowsFixture::Esd => {
            sources.push((b"INSTALL.ESD".as_slice(), 43, 4096, false));
        }
        WindowsFixture::X64Wim
        | WindowsFixture::Arm64Wim
        | WindowsFixture::MissingBootmgr
        | WindowsFixture::MissingBcd
        | WindowsFixture::UnknownArchitecture => {
            sources.push((b"INSTALL.WIM".as_slice(), 43, 4096, false));
        }
        WindowsFixture::MultiExtentWim
        | WindowsFixture::CaseCollision
        | WindowsFixture::Symlink => {}
    }
    write_subdirectory(&mut b, 39, 20, &sources);
    if fixture == WindowsFixture::CaseCollision {
        let records = vec![
            dir_record(&[0x00], 39, SECTOR as u32, true),
            dir_record(&[0x01], 20, SECTOR as u32, true),
            dir_record(b"IDWBINFO.TXT", 41, idwbinfo.len() as u32, false),
            dir_record(b"INSTALL.WIM", 43, SECTOR as u32, false),
            dir_record(b"install.wim", 44, SECTOR as u32, false),
        ];
        write_directory_records(&mut b, 39, &records);
    }
    if fixture == WindowsFixture::Symlink {
        let records = vec![
            dir_record(&[0x00], 39, SECTOR as u32, true),
            dir_record(&[0x01], 20, SECTOR as u32, true),
            dir_record(b"IDWBINFO.TXT", 41, idwbinfo.len() as u32, false),
            dir_record(b"INSTALL.WIM", 43, 4096, false),
            symlink_record(b"LINK", "BOOTMGR"),
        ];
        write_directory_records(&mut b, 39, &records);
    }
    if fixture == WindowsFixture::MultiExtentWim {
        let mut first = dir_record(b"INSTALL.WIM", 43, SECTOR as u32, false);
        first[25] |= 0x80;
        let second = dir_record(b"INSTALL.WIM", 44, SECTOR as u32, false);
        let records = vec![
            dir_record(&[0x00], 39, SECTOR as u32, true),
            dir_record(&[0x01], 20, SECTOR as u32, true),
            dir_record(b"IDWBINFO.TXT", 41, idwbinfo.len() as u32, false),
            first,
            second,
        ];
        write_directory_records(&mut b, 39, &records);
    }

    let mut pe = [0u8; 512];
    pe[0..2].copy_from_slice(b"MZ");
    pe[0x3c..0x40].copy_from_slice(&0x80u32.to_le_bytes());
    pe[0x80..0x84].copy_from_slice(b"PE\0\0");
    pe[0x84..0x86].copy_from_slice(&machine.to_le_bytes());
    b.data[30 * SECTOR..30 * SECTOR + pe.len()].copy_from_slice(&pe);
    b.data[31 * SECTOR..31 * SECTOR + pe.len()].copy_from_slice(&pe);
    b.data[40 * SECTOR..40 * SECTOR + 64].fill(0x5A);
    b.data[43 * SECTOR..44 * SECTOR].fill(0x43);
    b.data[44 * SECTOR..45 * SECTOR].fill(0x44);
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
