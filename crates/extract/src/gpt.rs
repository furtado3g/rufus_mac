//! GPT partition table with a single EFI System Partition.
//!
//! Extraction still writes one FAT32 volume; the only difference from MBR mode
//! is the partition table and the ESP type GUID. Keeping a single ESP means a
//! large tree can only be extracted when it fits in that volume, which is the
//! same limit as MBR mode.

use std::time::{SystemTime, UNIX_EPOCH};

use image2usb_core::{Error, RawIo, Result};

use crate::layout::{PartitionLayout, plan_layout};

/// EFI System Partition type GUID `C12A7328-F81F-11D2-BA4B-00A0C93EC93B` in
/// the mixed-endian byte order used by GPT.
const ESP_TYPE_GUID: [u8; 16] = [
    0x28, 0x73, 0x2A, 0xC1, 0x1F, 0xF8, 0xD2, 0x11, 0xBA, 0x4B, 0x00, 0xA0, 0xC9, 0x3E, 0xC9, 0x3B,
];

const GPT_SIGNATURE: &[u8; 8] = b"EFI PART";
const GPT_REVISION: u32 = 0x0001_0000;
const GPT_HEADER_SIZE: usize = 92;
const ENTRY_COUNT: u32 = 128;
const ENTRY_SIZE: u32 = 128;
const ENTRY_BYTES: usize = ENTRY_COUNT as usize * ENTRY_SIZE as usize;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GptLayout {
    /// FAT32 ESP inside the GPT.
    pub partition: PartitionLayout,
    /// Total logical sectors in the target device.
    pub device_sectors: u64,
    /// Last logical sector (backup GPT header).
    pub last_lba: u64,
    /// Number of sectors occupied by the 128-entry partition array.
    pub entry_sectors: u64,
    /// Disk GUID stored in both GPT headers.
    pub disk_guid: [u8; 16],
}

impl GptLayout {
    pub fn first_usable_lba(&self) -> u64 {
        2 + self.entry_sectors
    }

    pub fn last_usable_lba(&self) -> u64 {
        self.last_lba - 1 - self.entry_sectors
    }

    pub fn backup_entries_lba(&self) -> u64 {
        self.last_lba - self.entry_sectors
    }
}

pub fn plan_gpt_layout(device_size: u64, sector_size: u32) -> Result<GptLayout> {
    let partition = plan_layout(device_size, sector_size)?;
    let sector = u64::from(partition.sector_size);
    if !device_size.is_multiple_of(sector) {
        return Err(Error::Unsupported(format!(
            "GPT requires a device size that is a multiple of its {sector}-byte logical sector"
        )));
    }
    let device_sectors = device_size / sector;
    if device_sectors < 2 + 2 * ((ENTRY_BYTES as u64).div_ceil(sector)) + 1 {
        return Err(Error::Unsupported(
            "device is too small for a GPT partition table".to_owned(),
        ));
    }
    let last_lba = device_sectors - 1;
    let entry_sectors = (ENTRY_BYTES as u64).div_ceil(sector);
    let layout = GptLayout {
        partition,
        device_sectors,
        last_lba,
        entry_sectors,
        disk_guid: make_guid(device_size, sector),
    };
    if u64::from(layout.partition.start_lba) < layout.first_usable_lba()
        || layout.partition.end_lba() > layout.last_usable_lba()
    {
        return Err(Error::Unsupported(
            "device is too small for a GPT data partition".to_owned(),
        ));
    }
    Ok(layout)
}

