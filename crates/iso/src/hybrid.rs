use std::fs::File;
use std::io::{Read, Seek, SeekFrom};

use serde::{Deserialize, Serialize};

use crate::Result;
use crate::iso9660::{SECTOR_SIZE, le_u32, read_sector};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MbrPartition {
    pub bootable: bool,
    pub partition_type: u8,
    pub start_lba: u32,
    pub sectors: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MbrInfo {
    pub signature_valid: bool,
    pub boot_code_nonzero: bool,
    pub partitions: Vec<MbrPartition>,
}

impl MbrInfo {
    /// A hybrid ISO that boots in legacy BIOS mode when copied 1:1.
    pub fn isohybrid_bootable(&self) -> bool {
        self.signature_valid
            && self.boot_code_nonzero
            && self
                .partitions
                .iter()
                .any(|p| p.bootable && p.start_lba > 0 && p.sectors > 0 && p.partition_type != 0xEE)
    }

    pub fn has_efi_partition(&self) -> bool {
        self.partitions
            .iter()
            .any(|p| p.partition_type == 0xEF && p.sectors > 0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GptInfo {
    pub has_esp: bool,
    pub entry_count: u32,
}

/// GUID `C12A7328-F81F-11D2-BA4B-00A0C93EC93B` in on-disk mixed-endian order.
const ESP_GUID: [u8; 16] = [
    0x28, 0x73, 0x2A, 0xC1, 0x1F, 0xF8, 0xD2, 0x11, 0xBA, 0x4B, 0x00, 0xA0, 0xC9, 0x3E, 0xC9, 0x3B,
];

pub fn inspect_mbr(file: &mut File) -> Result<Option<MbrInfo>> {
    let mut mbr = [0u8; 512];
    file.seek(SeekFrom::Start(0))?;
    if file.read_exact(&mut mbr).is_err() {
        return Ok(None);
    }
    let signature_valid = mbr[510] == 0x55 && mbr[511] == 0xAA;
    if !signature_valid {
        return Ok(None);
    }
    let boot_code_nonzero = mbr[..440].iter().any(|&b| b != 0);
    let mut partitions = Vec::new();
    for i in 0..4 {
        let base = 446 + i * 16;
        let partition_type = mbr[base + 4];
        let start_lba = le_u32(&mbr[base + 8..base + 12]);
        let sectors = le_u32(&mbr[base + 12..base + 16]);
        if partition_type == 0 && start_lba == 0 && sectors == 0 {
            continue;
        }
        partitions.push(MbrPartition {
            bootable: mbr[base] == 0x80,
            partition_type,
            start_lba,
            sectors,
        });
    }
    Ok(Some(MbrInfo {
        signature_valid,
        boot_code_nonzero,
        partitions,
    }))
}

pub fn inspect_gpt(file: &mut File, image_size: u64) -> Result<Option<GptInfo>> {
    if image_size < 3 * SECTOR_SIZE {
        return Ok(None);
    }
    let header = read_sector(file, 1)?;
    if &header[0..8] != b"EFI PART" {
        return Ok(None);
    }
    let entry_lba = u64::from(le_u32(&header[72..76]));
    let entry_count = le_u32(&header[80..84]);
    let entry_size = le_u32(&header[84..88]) as usize;
    if entry_count == 0 || entry_size < 128 || entry_count > 1024 {
        return Ok(None);
    }

    let mut has_esp = false;
    let max_entries = entry_count.min(128) as usize;
    let read_len = max_entries * entry_size;
    let mut data = vec![0u8; read_len];
    file.seek(SeekFrom::Start(entry_lba * SECTOR_SIZE))?;
    if file.read_exact(&mut data).is_err() {
        return Ok(Some(GptInfo {
            has_esp: false,
            entry_count,
        }));
    }
    for entry in data.chunks_exact(entry_size) {
        if entry.len() < 128 {
            break;
        }
        let is_used = entry[..16].iter().any(|&b| b != 0);
        if is_used && entry[..16] == ESP_GUID {
            has_esp = true;
            break;
        }
    }
    Ok(Some(GptInfo {
        has_esp,
        entry_count,
    }))
}
