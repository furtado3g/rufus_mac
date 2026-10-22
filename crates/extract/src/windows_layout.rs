//! Two-partition layout used by Windows NTFS extraction.
//!
//! Windows installation media larger than the FAT32 file limit is written to
//! an NTFS data partition. UEFI firmware cannot read NTFS directly, so a small
//! FAT partition containing the pinned UEFI:NTFS chainloader is appended to
//! the same disk. The chainloader finds the NTFS partition and boots the
//! Windows EFI loader from it.
//!
//! Both partitions use 1 MiB alignment. The 1 MiB area at the start is left
//! free for partition tables and future boot code, and the final 1 MiB is
//! reserved so neither the GPT backup structures nor the ESP touch the end of
//! the device. This path currently requires 512-byte logical sectors; the
//! UEFI:NTFS image and its FAT BPB have not been validated on 4Kn media.

use std::time::{SystemTime, UNIX_EPOCH};

use image2usb_core::{Error, RawIo, Result};

use crate::layout::PartitionLayout;

/// Exact UEFI:NTFS image shipped with the application.
pub const UEFI_NTFS_IMAGE: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../assets/uefi-ntfs/uefi-ntfs.img"
));

/// The pinned image is a complete 1 MiB FAT disk image.
pub const UEFI_NTFS_BYTES: u64 = 1024 * 1024;

const MIB: u64 = 1024 * 1024;
const RESERVED_BYTES: u64 = MIB;
const MIN_DATA_BYTES: u64 = 33 * MIB;

const MBR_TYPE_NTFS: u8 = 0x07;
const MBR_TYPE_ESP: u8 = 0xEF;

/// Microsoft basic data partition type GUID
/// `EBD0A0A2-B9E5-4433-87C0-68B6B72699C7`, in GPT mixed-endian order.
const NTFS_TYPE_GUID: [u8; 16] = [
    0xA2, 0xA0, 0xD0, 0xEB, 0xE5, 0xB9, 0x33, 0x44, 0x87, 0xC0, 0x68, 0xB6, 0xB7, 0x26, 0x99, 0xC7,
];