/// Write a protective MBR, primary GPT and backup GPT. The caller still writes
/// the FAT32 filesystem into `layout.partition`.
pub fn write_gpt(device: &mut dyn RawIo, layout: &GptLayout) -> Result<()> {
    let sector = layout.partition.sector_size as usize;
    let sector_u64 = sector as u64;

    // Remove stale partition tables. The first 1 MiB contains the protective
    // MBR, primary header and array; the backup area is overwritten below.
    let zeros = vec![0u8; 64 * 1024];
    let mut offset = 0u64;
    while offset < layout.partition.start_byte {
        let n = (layout.partition.start_byte - offset).min(zeros.len() as u64) as usize;
        device.write_all_at(offset, &zeros[..n])?;
        offset += n as u64;
    }
    let backup_start = layout.backup_entries_lba() * sector_u64;
    let mut offset = backup_start;
    while offset < layout.device_sectors * sector_u64 {
        let n = (layout.device_sectors * sector_u64 - offset).min(zeros.len() as u64) as usize;
        device.write_all_at(offset, &zeros[..n])?;
        offset += n as u64;
    }

    let mut mbr = vec![0u8; sector];
    let entry = &mut mbr[446..462];
    entry[1..4].copy_from_slice(&[0x00, 0x02, 0x00]);
    entry[4] = 0xEE; // protective GPT partition
    entry[5..8].copy_from_slice(&[0xFF, 0xFF, 0xFF]);
    entry[8..12].copy_from_slice(&1u32.to_le_bytes());
    let protective_sectors = (layout.device_sectors - 1).min(u64::from(u32::MAX)) as u32;
    entry[12..16].copy_from_slice(&protective_sectors.to_le_bytes());
    mbr[510] = 0x55;
    mbr[511] = 0xAA;
    device.write_all_at(0, &mbr)?;

    let mut entries = vec![0u8; ENTRY_BYTES];
    write_partition_entry(&mut entries[..ENTRY_SIZE as usize], layout);
    let entries_crc = crc32(&entries);

    let primary = make_header(layout, 1, layout.last_lba, 2, entries_crc);
    let backup = make_header(
        layout,
        layout.last_lba,
        1,
        layout.backup_entries_lba(),
        entries_crc,
    );
    device.write_all_at(sector_u64, &sector_padded(&primary, sector))?;
    device.write_all_at(2 * sector_u64, &entries)?;
    device.write_all_at(layout.backup_entries_lba() * sector_u64, &entries)?;
    device.write_all_at(
        layout.last_lba * sector_u64,
        &sector_padded(&backup, sector),
    )?;
    device.sync()?;
    Ok(())
}

fn write_partition_entry(entry: &mut [u8], layout: &GptLayout) {
    entry[0..16].copy_from_slice(&ESP_TYPE_GUID);
    entry[16..32].copy_from_slice(&make_guid(
        layout.partition.len,
        u64::from(layout.partition.sector_size),
    ));
    entry[32..40].copy_from_slice(&u64::from(layout.partition.start_lba).to_le_bytes());
    entry[40..48].copy_from_slice(&layout.partition.end_lba().to_le_bytes());
    entry[48..56].copy_from_slice(&0u64.to_le_bytes());
    let name = "EFI System Partition".encode_utf16().collect::<Vec<_>>();
    for (index, unit) in name.iter().take(36).enumerate() {
        entry[56 + index * 2..58 + index * 2].copy_from_slice(&unit.to_le_bytes());
    }
}

fn make_header(
    layout: &GptLayout,
    current_lba: u64,
    backup_lba: u64,
    entry_lba: u64,
    entries_crc: u32,
) -> [u8; GPT_HEADER_SIZE] {
    let mut header = [0u8; GPT_HEADER_SIZE];
    header[0..8].copy_from_slice(GPT_SIGNATURE);
    header[8..12].copy_from_slice(&GPT_REVISION.to_le_bytes());
    header[12..16].copy_from_slice(&(GPT_HEADER_SIZE as u32).to_le_bytes());
    header[24..32].copy_from_slice(&current_lba.to_le_bytes());
    header[32..40].copy_from_slice(&backup_lba.to_le_bytes());
    header[40..48].copy_from_slice(&layout.first_usable_lba().to_le_bytes());
    header[48..56].copy_from_slice(&layout.last_usable_lba().to_le_bytes());
    header[56..72].copy_from_slice(&layout.disk_guid);
    header[72..80].copy_from_slice(&entry_lba.to_le_bytes());
    header[80..84].copy_from_slice(&ENTRY_COUNT.to_le_bytes());
    header[84..88].copy_from_slice(&ENTRY_SIZE.to_le_bytes());
    header[88..92].copy_from_slice(&entries_crc.to_le_bytes());
    let crc = crc32(&header);
    header[16..20].copy_from_slice(&crc.to_le_bytes());
    header
}

fn sector_padded(header: &[u8], sector: usize) -> Vec<u8> {
    let mut out = vec![0u8; sector];
    out[..header.len()].copy_from_slice(header);
    out
}

