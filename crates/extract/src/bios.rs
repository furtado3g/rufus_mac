//! Legacy BIOS boot support for extraction mode.
//!
//! The boot chain is deliberately small and self-contained:
//!
//! - `boot.img` is written to LBA 0, with the target disk signature and
//!   partition table preserved.
//! - A GRUB core image is written contiguously at LBA 34. That is the first
//!   sector after the GPT partition array and is still well before the
//!   partition at 1 MiB, for both MBR and GPT layouts.
//! - The core's diskboot blocklist is patched to read the rest of the core from
//!   LBA 35 onwards.
//!
//! The core has a small embedded module set and an embedded `early.cfg` that
//! finds the distribution's own `boot/grub/grub.cfg` or
//! `boot/grub2/grub.cfg` on the FAT32 partition.

use image2usb_core::{Error, PartitionScheme, RawIo, Result};
use image2usb_iso::WalkEntry;

use crate::layout::PartitionLayout;

const CORE_LBA: u64 = 34;
const SECTOR_SIZE: u64 = 512;
const KERNEL_SECTOR_OFFSET: usize = 0x5c;
const BOOT_DRIVE_OFFSET: usize = 0x64;
const PRESERVE_FROM: usize = 0x1b8;
const PARTITION_TABLE: usize = 0x1be;
const DISKBOOT_BLOCKLIST: usize = 0x1f4;
const KERNEL_SEGMENT: u16 = 0x820;

const BOOT_IMG: &[u8; 512] = include_bytes!("../../../assets/grub-bios/boot.img");
const CORE_MBR: &[u8] = include_bytes!("../../../assets/grub-bios/core-mbr.img");
const CORE_GPT: &[u8] = include_bytes!("../../../assets/grub-bios/core-gpt.img");

/// Return the distribution's GRUB configuration that the BIOS core can load.
pub fn grub_config(entries: &[WalkEntry]) -> Option<&WalkEntry> {
    entries.iter().find(|entry| {
        if entry.is_dir || entry.is_symlink() {
            return false;
        }
        matches!(
            entry.path.to_ascii_lowercase().as_str(),
            "boot/grub/grub.cfg" | "boot/grub2/grub.cfg"
        )
    })
}

/// Number of logical sectors occupied by the selected core image.
pub fn core_sectors(scheme: PartitionScheme) -> u64 {
    let core = match scheme {
        PartitionScheme::Mbr => CORE_MBR,
        PartitionScheme::Gpt => CORE_GPT,
    };
    (core.len() as u64).div_ceil(SECTOR_SIZE)
}

/// Whether the core fits between the GPT array and the FAT32 partition.
pub fn fits(layout: &PartitionLayout, scheme: PartitionScheme) -> bool {
    layout.sector_size == SECTOR_SIZE as u32
        && u64::from(layout.start_lba) >= CORE_LBA + core_sectors(scheme)
}

/// Write the GRUB boot image and core. The caller must already have a valid
/// partition table; this function only replaces the boot code and the free gap.
pub fn install(
    device: &mut dyn RawIo,
    layout: &PartitionLayout,
    scheme: PartitionScheme,
) -> Result<()> {
    if !fits(layout, scheme) {
        return Err(Error::Unsupported(
            "legacy BIOS boot needs 512-byte logical sectors and a 1 MiB partition gap".to_owned(),
        ));
    }

    let core = core_image(scheme);
    let mut mbr = [0u8; SECTOR_SIZE as usize];
    device.read_exact_at(0, &mut mbr)?;

    // Write the core first. If this fails, the existing partition table is
    // still intact and the extraction can be retried.
    device.write_all_at(CORE_LBA * SECTOR_SIZE, &core)?;

    let boot = boot_sector(&mbr, scheme);
    device.write_all_at(0, &boot)?;
    device.sync()?;
    Ok(())
}

fn boot_sector(mbr: &[u8; 512], scheme: PartitionScheme) -> [u8; 512] {
    let mut boot = *BOOT_IMG;

    // Keep the target disk signature, partition table and 0x55AA signature.
    // The partition table lives at 0x1BE, inside this preserved range.
    boot[PRESERVE_FROM..].copy_from_slice(&mbr[PRESERVE_FROM..]);

    boot[KERNEL_SECTOR_OFFSET..KERNEL_SECTOR_OFFSET + 8].copy_from_slice(&CORE_LBA.to_le_bytes());
    boot[BOOT_DRIVE_OFFSET] = 0xff;

    // GRUB's MBR is written by its setup tool with the partition marked
    // active. GPT uses a protective MBR, which should not be activated.
    if scheme == PartitionScheme::Mbr {
        boot[PARTITION_TABLE] = 0x80;
    }
    boot
}

fn core_image(scheme: PartitionScheme) -> Vec<u8> {
    let source = match scheme {
        PartitionScheme::Mbr => CORE_MBR,
        PartitionScheme::Gpt => CORE_GPT,
    };
    let mut core = source.to_vec();
    assert!(core.len() > 512, "GRUB core must contain diskboot");
    let core_sectors = (core.len() as u64).div_ceil(SECTOR_SIZE) as usize;
    let rest_sectors = (core.len() - 512).div_ceil(512);
    assert!(
        rest_sectors <= u16::MAX as usize,
        "GRUB core is too large for one diskboot blocklist entry"
    );

    core.resize(core_sectors * SECTOR_SIZE as usize, 0);

    // The first blocklist entry is at the end of the diskboot sector. The
    // first core sector was already loaded by boot.img; the blocklist covers
    // the remaining sectors, starting at LBA 35.
    let block = DISKBOOT_BLOCKLIST;
    core[block..block + 8].copy_from_slice(&(CORE_LBA + 1).to_le_bytes());
    core[block + 8..block + 10].copy_from_slice(&(rest_sectors as u16).to_le_bytes());
    core[block + 10..block + 12].copy_from_slice(&KERNEL_SEGMENT.to_le_bytes());
    core
}