/// EFI System Partition type GUID
/// `C12A7328-F81F-11D2-BA4B-00A0C93EC93B`, in GPT mixed-endian order.
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
pub struct WindowsLayout {
    /// NTFS partition holding the Windows installation tree.
    pub data: PartitionLayout,
    /// FAT partition holding the exact UEFI:NTFS image.
    pub esp: PartitionLayout,
    /// GPT metadata when the requested scheme is GPT.
    pub gpt: Option<WindowsGptLayout>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowsGptLayout {
    pub device_sectors: u64,
    pub last_lba: u64,
    pub entry_sectors: u64,
    pub disk_guid: [u8; 16],
    pub data_guid: [u8; 16],
    pub esp_guid: [u8; 16],
}

impl WindowsGptLayout {
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowsScheme {
    Mbr,
    Gpt,
}

/// Plan the NTFS data partition and the fixed-size UEFI:NTFS ESP.
pub fn plan_windows_layout(
    device_size: u64,
    sector_size: u32,
    scheme: WindowsScheme,
) -> Result<WindowsLayout> {
    if sector_size != 512 {
        return Err(Error::Unsupported(format!(
            "NTFS/UEFI:NTFS Windows media currently requires 512-byte logical sectors; found \
             {sector_size}-byte sectors"
        )));
    }
    if !device_size.is_multiple_of(u64::from(sector_size)) {
        return Err(Error::Unsupported(format!(
            "device size must be a multiple of its {sector_size}-byte logical sector"
        )));
    }
    if UEFI_NTFS_IMAGE.len() as u64 != UEFI_NTFS_BYTES {
        return Err(Error::Other(
            "embedded UEFI:NTFS image has an unexpected length".to_owned(),
        ));
    }

    let sector = u64::from(sector_size);
    let start_byte = RESERVED_BYTES;
    let minimum = start_byte
        .checked_add(MIN_DATA_BYTES)
        .and_then(|value| value.checked_add(UEFI_NTFS_BYTES))
        .and_then(|value| value.checked_add(RESERVED_BYTES))
        .ok_or_else(|| Error::Unsupported("device size overflow".to_owned()))?;
    if device_size < minimum {
        return Err(Error::Unsupported(format!(
            "NTFS/UEFI:NTFS extraction needs a device of at least {} MiB (found {})",
            minimum.div_ceil(MIB),
            image2usb_core::util::format_bytes(device_size)
        )));
    }

    // Keep the ESP at the end of the usable area. Align its start down to a
    // 1 MiB boundary so both the GPT entry and any firmware heuristics see a
    // conventional layout; any remainder becomes harmless space before the
    // final reserved MiB.
    let esp_limit = device_size - RESERVED_BYTES;
    let esp_start = ((esp_limit - UEFI_NTFS_BYTES) / MIB) * MIB;
    if esp_start < start_byte + MIN_DATA_BYTES {
        return Err(Error::Unsupported(
            "device is too small for the NTFS data partition and UEFI:NTFS ESP".to_owned(),
        ));
    }
    let data_len = esp_start - start_byte;
    if !data_len.is_multiple_of(sector) {
        return Err(Error::Unsupported(
            "NTFS data partition is not aligned to the logical sector size".to_owned(),
        ));
    }
    let data_sectors = data_len / sector;
    let esp_sectors = UEFI_NTFS_BYTES / sector;
    if data_sectors > u64::from(u32::MAX) {
        return Err(Error::Unsupported(
            "NTFS partitions larger than 2 TiB are not supported".to_owned(),
        ));
    }

    let data = PartitionLayout {
        start_lba: (start_byte / sector) as u32,
        sectors: data_sectors as u32,
        start_byte,
        len: data_len,
        sector_size,
    };
    let esp = PartitionLayout {
        start_lba: (esp_start / sector) as u32,
        sectors: esp_sectors as u32,
        start_byte: esp_start,
        len: UEFI_NTFS_BYTES,
        sector_size,
    };
    if esp.end_byte() > esp_limit {
        return Err(Error::Other(
            "planned UEFI:NTFS partition exceeds the reserved area".to_owned(),
        ));
    }

    let gpt = match scheme {
        WindowsScheme::Mbr => None,
        WindowsScheme::Gpt => {
            let device_sectors = device_size / sector;
            if device_sectors < 2 + 2 * ((ENTRY_BYTES as u64).div_ceil(sector)) + 1 {
                return Err(Error::Unsupported(
                    "device is too small for a GPT partition table".to_owned(),
                ));
            }
            let last_lba = device_sectors - 1;
            let entry_sectors = (ENTRY_BYTES as u64).div_ceil(sector);
            let gpt = WindowsGptLayout {
                device_sectors,
                last_lba,
                entry_sectors,
                disk_guid: make_guid(0x5753_4E54_4653_0001),
                data_guid: make_guid(0x5753_4E54_4653_0002),
                esp_guid: make_guid(0x5753_4E54_4653_0003),
            };
            if u64::from(data.start_lba) < gpt.first_usable_lba()
                || esp.end_lba() > gpt.last_usable_lba()
            {
                return Err(Error::Unsupported(
                    "device is too small for the two GPT partitions".to_owned(),
                ));
            }
            Some(gpt)
        }
    };

    Ok(WindowsLayout { data, esp, gpt })
}

/// Write the selected partition table. Filesystem contents are written later.
pub fn write_windows_layout(device: &mut dyn RawIo, layout: &WindowsLayout) -> Result<()> {
    match &layout.gpt {
        Some(gpt) => write_gpt(device, layout, gpt),
        None => write_mbr(device, layout),
    }
}

fn write_mbr(device: &mut dyn RawIo, layout: &WindowsLayout) -> Result<()> {
    wipe_range(device, 0, layout.data.start_byte)?;
    let mut mbr = [0u8; 512];
    write_mbr_entry(&mut mbr[446..462], MBR_TYPE_NTFS, &layout.data);
    write_mbr_entry(&mut mbr[462..478], MBR_TYPE_ESP, &layout.esp);
    mbr[510] = 0x55;
    mbr[511] = 0xAA;
    let mut sector = vec![0u8; layout.data.sector_size as usize];
    sector[..512].copy_from_slice(&mbr);
    device.write_all_at(0, &sector)?;
    device.sync()?;
    Ok(())
}

fn write_mbr_entry(entry: &mut [u8], partition_type: u8, layout: &PartitionLayout) {
    // CHS values are intentionally conventional dummies; UEFI ignores them and
    // this path does not install a legacy BIOS bootloader.
    entry[0] = 0x00;
    entry[1..4].copy_from_slice(&[0xFE, 0xFF, 0xFF]);
    entry[4] = partition_type;
    entry[5..8].copy_from_slice(&[0xFE, 0xFF, 0xFF]);
    entry[8..12].copy_from_slice(&layout.start_lba.to_le_bytes());
    entry[12..16].copy_from_slice(&layout.sectors.to_le_bytes());
}

fn write_gpt(device: &mut dyn RawIo, layout: &WindowsLayout, gpt: &WindowsGptLayout) -> Result<()> {
    let sector = layout.data.sector_size as usize;
    let sector_u64 = sector as u64;
    let device_bytes = gpt.device_sectors * sector_u64;

    wipe_range(device, 0, layout.data.start_byte)?;
    let backup_start = gpt.backup_entries_lba() * sector_u64;
    wipe_range(device, backup_start, device_bytes - backup_start)?;

    let mut mbr = vec![0u8; sector];
    let entry = &mut mbr[446..462];
    entry[1..4].copy_from_slice(&[0x00, 0x02, 0x00]);
    entry[4] = 0xEE;
    entry[5..8].copy_from_slice(&[0xFF, 0xFF, 0xFF]);
    entry[8..12].copy_from_slice(&1u32.to_le_bytes());
    let protective_sectors = (gpt.device_sectors - 1).min(u64::from(u32::MAX)) as u32;
    entry[12..16].copy_from_slice(&protective_sectors.to_le_bytes());
    mbr[510] = 0x55;
    mbr[511] = 0xAA;
    device.write_all_at(0, &mbr)?;

    let mut entries = vec![0u8; ENTRY_BYTES];
    write_gpt_entry(
        &mut entries[..ENTRY_SIZE as usize],
        NTFS_TYPE_GUID,
        gpt.data_guid,
        &layout.data,
        "Basic data partition",
    );
    write_gpt_entry(
        &mut entries[ENTRY_SIZE as usize..2 * ENTRY_SIZE as usize],
        ESP_TYPE_GUID,
        gpt.esp_guid,
        &layout.esp,
        "EFI System Partition",
    );
    let entries_crc = crc32(&entries);
    let primary = make_header(gpt, 1, gpt.last_lba, 2, entries_crc);
    let backup = make_header(gpt, gpt.last_lba, 1, gpt.backup_entries_lba(), entries_crc);
    device.write_all_at(sector_u64, &sector_padded(&primary, sector))?;
    device.write_all_at(2 * sector_u64, &entries)?;
    device.write_all_at(gpt.backup_entries_lba() * sector_u64, &entries)?;
    device.write_all_at(gpt.last_lba * sector_u64, &sector_padded(&backup, sector))?;
    device.sync()?;
    Ok(())
}

fn write_gpt_entry(
    entry: &mut [u8],
    type_guid: [u8; 16],
    unique_guid: [u8; 16],
    partition: &PartitionLayout,
    name: &str,
) {
    entry[0..16].copy_from_slice(&type_guid);
    entry[16..32].copy_from_slice(&unique_guid);
    entry[32..40].copy_from_slice(&u64::from(partition.start_lba).to_le_bytes());
    entry[40..48].copy_from_slice(&partition.end_lba().to_le_bytes());
    entry[48..56].copy_from_slice(&0u64.to_le_bytes());
    let mut units = name.encode_utf16();
    for index in 0..36 {
        let unit = units.next().unwrap_or(0);
        entry[56 + index * 2..58 + index * 2].copy_from_slice(&unit.to_le_bytes());
    }
}

fn make_header(
    gpt: &WindowsGptLayout,
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
    header[40..48].copy_from_slice(&gpt.first_usable_lba().to_le_bytes());
    header[48..56].copy_from_slice(&gpt.last_usable_lba().to_le_bytes());
    header[56..72].copy_from_slice(&gpt.disk_guid);
    header[72..80].copy_from_slice(&entry_lba.to_le_bytes());
    header[80..84].copy_from_slice(&ENTRY_COUNT.to_le_bytes());
    header[84..88].copy_from_slice(&ENTRY_SIZE.to_le_bytes());
    header[88..92].copy_from_slice(&entries_crc.to_le_bytes());
    let crc = crc32(&header);
    header[16..20].copy_from_slice(&crc.to_le_bytes());
    header
}

fn wipe_range(device: &mut dyn RawIo, start: u64, len: u64) -> Result<()> {
    let zeros = vec![0u8; 64 * 1024];
    let mut offset = start;
    let end = start
        .checked_add(len)
        .ok_or_else(|| Error::Unsupported("wipe range overflow".to_owned()))?;
    while offset < end {
        let count = (end - offset).min(zeros.len() as u64) as usize;
        device.write_all_at(offset, &zeros[..count])?;
        offset += count as u64;
    }
    Ok(())
}

fn sector_padded(header: &[u8], sector: usize) -> Vec<u8> {
    let mut out = vec![0u8; sector];
    out[..header.len()].copy_from_slice(header);
    out
}

fn make_guid(seed: u64) -> [u8; 16] {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos() as u64)
        .unwrap_or(0);
    let mut state = seed ^ now ^ u64::from(std::process::id());
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
    fn plans_two_non_overlapping_partitions() {
        let layout = plan_windows_layout(128 * MIB, 512, WindowsScheme::Mbr).unwrap();
        assert_eq!(layout.data.start_byte, MIB);
        assert_eq!(layout.data.len % MIB, 0);
        assert_eq!(layout.esp.len, UEFI_NTFS_BYTES);
        assert_eq!(layout.esp.start_byte % MIB, 0);
        assert!(layout.data.end_byte() <= layout.esp.start_byte);
        assert!(layout.esp.end_byte() <= 127 * MIB);
        assert!(layout.gpt.is_none());
    }

