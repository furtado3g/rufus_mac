//! End-to-end extraction against the synthetic ISO fixture.

use image2usb_core::{
    CancelToken, Error, FileDevice, FileSystemChoice, NoopSink, PartitionScheme, RawIo, VerifyMode,
    WriteOptions,
};
use image2usb_extract::{
    extract_image, extract_windows_fat32, list_extracted, plan_gpt_layout, plan_layout,
    prepare_windows, preview, read_extracted,
};
use image2usb_iso::{WalkEntry, analyze_path};
use image2usb_testiso::{
    DISK_INFO, EFI_BOOT, GRUB_CFG, ISOLINUX_CFG, WindowsFixture, build_image,
    build_image_with_grub, build_image_with_large_file, build_image_with_symlink,
    build_windows_image,
};

#[test]
fn extracts_uefi_tree_into_mbr_fat32() {
    let dir = tempfile::tempdir().unwrap();
    let iso = dir.path().join("test.iso");
    build_image(&iso, 0xEF, true);
    let info = analyze_path(&iso).unwrap();

    let disk = dir.path().join("disk.img");
    std::fs::write(&disk, vec![0u8; 96 * 1024 * 1024]).unwrap();
    let mut device = FileDevice::open(&disk).unwrap();
    let layout = plan_layout(device.size(), device.logical_sector_size()).unwrap();

    assert_eq!(preview(&iso).unwrap(), (3, 5, 30));

    let options = WriteOptions {
        verify: VerifyMode::Sha256,
        label: String::new(),
        ..WriteOptions::default()
    };
    let stats = extract_image(
        &iso,
        &info,
        &mut device,
        &options,
        &CancelToken::new(),
        &NoopSink,
    )
    .unwrap();
    assert_eq!(stats.files, 3);
    assert_eq!(stats.directories, 5);
    assert_eq!(stats.bytes, 30);
    assert!(
        stats
            .warnings
            .iter()
            .any(|warning| warning.contains("legacy BIOS")),
        "{:?}",
        stats.warnings
    );
    assert!(
        !stats
            .warnings
            .iter()
            .any(|warning| warning.contains("symlink")),
        "{:?}",
        stats.warnings
    );

    let entries = list_extracted(&mut device, &layout).unwrap();
    let names: Vec<&str> = entries.iter().map(|(path, _)| path.as_str()).collect();
    assert!(names.contains(&"EFI/BOOT/BOOTX64.EFI"), "{names:?}");
    assert!(names.contains(&"ISOLINUX/ISOLINUX.CFG"), "{names:?}");
    assert!(names.contains(&".DISK/INFO"), "{names:?}");
    assert!(names.contains(&"CASPER"), "{names:?}");

    assert_eq!(
        read_extracted(&mut device, &layout, "EFI/BOOT/BOOTX64.EFI").unwrap(),
        EFI_BOOT
    );
    assert_eq!(
        read_extracted(&mut device, &layout, "ISOLINUX/ISOLINUX.CFG").unwrap(),
        ISOLINUX_CFG
    );
    assert_eq!(
        read_extracted(&mut device, &layout, ".DISK/INFO").unwrap(),
        DISK_INFO
    );

    // MBR: one FAT32 LBA partition starting at 1 MiB.
    let mut mbr = [0u8; 512];
    device.read_exact_at(0, &mut mbr).unwrap();
    assert_eq!(&mbr[510..], &[0x55, 0xAA]);
    assert_eq!(mbr[446], 0);
    assert_eq!(u64::from_le_bytes(mbr[0x5c..0x64].try_into().unwrap()), 0);
    assert_eq!(mbr[446 + 4], 0x0C);
    assert_eq!(
        u32::from_le_bytes(mbr[446 + 8..446 + 12].try_into().unwrap()),
        layout.start_lba
    );
    assert_eq!(
        u32::from_le_bytes(mbr[446 + 12..446 + 16].try_into().unwrap()),
        layout.sectors
    );

    // FAT32 boot sector and FSInfo free count.
    let mut bpb = [0u8; 512];
    device.read_exact_at(layout.start_byte, &mut bpb).unwrap();
    assert_eq!(&bpb[82..90], b"FAT32   ");
    assert_eq!(&bpb[510..], &[0x55, 0xAA]);
    let mut fsinfo = [0u8; 512];
    device
        .read_exact_at(layout.start_byte + 512, &mut fsinfo)
        .unwrap();
    let free = u32::from_le_bytes(fsinfo[488..492].try_into().unwrap());
    assert_ne!(free, u32::MAX);
    assert!(free > 0);
}