fn make_guid(seed: u64, sector: u64) -> [u8; 16] {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos() as u64)
        .unwrap_or(0);
    let mut state = seed ^ sector.rotate_left(17) ^ now ^ u64::from(std::process::id());
    let mut out = [0u8; 16];
    for chunk in out.chunks_mut(8) {
        state = splitmix64(state);
        chunk.copy_from_slice(&state.to_le_bytes());
    }
    out[6] = (out[6] & 0x0F) | 0x40;
    out[8] = (out[8] & 0x3F) | 0x80;
    out
}

fn splitmix64(mut value: u64) -> u64 {
    value = value.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = value;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &byte in data {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            let mask = 0u32.wrapping_sub(crc & 1);
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

#[cfg(test)]
mod tests {
    use super::*;
    use image2usb_core::FileDevice;

    #[test]
    fn crc32_matches_the_standard_check_value() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    }

    #[test]
    fn plans_aligned_gpt_partition() {
        let layout = plan_gpt_layout(128 * 1024 * 1024, 512).unwrap();
        assert_eq!(layout.partition.start_byte, 1024 * 1024);
        assert_eq!(layout.partition.start_lba, 2048);
        assert!(layout.partition.end_lba() <= layout.last_usable_lba());
        assert_eq!(layout.first_usable_lba(), 34);
        assert_eq!(layout.backup_entries_lba(), layout.last_lba - 32);
    }

    #[test]
    fn plans_gpt_with_4k_sectors() {
        let layout = plan_gpt_layout(256 * 1024 * 1024, 4096).unwrap();
        assert_eq!(layout.partition.start_lba, 256);
        assert_eq!(layout.entry_sectors, 4);
        assert_eq!(layout.first_usable_lba(), 6);
        assert_eq!(layout.backup_entries_lba(), layout.last_lba - 4);
        assert!(layout.partition.end_lba() <= layout.last_usable_lba());
    }

    #[test]
    fn writes_valid_primary_and_backup_headers() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("gpt.img");
        std::fs::write(&path, vec![0u8; 64 * 1024 * 1024]).unwrap();
        let mut device = FileDevice::open(&path).unwrap();
        let layout = plan_gpt_layout(device.size(), device.logical_sector_size()).unwrap();
        write_gpt(&mut device, &layout).unwrap();

        let mut mbr = [0u8; 512];
        device.read_exact_at(0, &mut mbr).unwrap();
        assert_eq!(&mbr[510..512], &[0x55, 0xAA]);
        assert_eq!(mbr[446 + 4], 0xEE);
        assert_eq!(
            u32::from_le_bytes(mbr[446 + 8..446 + 12].try_into().unwrap()),
            1
        );

        let mut primary = [0u8; 512];
        device.read_exact_at(512, &mut primary).unwrap();
        assert_eq!(&primary[0..8], GPT_SIGNATURE);
        assert_eq!(u64::from_le_bytes(primary[24..32].try_into().unwrap()), 1);
        assert_eq!(
            u64::from_le_bytes(primary[32..40].try_into().unwrap()),
            layout.last_lba
        );
        let stored_crc = u32::from_le_bytes(primary[16..20].try_into().unwrap());
        primary[16..20].fill(0);
        assert_eq!(stored_crc, crc32(&primary[..GPT_HEADER_SIZE]));

        let mut entries = vec![0u8; ENTRY_BYTES];
        device.read_exact_at(1024, &mut entries).unwrap();
        assert_eq!(
            u32::from_le_bytes(primary[88..92].try_into().unwrap()),
            crc32(&entries)
        );
        assert_eq!(&entries[0..16], &ESP_TYPE_GUID);
        assert_eq!(
            u64::from_le_bytes(entries[32..40].try_into().unwrap()),
            u64::from(layout.partition.start_lba)
        );
        assert_eq!(
            u64::from_le_bytes(entries[40..48].try_into().unwrap()),
            layout.partition.end_lba()
        );

        let mut backup = [0u8; 512];
        device
            .read_exact_at(layout.last_lba * 512, &mut backup)
            .unwrap();
        assert_eq!(&backup[0..8], GPT_SIGNATURE);
        assert_eq!(
            u64::from_le_bytes(backup[24..32].try_into().unwrap()),
            layout.last_lba
        );
        assert_eq!(u64::from_le_bytes(backup[32..40].try_into().unwrap()), 1);
        let stored_crc = u32::from_le_bytes(backup[16..20].try_into().unwrap());
        backup[16..20].fill(0);
        assert_eq!(stored_crc, crc32(&backup[..GPT_HEADER_SIZE]));
    }
}
