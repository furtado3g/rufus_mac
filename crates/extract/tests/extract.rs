//! End-to-end extraction against the synthetic ISO fixture.

use image2usb_core::{CancelToken, FileDevice, NoopSink, RawIo, VerifyMode, WriteOptions};
use image2usb_extract::{extract_image, list_extracted, plan_layout, preview, read_extracted};
use image2usb_iso::analyze_path;
use image2usb_testiso::{DISK_INFO, EFI_BOOT, ISOLINUX_CFG, build_image};

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