#[test]
fn installs_bios_boot_for_grub_mbr_image() {
    let dir = tempfile::tempdir().unwrap();
    let iso = dir.path().join("test.iso");
    build_image_with_grub(&iso, 0xEF, true);
    let info = analyze_path(&iso).unwrap();

    let disk = dir.path().join("disk.img");
    std::fs::write(&disk, vec![0u8; 96 * 1024 * 1024]).unwrap();
    let mut device = FileDevice::open(&disk).unwrap();
    let layout = plan_layout(device.size(), device.logical_sector_size()).unwrap();
    let stats = extract_image(
        &iso,
        &info,
        &mut device,
        &WriteOptions::default(),
        &CancelToken::new(),
        &NoopSink,
    )
    .unwrap();
    assert!(
        !stats
            .warnings
            .iter()
            .any(|warning| warning.contains("will not be installed")),
        "{:?}",
        stats.warnings
    );
    assert_eq!(
        read_extracted(&mut device, &layout, "BOOT/GRUB/GRUB.CFG").unwrap(),
        GRUB_CFG
    );

    let mut mbr = [0u8; 512];
    device.read_exact_at(0, &mut mbr).unwrap();
    assert_eq!(&mbr[0..3], &[0xeb, 0x63, 0x90]);
    assert_eq!(u64::from_le_bytes(mbr[0x5c..0x64].try_into().unwrap()), 34);
    assert_eq!(mbr[0x64], 0xff);
    assert_eq!(mbr[446], 0x80);
    assert_eq!(mbr[446 + 4], 0x0c);
    assert_eq!(
        u32::from_le_bytes(mbr[446 + 8..446 + 12].try_into().unwrap()),
        layout.start_lba
    );
    assert_eq!(&mbr[510..512], &[0x55, 0xaa]);

    let mut core = [0u8; 512];
    device.read_exact_at(34 * 512, &mut core).unwrap();
    assert_eq!(&core[0..2], &[0x52, 0x56]);
    let start = u64::from_le_bytes(core[0x1f4..0x1fc].try_into().unwrap());
    let len = u16::from_le_bytes(core[0x1fc..0x1fe].try_into().unwrap());
    let segment = u16::from_le_bytes(core[0x1fe..0x200].try_into().unwrap());
    assert_eq!(start, 35);
    assert_eq!(
        u64::from(len),
        (bios_core_len("core-mbr.img") - 512).div_ceil(512)
    );
    assert_eq!(segment, 0x820);
}

#[test]
fn installs_bios_boot_for_grub_gpt_image() {
    let dir = tempfile::tempdir().unwrap();
    let iso = dir.path().join("test.iso");
    build_image_with_grub(&iso, 0xEF, true);
    let info = analyze_path(&iso).unwrap();

    let disk = dir.path().join("disk.img");
    std::fs::write(&disk, vec![0u8; 96 * 1024 * 1024]).unwrap();
    let mut device = FileDevice::open(&disk).unwrap();
    let gpt = plan_gpt_layout(device.size(), device.logical_sector_size()).unwrap();
    let options = WriteOptions {
        scheme: PartitionScheme::Gpt,
        ..WriteOptions::default()
    };
    let stats = extract_image(
        &iso,
        &info,
        &mut device,
        &options,
        &CancelToken::new(),
        &NoopSink,
    )
    .unwrap();
    assert!(
        !stats
            .warnings
            .iter()
            .any(|warning| warning.contains("will not be installed")),
        "{:?}",
        stats.warnings
    );
    assert_eq!(
        read_extracted(&mut device, &gpt.partition, "BOOT/GRUB/GRUB.CFG").unwrap(),
        GRUB_CFG
    );

    let mut mbr = [0u8; 512];
    device.read_exact_at(0, &mut mbr).unwrap();
    assert_eq!(mbr[446 + 4], 0xee);
    assert_eq!(mbr[446], 0);
    assert_eq!(u64::from_le_bytes(mbr[0x5c..0x64].try_into().unwrap()), 34);

    let mut header = [0u8; 92];
    device.read_exact_at(512, &mut header).unwrap();
    assert_eq!(&header[0..8], b"EFI PART");

    let mut entry = [0u8; 16];
    device.read_exact_at(1024, &mut entry).unwrap();
    assert_eq!(
        &entry,
        &[
            0x28, 0x73, 0x2A, 0xC1, 0x1F, 0xF8, 0xD2, 0x11, 0xBA, 0x4B, 0x00, 0xA0, 0xC9, 0x3E,
            0xC9, 0x3B
        ]
    );

    let mut core = [0u8; 512];
    device.read_exact_at(34 * 512, &mut core).unwrap();
    assert_eq!(&core[0..2], &[0x52, 0x56]);
    let start = u64::from_le_bytes(core[0x1f4..0x1fc].try_into().unwrap());
    let len = u16::from_le_bytes(core[0x1fc..0x1fe].try_into().unwrap());
    let segment = u16::from_le_bytes(core[0x1fe..0x200].try_into().unwrap());
    assert_eq!(start, 35);
    assert_eq!(
        u64::from(len),
        (bios_core_len("core-gpt.img") - 512).div_ceil(512)
    );
    assert_eq!(segment, 0x820);
}

