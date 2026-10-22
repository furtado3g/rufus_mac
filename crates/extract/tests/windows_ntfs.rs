//! End-to-end synthetic tests for the Windows NTFS + UEFI:NTFS path.
//!
//! These tests never touch a host block device. They build a small synthetic
//! Windows-shaped ISO, write a two-partition image to a regular file, then
//! inspect the partition tables, the embedded UEFI:NTFS asset and the NTFS
//! files through the native writer.

#![cfg(all(feature = "windows-media", target_os = "macos"))]

use std::fs::File;

use image2usb_core::{
    CancelToken, FileDevice, FileSystemChoice, NoopSink, PartitionScheme, RawIo, VerifyMode,
    WriteOptions,
};
use image2usb_extract::extract_image;
use image2usb_iso::analyze_path;
use image2usb_platform::media::NtfsVolume;
use image2usb_testiso::{WindowsFixture, build_windows_image};

const TARGET_BYTES: u64 = 96 * 1024 * 1024;
const UEFI_NTFS: &[u8] = include_bytes!("../../../assets/uefi-ntfs/uefi-ntfs.img");

const NTFS_TYPE_GUID: [u8; 16] = [
    0xA2, 0xA0, 0xD0, 0xEB, 0xE5, 0xB9, 0x33, 0x44, 0x87, 0xC0, 0x68, 0xB6, 0xB7, 0x26, 0x99, 0xC7,
];
const ESP_TYPE_GUID: [u8; 16] = [
    0x28, 0x73, 0x2A, 0xC1, 0x1F, 0xF8, 0xD2, 0x11, 0xBA, 0x4B, 0x00, 0xA0, 0xC9, 0x3E, 0xC9, 0x3B,
];

#[test]
fn writes_both_partition_schemes_with_ntfs_and_uefi_ntfs() {
    // The native NTFS layer serializes access through a process-global lock.
    // Run both schemes in one test so a failure in one cannot panic while the
    // other test is holding that lock.
    run_case(PartitionScheme::Mbr);
    run_case(PartitionScheme::Gpt);
}

#[test]
fn embedded_uefi_ntfs_asset_matches_the_pinned_hash() {
    use sha2::{Digest, Sha256};
    assert_eq!(UEFI_NTFS.len(), 1024 * 1024);
    assert_eq!(
        hex::encode(Sha256::digest(UEFI_NTFS)),
        "72683fa1250eeea772d3399277b434d4e55ba8dd0dc926e52d817e701fc2eb9e"
    );
}

