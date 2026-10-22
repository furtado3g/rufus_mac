//! Analysis tests against a synthetic ISO 9660 image, so the suite needs no
//! network access and no multi-hundred-megabyte fixture.

use std::fs::File;

use image2usb_iso::{
    Distro, WindowsArchitecture, analyze_path, read_volume_descriptors, read_walk_entry, walk_tree,
};
use image2usb_testiso::{
    WindowsFixture, build_image, build_image_with_large_file, build_windows_image,
};

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
fn rejects_udf_only_images_with_a_clear_error() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("udf.iso");
    let mut data = vec![0u8; 128 * 2048];
    data[16 * 2048 + 1..16 * 2048 + 6].copy_from_slice(b"BEA01");
    data[17 * 2048 + 1..17 * 2048 + 6].copy_from_slice(b"NSR02");
    data[18 * 2048 + 1..18 * 2048 + 6].copy_from_slice(b"TEA01");
    std::fs::write(&path, data).unwrap();

    let err = analyze_path(&path).unwrap_err().to_string();
    assert!(err.contains("UDF-only"), "{err}");
}

#[test]
fn detects_windows_x64_media_without_textual_marker() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("win11.iso");
    build_windows_image(&path, WindowsFixture::X64Wim);

    let info = analyze_path(&path).unwrap();
    assert_eq!(info.distro, Distro::Windows);
    let windows = info.windows.unwrap();
    assert_eq!(windows.architecture, WindowsArchitecture::X64);
    assert_eq!(windows.pe_architecture, WindowsArchitecture::X64);
    assert_eq!(windows.install_format.as_deref(), Some("wim"));
    assert_eq!(windows.install_wim_size, Some(4096));
    assert_eq!(windows.install_parts.len(), 1);
    assert!(windows.bootmgr);
    assert!(windows.bcd);
    assert_eq!(
        windows.efi_fallback.as_deref(),
        Some("efi/boot/bootx64.efi")
    );
    assert_eq!(windows.build.as_deref(), Some("19041"));
    assert_eq!(windows.edition.as_deref(), Some("Professional"));
}

#[test]
fn detects_arm64_from_pe_machine() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("win11-arm.iso");
    build_windows_image(&path, WindowsFixture::Arm64Wim);

    let info = analyze_path(&path).unwrap();
    let windows = info.windows.unwrap();
    assert_eq!(windows.architecture, WindowsArchitecture::Arm64);
    assert_eq!(windows.pe_architecture, WindowsArchitecture::Arm64);
    assert_eq!(
        windows.efi_fallback.as_deref(),
        Some("efi/boot/bootaa64.efi")
    );
    assert_eq!(windows.idwbinfo_architecture, WindowsArchitecture::Arm64);
}

#[test]
fn windows_candidate_with_missing_boot_files_is_reported_but_not_bootable() {
    let dir = tempfile::tempdir().unwrap();
    for (name, fixture) in [
        ("no-bootmgr.iso", WindowsFixture::MissingBootmgr),
        ("no-bcd.iso", WindowsFixture::MissingBcd),
    ] {
        let path = dir.path().join(name);
        build_windows_image(&path, fixture);
        let info = analyze_path(&path).unwrap();
        assert_eq!(info.distro, Distro::Windows);
        assert!(info.windows.is_some());
        assert!(!info.warnings.is_empty());
    }
}

#[test]
fn unknown_pe_architecture_is_not_claimed_as_supported() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("unknown.iso");
    build_windows_image(&path, WindowsFixture::UnknownArchitecture);

    let info = analyze_path(&path).unwrap();
    let windows = info.windows.unwrap();
    assert_eq!(windows.pe_architecture, WindowsArchitecture::Unknown);
    assert_eq!(windows.architecture, WindowsArchitecture::X64);
    assert!(
        info.warnings
            .iter()
            .any(|warning| warning.contains("PE machine"))
    );
}

#[test]
fn detects_esd_and_swm_formats() {
    let dir = tempfile::tempdir().unwrap();
    let esd = dir.path().join("esd.iso");
    build_windows_image(&esd, WindowsFixture::Esd);
    let info = analyze_path(&esd).unwrap();
    let windows = info.windows.unwrap();
    assert_eq!(windows.install_format.as_deref(), Some("esd"));
    assert_eq!(windows.install_wim_size, None);

    let swm = dir.path().join("swm.iso");
    build_windows_image(&swm, WindowsFixture::Swm);
    let info = analyze_path(&swm).unwrap();
    let windows = info.windows.unwrap();
    assert_eq!(windows.install_format.as_deref(), Some("swm"));
    assert_eq!(windows.install_parts.len(), 2);
    assert_eq!(windows.install_parts[0].name, "sources/install.swm");
    assert_eq!(windows.install_parts[1].name, "sources/install2.swm");
}

#[test]
fn joins_iso_multi_extent_files_and_reports_total_size() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("multi.iso");
    build_windows_image(&path, WindowsFixture::MultiExtentWim);

    let size = std::fs::metadata(&path).unwrap().len();
    let mut file = File::open(&path).unwrap();
    let descriptors = read_volume_descriptors(&mut file, size).unwrap();
    let root = descriptors
        .joliet
        .clone()
        .or_else(|| descriptors.primary.clone().map(|volume| volume.root))
        .unwrap();
    let entries = walk_tree(&mut file, &root, descriptors.has_joliet, 100, size).unwrap();
    let install = entries
        .iter()
        .find(|entry| entry.path.eq_ignore_ascii_case("SOURCES/INSTALL.WIM"))
        .unwrap();
    assert_eq!(install.len, 4096);
    assert_eq!(install.source_extents().len(), 2);

    let bytes = read_walk_entry(&mut file, install, 8192).unwrap();
    assert_eq!(bytes.len(), 4096);
    assert!(bytes[..2048].iter().all(|byte| *byte == 0x43));
    assert!(bytes[2048..].iter().all(|byte| *byte == 0x44));

    let info = analyze_path(&path).unwrap();
    let windows = info.windows.unwrap();
    assert_eq!(windows.install_wim_size, Some(4096));
}

#[test]
fn catalog_uses_u64_sizes_and_rejects_extents_outside_the_image() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("large.iso");
    build_image_with_large_file(&path, 200 * 1024 * 1024);
    let info = analyze_path(&path).unwrap();
    assert_eq!(info.distro, Distro::Ubuntu);

    let size = std::fs::metadata(&path).unwrap().len();
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&path)
        .unwrap();
    let descriptors = read_volume_descriptors(&mut file, size).unwrap();
    let root = descriptors
        .joliet
        .clone()
        .or_else(|| descriptors.primary.clone().map(|volume| volume.root))
        .unwrap();
    let entries = walk_tree(&mut file, &root, descriptors.has_joliet, 100, size).unwrap();
    let huge = entries
        .iter()
        .find(|entry| entry.path == "HUGE.BIN")
        .unwrap();
    assert_eq!(huge.len, 200 * 1024 * 1024u64);
    let limited = walk_tree(&mut file, &root, descriptors.has_joliet, 1, size).unwrap_err();
    assert!(limited.to_string().contains("more than 1"), "{limited}");

    file.set_len(0).unwrap();
    let error = walk_tree(&mut file, &root, descriptors.has_joliet, 100, size).unwrap_err();
    assert!(
        matches!(error, image2usb_iso::IsoError::Truncated),
        "{error}"
    );
}

#[test]
fn rejects_non_iso_files() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("not.iso");
    std::fs::write(&path, vec![0u8; 200 * 1024]).unwrap();
    assert!(analyze_path(&path).is_err());
}