fn bios_core_len(name: &str) -> u64 {
    std::fs::read(format!(
        "{}/../../assets/grub-bios/{name}",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap()
    .len() as u64
}

#[test]
fn skips_rock_ridge_symlinks_with_a_warning() {
    let dir = tempfile::tempdir().unwrap();
    let iso = dir.path().join("test.iso");
    build_image_with_symlink(&iso, 0xEF, true);
    let info = analyze_path(&iso).unwrap();

    let disk = dir.path().join("disk.img");
    std::fs::write(&disk, vec![0u8; 96 * 1024 * 1024]).unwrap();
    let mut device = FileDevice::open(&disk).unwrap();
    let layout = plan_layout(device.size(), device.logical_sector_size()).unwrap();

    // The symlink is not a regular file and is not counted by the preview.
    assert_eq!(preview(&iso).unwrap(), (3, 5, 30));

    let stats = extract_image(
        &iso,
        &info,
        &mut device,
        &WriteOptions::default(),
        &CancelToken::new(),
        &NoopSink,
    )
    .unwrap();
    assert_eq!(stats.files, 3);
    assert_eq!(stats.directories, 5);
    assert_eq!(stats.bytes, 30);
    assert!(
        stats
            .warnings
            .iter()
            .any(|warning| warning.contains("symlink") && warning.contains("BOOTLINK")),
        "{:?}",
        stats.warnings
    );

    let entries = list_extracted(&mut device, &layout).unwrap();
    let names: Vec<&str> = entries.iter().map(|(path, _)| path.as_str()).collect();
    assert!(!names.contains(&"BOOTLINK"), "{names:?}");
}

#[test]
fn rejects_too_large_tree_before_mbr_write() {
    let dir = tempfile::tempdir().unwrap();
    let iso = dir.path().join("test.iso");
    build_image_with_large_file(&iso, 256 * 1024 * 1024);
    let info = analyze_path(&iso).unwrap();

    let disk = dir.path().join("disk.img");
    std::fs::write(&disk, vec![0u8; 96 * 1024 * 1024]).unwrap();
    let mut device = FileDevice::open(&disk).unwrap();

    let err = extract_image(
        &iso,
        &info,
        &mut device,
        &WriteOptions::default(),
        &CancelToken::new(),
        &NoopSink,
    )
    .unwrap_err();
    assert!(matches!(err, Error::ExtractionTooLarge { .. }), "{err}");

    let mut mbr = [0u8; 512];
    device.read_exact_at(0, &mut mbr).unwrap();
    assert_eq!(mbr, [0u8; 512]);
}

#[test]
fn extracts_gpt_esp_with_uefi_tree() {
    let dir = tempfile::tempdir().unwrap();
    let iso = dir.path().join("test.iso");
    build_image(&iso, 0xEF, true);
    let info = analyze_path(&iso).unwrap();

    let disk = dir.path().join("disk.img");
    std::fs::write(&disk, vec![0u8; 96 * 1024 * 1024]).unwrap();
    let mut device = FileDevice::open(&disk).unwrap();
    let gpt = plan_gpt_layout(device.size(), device.logical_sector_size()).unwrap();
    let options = WriteOptions {
        scheme: PartitionScheme::Gpt,
        ..WriteOptions::default()
    };

    let stats = extract_image(
        &iso,
        &info,
        &mut device,
        &options,
        &CancelToken::new(),
        &NoopSink,
    )
    .unwrap();
    assert_eq!(stats.files, 3);
    assert_eq!(
        read_extracted(&mut device, &gpt.partition, "EFI/BOOT/BOOTX64.EFI").unwrap(),
        EFI_BOOT
    );

    let mut mbr = [0u8; 512];
    device.read_exact_at(0, &mut mbr).unwrap();
    assert_eq!(&mbr[510..512], &[0x55, 0xAA]);
    assert_eq!(mbr[446 + 4], 0xEE);

    let mut header = [0u8; 92];
    device.read_exact_at(512, &mut header).unwrap();
    assert_eq!(&header[0..8], b"EFI PART");
    assert_eq!(u64::from_le_bytes(header[24..32].try_into().unwrap()), 1);

    let mut entry = [0u8; 128];
    device.read_exact_at(1024, &mut entry).unwrap();
    assert_eq!(
        &entry[0..16],
        &[
            0x28, 0x73, 0x2A, 0xC1, 0x1F, 0xF8, 0xD2, 0x11, 0xBA, 0x4B, 0x00, 0xA0, 0xC9, 0x3E,
            0xC9, 0x3B
        ]
    );

    match std::process::Command::new("sgdisk")
        .arg("--verify")
        .arg(&disk)
        .output()
    {
        Ok(output) => assert!(
            output.status.success(),
            "sgdisk --verify failed\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => panic!("failed to run sgdisk: {err}"),
    }
}

#[test]
fn cancelled_extraction_does_not_touch_the_device() {
    let dir = tempfile::tempdir().unwrap();
    let iso = dir.path().join("test.iso");
    build_image(&iso, 0xEF, true);
    let info = analyze_path(&iso).unwrap();

    let disk = dir.path().join("disk.img");
    std::fs::write(&disk, vec![0u8; 96 * 1024 * 1024]).unwrap();
    let mut device = FileDevice::open(&disk).unwrap();
    let cancel = CancelToken::new();
    cancel.cancel();

    let err = extract_image(
        &iso,
        &info,
        &mut device,
        &WriteOptions::default(),
        &cancel,
        &NoopSink,
    )
    .unwrap_err();
    assert!(matches!(err, Error::Cancelled), "{err}");

    let mut mbr = [0u8; 512];
    device.read_exact_at(0, &mut mbr).unwrap();
    assert_eq!(mbr, [0u8; 512]);
}

#[test]
fn extracted_fat32_passes_platform_fsck() {
    assert_platform_fsck(PartitionScheme::Mbr);
}

#[test]
fn extracted_gpt_fat32_passes_platform_fsck() {
    assert_platform_fsck(PartitionScheme::Gpt);
}

fn assert_platform_fsck(scheme: PartitionScheme) {
    let dir = tempfile::tempdir().unwrap();
    let iso = dir.path().join("test.iso");
    build_image(&iso, 0xEF, true);
    let info = analyze_path(&iso).unwrap();

    let disk = dir.path().join("disk.img");
    std::fs::write(&disk, vec![0u8; 96 * 1024 * 1024]).unwrap();
    let mut device = FileDevice::open(&disk).unwrap();
    let partition_layout = match scheme {
        PartitionScheme::Mbr => plan_layout(device.size(), device.logical_sector_size()).unwrap(),
        PartitionScheme::Gpt => {
            plan_gpt_layout(device.size(), device.logical_sector_size())
                .unwrap()
                .partition
        }
    };
    let options = WriteOptions {
        scheme,
        ..WriteOptions::default()
    };
    extract_image(
        &iso,
        &info,
        &mut device,
        &options,
        &CancelToken::new(),
        &NoopSink,
    )
    .unwrap();

    let fsck = if cfg!(target_os = "macos") {
        "fsck_msdos"
    } else if cfg!(target_os = "linux") {
        "fsck.vfat"
    } else {
        return;
    };

    // fsck wants the partition, not the whole disk with its partition table.
    let image = std::fs::read(&disk).unwrap();
    let partition =
        &image[partition_layout.start_byte as usize..partition_layout.end_byte() as usize];
    let part_path = dir.path().join("partition.img");
    std::fs::write(&part_path, partition).unwrap();

    let output = match std::process::Command::new(fsck)
        .arg("-n")
        .arg(&part_path)
        .output()
    {
        Ok(output) => output,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return,
        Err(err) => panic!("failed to run {fsck}: {err}"),
    };
    assert!(
        output.status.success(),
        "{fsck} failed\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn extracts_with_4k_logical_sectors() {
    let dir = tempfile::tempdir().unwrap();
    let iso = dir.path().join("test.iso");
    build_image_with_grub(&iso, 0xEF, true);
    let info = analyze_path(&iso).unwrap();

    let disk = dir.path().join("disk.img");
    std::fs::write(&disk, vec![0u8; 96 * 1024 * 1024]).unwrap();
    let mut device = SectorDevice {
        inner: FileDevice::open(&disk).unwrap(),
        sector: 4096,
    };
    let layout = plan_layout(device.size(), device.logical_sector_size()).unwrap();
    assert_eq!(layout.start_lba, 256);
    assert_eq!(layout.sector_size, 4096);
    assert_eq!(layout.len % 4096, 0);

    let stats = extract_image(
        &iso,
        &info,
        &mut device,
        &WriteOptions::default(),
        &CancelToken::new(),
        &NoopSink,
    )
    .unwrap();
    assert_eq!(stats.files, 4);
    assert!(
        stats.warnings.iter().any(|warning| warning.contains("4Kn")),
        "{:?}",
        stats.warnings
    );

    let entries = list_extracted(&mut device, &layout).unwrap();
    assert!(
        entries
            .iter()
            .any(|(path, _)| path == "EFI/BOOT/BOOTX64.EFI")
    );
    assert_eq!(
        read_extracted(&mut device, &layout, "EFI/BOOT/BOOTX64.EFI").unwrap(),
        EFI_BOOT
    );

    let mut first_sector = vec![0u8; 4096];
    device.read_exact_at(0, &mut first_sector).unwrap();
    assert_eq!(&first_sector[510..512], &[0x55, 0xAA]);
    assert_eq!(first_sector[446], 0);
    assert_eq!(
        u64::from_le_bytes(first_sector[0x5c..0x64].try_into().unwrap()),
        0
    );
    assert_eq!(first_sector[446 + 4], 0x0C);
    assert_eq!(
        u32::from_le_bytes(first_sector[446 + 8..446 + 12].try_into().unwrap()),
        256
    );
    assert!(first_sector[512..].iter().all(|byte| *byte == 0));
}

struct SectorDevice {
    inner: FileDevice,
    sector: u32,
}

impl RawIo for SectorDevice {
    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> image2usb_core::Result<usize> {
        self.inner.read_at(offset, buf)
    }

    fn write_at(&mut self, offset: u64, buf: &[u8]) -> image2usb_core::Result<usize> {
        self.inner.write_at(offset, buf)
    }

    fn size(&self) -> u64 {
        self.inner.size()
    }

    fn logical_sector_size(&self) -> u32 {
        self.sector
    }

    fn physical_sector_size(&self) -> u32 {
        self.sector
    }

    fn sync(&mut self) -> image2usb_core::Result<()> {
        self.inner.sync()
    }

    fn invalidate(&mut self) -> image2usb_core::Result<()> {
        self.inner.invalidate()
    }
}

#[test]
fn extracts_gpt_with_4k_logical_sectors() {
    let dir = tempfile::tempdir().unwrap();
    let iso = dir.path().join("test.iso");
    build_image(&iso, 0xEF, true);
    let info = analyze_path(&iso).unwrap();

    let disk = dir.path().join("disk.img");
    std::fs::write(&disk, vec![0u8; 96 * 1024 * 1024]).unwrap();
    let mut device = SectorDevice {
        inner: FileDevice::open(&disk).unwrap(),
        sector: 4096,
    };
    let gpt = plan_gpt_layout(device.size(), device.logical_sector_size()).unwrap();
    assert_eq!(gpt.partition.start_lba, 256);
    let options = WriteOptions {
        scheme: PartitionScheme::Gpt,
        ..WriteOptions::default()
    };
    extract_image(
        &iso,
        &info,
        &mut device,
        &options,
        &CancelToken::new(),
        &NoopSink,
    )
    .unwrap();
    assert_eq!(
        read_extracted(&mut device, &gpt.partition, "EFI/BOOT/BOOTX64.EFI").unwrap(),
        EFI_BOOT
    );

    let mut header = [0u8; 92];
    device.read_exact_at(4096, &mut header).unwrap();
    assert_eq!(&header[0..8], b"EFI PART");
    let mut entry = [0u8; 16];
    device.read_exact_at(8192, &mut entry).unwrap();
    assert_eq!(&entry, &ESP_TYPE_GUID_FOR_TEST);
    let mut backup = [0u8; 92];
    device
        .read_exact_at(gpt.last_lba * 4096, &mut backup)
        .unwrap();
    assert_eq!(&backup[0..8], b"EFI PART");
}

const ESP_TYPE_GUID_FOR_TEST: [u8; 16] = [
    0x28, 0x73, 0x2A, 0xC1, 0x1F, 0xF8, 0xD2, 0x11, 0xBA, 0x4B, 0x00, 0xA0, 0xC9, 0x3E, 0xC9, 0x3B,
];

#[test]
fn extracts_windows_x64_tree_into_fat32_and_verifies_it() {
    let dir = tempfile::tempdir().unwrap();
    let iso = dir.path().join("win11.iso");
    build_windows_image(&iso, WindowsFixture::X64Wim);
    let source_before = std::fs::read(&iso).unwrap();
    let info = analyze_path(&iso).unwrap();
    assert_eq!(info.distro, image2usb_iso::Distro::Windows);

    let disk = dir.path().join("disk.img");
    std::fs::write(&disk, vec![0u8; 96 * 1024 * 1024]).unwrap();
    let mut device = FileDevice::open(&disk).unwrap();
    let layout = plan_layout(device.size(), device.logical_sector_size()).unwrap();
    let options = WriteOptions {
        verify: VerifyMode::Sha256,
        label: String::new(),
        ..WriteOptions::default()
    };
    let stats = extract_image(
        &iso,
        &info,
        &mut device,
        &options,
        &CancelToken::new(),
        &NoopSink,
    )
    .unwrap();
    assert_eq!(stats.files, 5);
    assert!(stats.bytes > 4096);
    assert!(
        stats
            .warnings
            .iter()
            .any(|warning| warning.contains("UEFI-only")),
        "{:?}",
        stats.warnings
    );
    assert!(
        !stats
            .warnings
            .iter()
            .any(|warning| warning.contains("GRUB")),
        "{:?}",
        stats.warnings
    );

    let entries = list_extracted(&mut device, &layout).unwrap();
    let names: Vec<&str> = entries.iter().map(|(path, _)| path.as_str()).collect();
    for required in [
        "BOOTMGR",
        "BOOT/BCD",
        "EFI/BOOT/BOOTX64.EFI",
        "SOURCES/INSTALL.WIM",
        "SOURCES/IDWBINFO.TXT",
    ] {
        assert!(names.contains(&required), "missing {required}: {names:?}");
    }
    let efi = read_extracted(&mut device, &layout, "EFI/BOOT/BOOTX64.EFI").unwrap();
    assert_eq!(&efi[..2], b"MZ");
    let install = read_extracted(&mut device, &layout, "SOURCES/INSTALL.WIM").unwrap();
    assert_eq!(install.len(), 4096);
    assert_eq!(std::fs::read(&iso).unwrap(), source_before);
    assert!(
        !names
            .iter()
            .any(|name| name.eq_ignore_ascii_case("AUTOUNATTEND.XML")),
        "default extraction must not add Windows customization: {names:?}"
    );

    let mut mbr = [0u8; 512];
    device.read_exact_at(0, &mut mbr).unwrap();
    assert_eq!(mbr[446 + 4], 0x0C);
}

#[test]
fn windows_preparation_rejects_ambiguous_or_linked_paths_before_any_write() {
    for (fixture, expected) in [
        (WindowsFixture::CaseCollision, "ambiguous by case"),
        (WindowsFixture::Symlink, "symlink"),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let iso = dir.path().join("windows.iso");
        build_windows_image(&iso, fixture);
        let info = analyze_path(&iso).unwrap();
        let disk = dir.path().join("disk.img");
        std::fs::write(&disk, vec![0u8; 96 * 1024 * 1024]).unwrap();
        let mut device = FileDevice::open(&disk).unwrap();
        let error = extract_image(
            &iso,
            &info,
            &mut device,
            &WriteOptions::default(),
            &CancelToken::new(),
            &NoopSink,
        )
        .unwrap_err();
        assert!(error.to_string().contains(expected), "{fixture:?}: {error}");
        let mut first = [0u8; 512];
        device.read_exact_at(0, &mut first).unwrap();
        assert_eq!(first, [0u8; 512], "{fixture:?}");
    }
}

#[test]
fn source_changes_after_snapshot_do_not_affect_the_committed_tree() {
    let dir = tempfile::tempdir().unwrap();
    let iso = dir.path().join("windows.iso");
    build_windows_image(&iso, WindowsFixture::X64Wim);
    let prepared = prepare_windows(
        &iso,
        &WriteOptions::default(),
        &CancelToken::new(),
        &NoopSink,
    )
    .unwrap();

    std::fs::write(&iso, b"source replaced after the snapshot").unwrap();

    let disk = dir.path().join("disk.img");
    std::fs::write(&disk, vec![0u8; 96 * 1024 * 1024]).unwrap();
    let mut device = FileDevice::open(&disk).unwrap();
    let layout = plan_layout(device.size(), device.logical_sector_size()).unwrap();
    let stats = extract_windows_fat32(
        &prepared,
        &mut device,
        &WriteOptions {
            verify: VerifyMode::Sha256,
            ..WriteOptions::default()
        },
        &CancelToken::new(),
        &NoopSink,
    )
    .unwrap();
    assert_eq!(stats.files, 5);
    let install = read_extracted(&mut device, &layout, "SOURCES/INSTALL.WIM").unwrap();
    assert_eq!(install.len(), 4096);
    assert!(install[..2048].iter().all(|byte| *byte == 0x43));
    assert!(install[2048..].iter().all(|byte| *byte == 0x44));
}

#[test]
fn windows_preparation_rejects_incomplete_boot_chain_before_any_write() {
    for fixture in [
        WindowsFixture::MissingBootmgr,
        WindowsFixture::MissingBcd,
        WindowsFixture::UnknownArchitecture,
    ] {
        let dir = tempfile::tempdir().unwrap();
        let iso = dir.path().join("windows.iso");
        build_windows_image(&iso, fixture);
        let info = analyze_path(&iso).unwrap();
        let disk = dir.path().join("disk.img");
        std::fs::write(&disk, vec![0u8; 96 * 1024 * 1024]).unwrap();
        let mut device = FileDevice::open(&disk).unwrap();
        let error = extract_image(
            &iso,
            &info,
            &mut device,
            &WriteOptions::default(),
            &CancelToken::new(),
            &NoopSink,
        )
        .unwrap_err();
        let message = error.to_string().to_ascii_lowercase();
        assert!(
            matches!(fixture, WindowsFixture::UnknownArchitecture)
                && message.contains("pe machine")
                || message.contains("bootmgr")
                || message.contains("bcd"),
            "{fixture:?}: {error}"
        );
        let mut first = [0u8; 512];
        device.read_exact_at(0, &mut first).unwrap();
        assert_eq!(first, [0u8; 512], "{fixture:?}");
    }
}

#[test]
fn windows_extraction_fails_before_writing_when_target_is_too_small() {
    let dir = tempfile::tempdir().unwrap();
    let iso = dir.path().join("windows.iso");
    build_windows_image(&iso, WindowsFixture::X64Wim);
    let info = analyze_path(&iso).unwrap();
    let disk = dir.path().join("disk.img");
    std::fs::write(&disk, vec![0u8; 32 * 1024 * 1024]).unwrap();
    let mut device = FileDevice::open(&disk).unwrap();
    let error = extract_image(
        &iso,
        &info,
        &mut device,
        &WriteOptions::default(),
        &CancelToken::new(),
        &NoopSink,
    )
    .unwrap_err();
    assert!(error.to_string().contains("at least"), "{error}");
    let mut first = [0u8; 512];
    device.read_exact_at(0, &mut first).unwrap();
    assert_eq!(first, [0u8; 512]);
}

#[test]
fn rejects_windows_media_before_touching_the_device() {
    // The fixture is not Windows media, so this only checks that a non-FAT32
    // filesystem is refused before any write happens.
    let dir = tempfile::tempdir().unwrap();
    let iso = dir.path().join("test.iso");
    build_image(&iso, 0xEF, true);
    let info = analyze_path(&iso).unwrap();

    let disk = dir.path().join("disk.img");
    std::fs::write(&disk, vec![0u8; 96 * 1024 * 1024]).unwrap();
    let mut device = FileDevice::open(&disk).unwrap();
    let options = WriteOptions {
        filesystem: FileSystemChoice::Ntfs,
        ..WriteOptions::default()
    };
    let err = extract_image(
        &iso,
        &info,
        &mut device,
        &options,
        &CancelToken::new(),
        &NoopSink,
    )
    .unwrap_err();
    assert!(err.to_string().contains("FAT32"), "{err}");

    let mut mbr = [0u8; 512];
    device.read_exact_at(0, &mut mbr).unwrap();
    assert_eq!(mbr, [0u8; 512]);
}

#[test]
fn windows_customizations_regressions_and_answer_generation() {
    let dir = tempfile::tempdir().unwrap();
    let iso = dir.path().join("windows.iso");
    build_windows_image(&iso, WindowsFixture::X64Wim);
    let mut info = analyze_path(&iso).unwrap();

    // 1. Unknown / unvalidated build rejection before writing
    let unvalidated_options = WriteOptions {
        windows: image2usb_core::WindowsOptions {
            bypass_tpm: true,
            bypass_secure_boot: false,
            offline_account: false,
        },
        ..WriteOptions::default()
    };
    let err =
        prepare_windows(&iso, &unvalidated_options, &CancelToken::new(), &NoopSink).unwrap_err();
    assert!(err.to_string().contains("not validated"), "{err}");

    // 2. Validate capabilities for validated build 26100 / 26300
    info.windows.as_mut().unwrap().build = Some("26300".to_owned());
    info.windows.as_mut().unwrap().capabilities =
        image2usb_iso::WindowsCapabilities::for_build(Some("26300"));

    // Default flags are all false
    let defaults = image2usb_core::WindowsOptions::default();
    assert!(!defaults.bypass_tpm);
    assert!(!defaults.bypass_secure_boot);
    assert!(!defaults.offline_account);

    // Offline account rejected on unproven build 26300
    let offline_opts = image2usb_core::WindowsOptions {
        offline_account: true,
        ..defaults
    };
    let catalog = vec![];
    let err = image2usb_extract::generate_autounattend_xml(
        info.windows.as_ref().unwrap(),
        &offline_opts,
        &catalog,
    )
    .unwrap_err();
    assert!(
        err.to_string()
            .contains("offline account flow is not validated"),
        "{err}"
    );

    // Conflicting answer file in source image rejected
    let conflict_catalog = vec![WalkEntry {
        path: "autounattend.xml".to_owned(),
        extent: 0,
        len: 10,
        extents: vec![],
        is_dir: false,
        symlink_target: None,
    }];
    let tpm_opts = image2usb_core::WindowsOptions {
        bypass_tpm: true,
        ..defaults
    };
    let err = image2usb_extract::generate_autounattend_xml(
        info.windows.as_ref().unwrap(),
        &tpm_opts,
        &conflict_catalog,
    )
    .unwrap_err();
    assert!(
        err.to_string().contains("already contains an answer file"),
        "{err}"
    );

    // Valid XML generation with TPM and Secure Boot bypass
    let both_opts = image2usb_core::WindowsOptions {
        bypass_tpm: true,
        bypass_secure_boot: true,
        offline_account: false,
    };
    let xml_bytes = image2usb_extract::generate_autounattend_xml(
        info.windows.as_ref().unwrap(),
        &both_opts,
        &catalog,
    )
    .unwrap();
    let xml_text = String::from_utf8(xml_bytes).unwrap();
    assert!(xml_text.starts_with("<?xml version=\"1.0\" encoding=\"utf-8\"?>"));
    assert!(xml_text.contains("BypassTPMCheck"));
    assert!(xml_text.contains("BypassSecureBootCheck"));
    // Ensures no automatic admin account without password or automated disk selection is introduced
    assert!(!xml_text.contains("<AdministratorPassword>"));
    assert!(!xml_text.contains("<InstallTo>"));
    assert!(!xml_text.contains("<DiskID>"));

    // Isolated TPM only
    let tpm_xml = String::from_utf8(
        image2usb_extract::generate_autounattend_xml(
            info.windows.as_ref().unwrap(),
            &tpm_opts,
            &catalog,
        )
        .unwrap(),
    )
    .unwrap();
    assert!(tpm_xml.contains("BypassTPMCheck"));
    assert!(!tpm_xml.contains("BypassSecureBootCheck"));

    // Isolated Secure Boot only
    let sb_opts = image2usb_core::WindowsOptions {
        bypass_secure_boot: true,
        ..defaults
    };
    let sb_xml = String::from_utf8(
        image2usb_extract::generate_autounattend_xml(
            info.windows.as_ref().unwrap(),
            &sb_opts,
            &catalog,
        )
        .unwrap(),
    )
    .unwrap();
    assert!(!sb_xml.contains("BypassTPMCheck"));
    assert!(sb_xml.contains("BypassSecureBootCheck"));
}