fn run_case(scheme: PartitionScheme) {
    let dir = tempfile::tempdir().unwrap();
    let iso = dir.path().join("windows.iso");
    build_windows_image(&iso, WindowsFixture::X64Wim);
    let info = analyze_path(&iso).unwrap();

    let target = dir.path().join("target.img");
    File::create(&target)
        .unwrap()
        .set_len(TARGET_BYTES)
        .unwrap();
    let mut device = FileDevice::open(&target).unwrap();
    let options = WriteOptions {
        scheme,
        filesystem: FileSystemChoice::Ntfs,
        verify: VerifyMode::Sha256,
        ..WriteOptions::default()
    };
    let cancel = CancelToken::new();
    let stats = extract_image(&iso, &info, &mut device, &options, &cancel, &NoopSink).unwrap();
    assert!(stats.files >= 4, "{stats:?}");
    assert!(stats.directories >= 2, "{stats:?}");

    let (data_start, data_len, esp_start) = match scheme {
        PartitionScheme::Mbr => read_mbr_layout(&mut device),
        PartitionScheme::Gpt => read_gpt_layout(&mut device),
    };
    assert_eq!(data_start % (1024 * 1024), 0);
    assert_eq!(esp_start % (1024 * 1024), 0);
    assert!(data_start + data_len <= esp_start);

    let mut asset = vec![0u8; UEFI_NTFS.len()];
    device.read_exact_at(esp_start, &mut asset).unwrap();
    assert_eq!(asset, UEFI_NTFS, "UEFI:NTFS bytes changed");

    let mut part = image2usb_extract::PartitionIo::new(&mut device, data_start, data_len);
    let mut volume = NtfsVolume::mount(&mut part).unwrap();

    let mut boot = [0u8; 512];
    let mut file = volume.open_file("BOOTMGR").unwrap();
    assert_eq!(file.read_at(0, &mut boot).unwrap(), boot.len());
    assert_eq!(&boot[..2], b"MZ");
    file.close().unwrap();

    let mut efi = [0u8; 512];
    let mut file = volume.open_file("EFI/BOOT/BOOTX64.EFI").unwrap();
    assert_eq!(file.read_at(0, &mut efi).unwrap(), efi.len());
    assert_eq!(&efi[..2], b"MZ");
    assert_eq!(
        u16::from_le_bytes(efi[0x84..0x86].try_into().unwrap()),
        0x8664
    );
    file.close().unwrap();

    let mut wim = vec![0u8; 4096];
    let mut file = volume.open_file("SOURCES/INSTALL.WIM").unwrap();
    assert_eq!(file.read_at(0, &mut wim).unwrap(), wim.len());
    assert!(wim[..2048].iter().all(|byte| *byte == 0x43));
    assert!(wim[2048..].iter().all(|byte| *byte == 0x44));
    file.close().unwrap();

    let mut info_file = [0u8; 11];
    let mut file = volume.open_file("SOURCES/IDWBINFO.TXT").unwrap();
    assert_eq!(file.read_at(0, &mut info_file).unwrap(), info_file.len());
    assert_eq!(&info_file, b"[BUILDINFO]");
    file.close().unwrap();

    volume.unmount().unwrap();
    part.flush().unwrap();
    device.sync().unwrap();
}

fn read_mbr_layout(device: &mut FileDevice) -> (u64, u64, u64) {
    let mut mbr = [0u8; 512];
    device.read_exact_at(0, &mut mbr).unwrap();
    assert_eq!(&mbr[510..512], &[0x55, 0xAA]);
    assert_eq!(mbr[446 + 4], 0x07, "first MBR partition is not NTFS");
    assert_eq!(mbr[462 + 4], 0xEF, "second MBR partition is not ESP");
    let data_start = u64::from(u32::from_le_bytes(
        mbr[446 + 8..446 + 12].try_into().unwrap(),
    )) * 512;
    let data_len = u64::from(u32::from_le_bytes(
        mbr[446 + 12..446 + 16].try_into().unwrap(),
    )) * 512;
    let esp_start = u64::from(u32::from_le_bytes(
        mbr[462 + 8..462 + 12].try_into().unwrap(),
    )) * 512;
    (data_start, data_len, esp_start)
}

fn read_gpt_layout(device: &mut FileDevice) -> (u64, u64, u64) {
    let mut header = [0u8; 512];
    device.read_exact_at(512, &mut header).unwrap();
    assert_eq!(&header[0..8], b"EFI PART");
    let mut entries = vec![0u8; 128 * 128];
    device.read_exact_at(1024, &mut entries).unwrap();
    assert_eq!(&entries[0..16], &NTFS_TYPE_GUID);
    assert_eq!(&entries[128..144], &ESP_TYPE_GUID);
    let data_start = u64::from_le_bytes(entries[32..40].try_into().unwrap()) * 512;
    let data_end = (u64::from_le_bytes(entries[40..48].try_into().unwrap()) + 1) * 512;
    let esp_start = u64::from_le_bytes(entries[128 + 32..128 + 40].try_into().unwrap()) * 512;
    let esp_end = (u64::from_le_bytes(entries[128 + 40..128 + 48].try_into().unwrap()) + 1) * 512;
    assert_eq!(esp_end - esp_start, 1024 * 1024);
    (data_start, data_end - data_start, esp_start)
}