#[cfg(test)]
mod tests {
    use super::*;
    use image2usb_core::FileDevice;

    fn read_core_from_repo(scheme: PartitionScheme) -> Vec<u8> {
        let name = match scheme {
            PartitionScheme::Mbr => "core-mbr.img",
            PartitionScheme::Gpt => "core-gpt.img",
        };
        std::fs::read(format!(
            "{}/../../assets/grub-bios/{name}",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap()
    }

    #[test]
    fn boot_sector_preserves_the_partition_table_and_activates_mbr() {
        let mut mbr = [0u8; 512];
        mbr[PRESERVE_FROM..PRESERVE_FROM + 4].copy_from_slice(&0x1234_5678u32.to_le_bytes());
        mbr[PARTITION_TABLE + 4] = 0x0c;
        mbr[PARTITION_TABLE + 8..PARTITION_TABLE + 12].copy_from_slice(&2048u32.to_le_bytes());
        mbr[510] = 0x55;
        mbr[511] = 0xaa;
        let sector = boot_sector(&mbr, PartitionScheme::Mbr);

        assert_eq!(&sector[0..3], &[0xeb, 0x63, 0x90]);
        assert_eq!(
            u64::from_le_bytes(
                sector[KERNEL_SECTOR_OFFSET..KERNEL_SECTOR_OFFSET + 8]
                    .try_into()
                    .unwrap()
            ),
            CORE_LBA
        );
        assert_eq!(sector[BOOT_DRIVE_OFFSET], 0xff);
        assert_eq!(
            sector[PRESERVE_FROM..PRESERVE_FROM + 4],
            0x1234_5678u32.to_le_bytes()
        );
        assert_eq!(sector[PARTITION_TABLE], 0x80);
        assert_eq!(sector[PARTITION_TABLE + 4], 0x0c);
        assert_eq!(&sector[510..512], &[0x55, 0xaa]);
    }

    #[test]
    fn gpt_protective_mbr_is_not_activated() {
        let mut mbr = [0u8; 512];
        mbr[PARTITION_TABLE + 4] = 0xee;
        mbr[510] = 0x55;
        mbr[511] = 0xaa;
        let sector = boot_sector(&mbr, PartitionScheme::Gpt);
        assert_eq!(sector[PARTITION_TABLE], 0);
        assert_eq!(sector[PARTITION_TABLE + 4], 0xee);
    }

    #[test]
    fn core_blocklist_is_contiguous_after_the_core_sector() {
        for scheme in [PartitionScheme::Mbr, PartitionScheme::Gpt] {
            let core = core_image(scheme);
            let expected_len = (read_core_from_repo(scheme).len() as u64 - 512).div_ceil(512);
            let start = u64::from_le_bytes(
                core[DISKBOOT_BLOCKLIST..DISKBOOT_BLOCKLIST + 8]
                    .try_into()
                    .unwrap(),
            );
            let len = u16::from_le_bytes(
                core[DISKBOOT_BLOCKLIST + 8..DISKBOOT_BLOCKLIST + 10]
                    .try_into()
                    .unwrap(),
            );
            let segment = u16::from_le_bytes(
                core[DISKBOOT_BLOCKLIST + 10..DISKBOOT_BLOCKLIST + 12]
                    .try_into()
                    .unwrap(),
            );
            assert_eq!(start, CORE_LBA + 1);
            assert_eq!(u64::from(len), expected_len);
            assert_eq!(segment, KERNEL_SEGMENT);
            assert_eq!(core.len() as u64 % SECTOR_SIZE, 0);
            assert_eq!(&core[0..2], &[0x52, 0x56]);
        }
    }

    #[test]
    fn four_kn_layouts_do_not_use_the_512_byte_bios_chain() {
        let layout = crate::layout::plan_layout(96 * 1024 * 1024, 4096).unwrap();
        assert_eq!(layout.sector_size, 4096);
        assert!(!fits(&layout, PartitionScheme::Mbr));
        assert!(!fits(&layout, PartitionScheme::Gpt));
    }

    #[test]
    fn install_writes_the_core_and_preserves_gpt_headers() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("disk.img");
        std::fs::write(&path, vec![0u8; 96 * 1024 * 1024]).unwrap();
        let mut device = FileDevice::open(&path).unwrap();
        let gpt = crate::gpt::plan_gpt_layout(device.size(), device.logical_sector_size()).unwrap();
        crate::gpt::write_gpt(&mut device, &gpt).unwrap();

        install(&mut device, &gpt.partition, PartitionScheme::Gpt).unwrap();

        let mut boot = [0u8; 512];
        device.read_exact_at(0, &mut boot).unwrap();
        assert_eq!(boot[PARTITION_TABLE + 4], 0xee);
        assert_eq!(u64::from_le_bytes(boot[0x5c..0x64].try_into().unwrap()), 34);

        let mut header = [0u8; 92];
        device.read_exact_at(512, &mut header).unwrap();
        assert_eq!(&header[0..8], b"EFI PART");

        let mut core = [0u8; 512];
        device.read_exact_at(34 * 512, &mut core).unwrap();
        assert_eq!(&core[0..2], &[0x52, 0x56]);
    }
}
