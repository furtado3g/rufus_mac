//! End-to-end extraction against the synthetic ISO fixture.

use image2usb_core::{
    CancelToken, Error, FileDevice, NoopSink, PartitionScheme, RawIo, VerifyMode, WriteOptions,
};
use image2usb_extract::{
    extract_image, list_extracted, plan_gpt_layout, plan_layout, preview, read_extracted,
};
use image2usb_iso::analyze_path;
use image2usb_testiso::{
    DISK_INFO, EFI_BOOT, ISOLINUX_CFG, build_image, build_image_with_large_file,
    build_image_with_symlink,
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
    assert!(stats.warnings.is_empty(), "{:?}", stats.warnings);

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
    let dir = tempfile::tempdir().unwrap();
    let iso = dir.path().join("test.iso");
    build_image(&iso, 0xEF, true);
    let info = analyze_path(&iso).unwrap();

    let disk = dir.path().join("disk.img");
    std::fs::write(&disk, vec![0u8; 96 * 1024 * 1024]).unwrap();
    let mut device = FileDevice::open(&disk).unwrap();
    let layout = plan_layout(device.size(), device.logical_sector_size()).unwrap();
    extract_image(
        &iso,
        &info,
        &mut device,
        &WriteOptions::default(),
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

    // fsck wants the partition, not the whole disk with its MBR.
    let image = std::fs::read(&disk).unwrap();
    let partition = &image[layout.start_byte as usize..layout.end_byte() as usize];
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
    build_image(&iso, 0xEF, true);
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
    assert_eq!(stats.files, 3);

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
        filesystem: image2usb_core::FileSystem::Ntfs,
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
    assert!(err.to_string().contains("FAT32"));

    let mut mbr = [0u8; 512];
    device.read_exact_at(0, &mut mbr).unwrap();
    assert_eq!(mbr, [0u8; 512]);
}
