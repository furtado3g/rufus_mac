//! Analysis tests against a synthetic ISO 9660 image, so the suite needs no
//! network access and no multi-hundred-megabyte fixture.

use image2usb_iso::{Distro, analyze_path};
use image2usb_testiso::build_image;

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
fn rejects_non_iso_files() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("not.iso");
    std::fs::write(&path, vec![0u8; 200 * 1024]).unwrap();
    assert!(analyze_path(&path).is_err());
}