    #[test]
    fn plans_gpt_within_usable_bounds() {
        let layout = plan_windows_layout(128 * MIB, 512, WindowsScheme::Gpt).unwrap();
        let gpt = layout.gpt.as_ref().unwrap();
        assert_eq!(gpt.first_usable_lba(), 34);
        assert!(u64::from(layout.data.start_lba) >= gpt.first_usable_lba());
        assert!(layout.esp.end_lba() <= gpt.last_usable_lba());
        assert!(layout.esp.end_lba() < gpt.backup_entries_lba());
    }

    #[test]
    fn rejects_4k_sectors_for_the_unvalidated_boot_asset() {
        let error = plan_windows_layout(256 * MIB, 4096, WindowsScheme::Mbr).unwrap_err();
        assert!(error.to_string().contains("512-byte"), "{error}");
    }

    #[test]
    fn writes_two_mbr_entries() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mbr.img");
        std::fs::write(&path, vec![0u8; (64 * MIB) as usize]).unwrap();
        let mut device = FileDevice::open(&path).unwrap();
        let layout = plan_windows_layout(device.size(), 512, WindowsScheme::Mbr).unwrap();
        write_windows_layout(&mut device, &layout).unwrap();

        let mut mbr = [0u8; 512];
        device.read_exact_at(0, &mut mbr).unwrap();
        assert_eq!(&mbr[510..512], &[0x55, 0xAA]);
        assert_eq!(mbr[446 + 4], MBR_TYPE_NTFS);
        assert_eq!(mbr[462 + 4], MBR_TYPE_ESP);
        assert_eq!(
            u32::from_le_bytes(mbr[446 + 8..446 + 12].try_into().unwrap()),
            layout.data.start_lba
        );
        assert_eq!(
            u32::from_le_bytes(mbr[462 + 8..462 + 12].try_into().unwrap()),
            layout.esp.start_lba
        );
    }

