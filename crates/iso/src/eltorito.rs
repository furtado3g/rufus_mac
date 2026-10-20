use std::fs::File;

use serde::{Deserialize, Serialize};

use crate::iso9660::{SECTOR_SIZE, le_u16, le_u32, read_sector};
use crate::{IsoError, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Platform {
    X86,
    PowerPc,
    Mac,
    Efi,
    Unknown(u8),
}

impl Platform {
    fn from_id(id: u8) -> Self {
        match id {
            0x00 => Platform::X86,
            0x01 => Platform::PowerPc,
            0x02 => Platform::Mac,
            0xEF => Platform::Efi,
            other => Platform::Unknown(other),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaType {
    NoEmulation,
    Floppy1_2,
    Floppy1_44,
    Floppy2_88,
    HardDisk,
    Unknown(u8),
}

impl MediaType {
    fn from_id(id: u8) -> Self {
        match id {
            0 => MediaType::NoEmulation,
            1 => MediaType::Floppy1_2,
            2 => MediaType::Floppy1_44,
            3 => MediaType::Floppy2_88,
            4 => MediaType::HardDisk,
            other => MediaType::Unknown(other),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BootEntry {
    pub platform: Platform,
    pub bootable: bool,
    pub media: MediaType,
    pub load_segment: u16,
    pub sector_count: u16,
    pub load_lba: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ElToritoInfo {
    pub catalog_lba: u32,
    /// A bootable x86 entry is present (legacy BIOS).
    pub bios: bool,
    /// A bootable EFI entry is present.
    pub uefi: bool,
    pub entries: Vec<BootEntry>,
}

/// Parse an El Torito boot catalog.
pub fn parse_catalog(file: &mut File, catalog_lba: u32) -> Result<ElToritoInfo> {
    if catalog_lba == 0 {
        return Err(IsoError::InvalidElTorito("catalog LBA is zero".to_owned()));
    }
    let sector = read_sector(file, catalog_lba as u64)?;

    if sector[0] != 0x01 {
        return Err(IsoError::InvalidElTorito(format!(
            "validation entry has type 0x{:02x}, expected 0x01",
            sector[0]
        )));
    }
    let sum: u32 = sector[..32]
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| le_u16(c) as u32)
        .sum();
    if !sum.is_multiple_of(65536) {
        return Err(IsoError::InvalidElTorito(
            "validation entry checksum mismatch".to_owned(),
        ));
    }
    if sector[30] != 0x55 || sector[31] != 0xAA {
        return Err(IsoError::InvalidElTorito(
            "validation entry signature 0x55AA missing".to_owned(),
        ));
    }
    let platform = Platform::from_id(sector[1]);

    let mut entries = Vec::new();
    let mut offset = 32usize;
    let mut current_platform = platform;
    while offset + 32 <= SECTOR_SIZE as usize {
        let entry = &sector[offset..offset + 32];
        let indicator = entry[0];
        // The initial entry at offset 32 may legitimately be non-bootable
        // (0x00). Later 0x00 bytes mean the catalog is exhausted.
        if indicator == 0x00 && offset > 32 {
            break;
        }
        if indicator == 0x90 || indicator == 0x91 {
            current_platform = Platform::from_id(entry[1]);
            offset += 32;
            continue;
        }
        if indicator == 0x88 || indicator == 0x00 {
            entries.push(BootEntry {
                platform: current_platform,
                bootable: indicator == 0x88,
                media: MediaType::from_id(entry[1]),
                load_segment: le_u16(&entry[2..4]),
                sector_count: le_u16(&entry[6..8]),
                load_lba: le_u32(&entry[8..12]),
            });
        }
        offset += 32;
    }

    let bios = entries
        .iter()
        .any(|e| e.bootable && matches!(e.platform, Platform::X86));
    let uefi = entries
        .iter()
        .any(|e| e.bootable && matches!(e.platform, Platform::Efi));

    Ok(ElToritoInfo {
        catalog_lba,
        bios,
        uefi,
        entries,
    })
}
