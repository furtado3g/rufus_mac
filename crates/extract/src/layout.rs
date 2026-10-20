//! Partition layout and MBR generation for extraction mode.
//!
//! MBR mode writes a single type `0x0C` (FAT32 LBA) partition. UEFI firmware on
//! removable media scans every FAT partition for `\EFI\BOOT\BOOT<arch>.EFI`, so
//! the ESP type is not required for booting, but GPT mode provides it anyway.
//! GPT generation lives in `gpt.rs`; a BIOS boot record is planned for the next
//! phase.

use image2usb_core::{Error, RawIo, Result};

/// MBR partition type: FAT32 LBA.
const PARTITION_TYPE_FAT32_LBA: u8 = 0x0C;

/// Leave 1 MiB at the start for the MBR/boot area and 1 MiB at the end so the
/// partition never touches the last sector of the device.
const RESERVED_BYTES: u64 = 1024 * 1024;

/// FAT32 needs at least 65525 clusters; with 512-byte clusters this is ~32 MiB.
const MIN_PARTITION_BYTES: u64 = 33 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PartitionLayout {
    /// First sector of the partition, in device logical sectors.
    pub start_lba: u32,
    /// Number of sectors in the partition.
    pub sectors: u32,
    /// Byte offset of the partition inside the device.
    pub start_byte: u64,
    /// Byte length of the partition.
    pub len: u64,
    /// Logical sector size used for the MBR fields.
    pub sector_size: u32,
}

impl PartitionLayout {
    pub fn end_byte(&self) -> u64 {
        self.start_byte + self.len
    }

    pub fn end_lba(&self) -> u64 {
        u64::from(self.start_lba) + u64::from(self.sectors) - 1
    }
}

pub fn plan_layout(device_size: u64, sector_size: u32) -> Result<PartitionLayout> {
    let sector_size = sector_size.max(512).next_power_of_two() as u64;
    let start_byte = RESERVED_BYTES;
    if device_size < start_byte + RESERVED_BYTES + MIN_PARTITION_BYTES {
        return Err(Error::Unsupported(format!(
            "extraction needs a device of at least {} MiB (found {})",
            64,
            image2usb_core::util::format_bytes(device_size)
        )));
    }
    let len = ((device_size - start_byte - RESERVED_BYTES) / sector_size) * sector_size;
    let sectors = len / sector_size;
    if sectors > u32::MAX as u64 {
        return Err(Error::Unsupported(
            "FAT32 partitions larger than 2 TiB are not supported".to_owned(),
        ));
    }
    Ok(PartitionLayout {
        start_lba: (start_byte / sector_size) as u32,
        sectors: sectors as u32,
        start_byte,
        len,
        sector_size: sector_size as u32,
    })
}

/// Write a clean MBR with one FAT32 partition and zero out the reserved area.
pub fn write_mbr(device: &mut dyn RawIo, layout: &PartitionLayout) -> Result<()> {
    // Wipe the first MiB so stale hybrid boot code or a previous partition
    // table cannot survive under the new one.
    let zeros = vec![0u8; 64 * 1024];
    let mut offset = 0u64;
    while offset < layout.start_byte {
        let n = (layout.start_byte - offset).min(zeros.len() as u64) as usize;
        device.write_all_at(offset, &zeros[..n])?;
        offset += n as u64;
    }

    let mut mbr = [0u8; 512];
    let entry = &mut mbr[446..462];
    entry[0] = 0x00; // not active: extraction currently installs no BIOS boot record
    entry[1..4].copy_from_slice(&[0xFE, 0xFF, 0xFF]); // CHS start (dummy for large disks)
    entry[4] = PARTITION_TYPE_FAT32_LBA;
    entry[5..8].copy_from_slice(&[0xFE, 0xFF, 0xFF]); // CHS end
    entry[8..12].copy_from_slice(&layout.start_lba.to_le_bytes());
    entry[12..16].copy_from_slice(&layout.sectors.to_le_bytes());
    mbr[510] = 0x55;
    mbr[511] = 0xAA;

    // Write at least one full logical sector: 4Kn devices reject a 512-byte
    // write even though the MBR itself is only 512 bytes long.
    let mut sector = vec![0u8; layout.sector_size.max(512) as usize];
    sector[..512].copy_from_slice(&mbr);
    device.write_all_at(0, &sector)?;
    device.sync()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plans_aligned_layout() {
        let size = 128 * 1024 * 1024;
        let layout = plan_layout(size, 512).unwrap();
        assert_eq!(layout.start_byte, 1024 * 1024);
        assert_eq!(layout.start_lba, 2048);
        assert_eq!(layout.sector_size, 512);
        assert_eq!(layout.len % 512, 0);
        assert!(layout.end_byte() <= size - 1024 * 1024);
        assert_eq!(layout.sectors as u64 * 512, layout.len);
    }

    #[test]
    fn rejects_small_devices() {
        assert!(plan_layout(32 * 1024 * 1024, 512).is_err());
    }

    #[test]
    fn handles_4k_sectors() {
        let layout = plan_layout(256 * 1024 * 1024, 4096).unwrap();
        assert_eq!(layout.start_lba, 256);
        assert_eq!(layout.start_byte % 4096, 0);
        assert_eq!(layout.len % 4096, 0);
    }
}