    #[test]
    fn writes_gpt_entries_and_headers() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("gpt.img");
        std::fs::write(&path, vec![0u8; (64 * MIB) as usize]).unwrap();
        let mut device = FileDevice::open(&path).unwrap();
        let layout = plan_windows_layout(device.size(), 512, WindowsScheme::Gpt).unwrap();
        write_windows_layout(&mut device, &layout).unwrap();
        let gpt = layout.gpt.as_ref().unwrap();

        let mut primary = [0u8; 512];
        device.read_exact_at(512, &mut primary).unwrap();
        assert_eq!(&primary[0..8], GPT_SIGNATURE);
        let stored_crc = u32::from_le_bytes(primary[16..20].try_into().unwrap());
        primary[16..20].fill(0);
        assert_eq!(stored_crc, crc32(&primary[..GPT_HEADER_SIZE]));

        let mut entries = vec![0u8; ENTRY_BYTES];
        device.read_exact_at(1024, &mut entries).unwrap();
        assert_eq!(&entries[0..16], &NTFS_TYPE_GUID);
        assert_eq!(
            u64::from_le_bytes(entries[32..40].try_into().unwrap()),
            u64::from(layout.data.start_lba)
        );
        assert_eq!(
            u64::from_le_bytes(entries[40..48].try_into().unwrap()),
            layout.data.end_lba()
        );
        assert_eq!(
            &entries[ENTRY_SIZE as usize..ENTRY_SIZE as usize + 16],
            &ESP_TYPE_GUID
        );
        assert_eq!(
            u64::from_le_bytes(
                entries[ENTRY_SIZE as usize + 32..ENTRY_SIZE as usize + 40]
                    .try_into()
                    .unwrap()
            ),
            u64::from(layout.esp.start_lba)
        );

        let mut backup = [0u8; 512];
        device
            .read_exact_at(gpt.last_lba * 512, &mut backup)
            .unwrap();
        assert_eq!(&backup[0..8], GPT_SIGNATURE);
        assert_eq!(
            u64::from_le_bytes(backup[24..32].try_into().unwrap()),
            gpt.last_lba
        );
    }
}
