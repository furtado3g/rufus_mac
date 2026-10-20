use std::fs::File;
use std::io::{Read, Seek, SeekFrom};

use crate::{IsoError, Result};

pub const SECTOR_SIZE: u64 = 2048;
const MAX_DIR_BYTES: usize = 8 * 1024 * 1024;

/// A directory record as stored in an ISO 9660 directory extent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirRecord {
    /// Logical block address of the file or directory data.
    pub extent: u32,
    /// Data length in bytes.
    pub len: u32,
    /// Raw flags; bit 1 marks a directory.
    pub flags: u8,
    /// Decoded name (`.` and `..` kept as-is).
    pub name: String,
}

impl DirRecord {
    pub fn is_dir(&self) -> bool {
        self.flags & 0x02 != 0
    }
}

#[derive(Debug, Clone)]
pub struct PrimaryVolume {
    pub volume_id: String,
    pub volume_space_size: u32,
    pub logical_block_size: u16,
    pub root: DirRecord,
}

#[derive(Debug, Clone, Default)]
pub struct VolumeDescriptors {
    pub primary: Option<PrimaryVolume>,
    /// Root directory of the Joliet supplementary descriptor, when present.
    pub joliet: Option<DirRecord>,
    pub has_joliet: bool,
    pub has_rock_ridge: bool,
    pub has_udf: bool,
    pub el_torito_catalog: Option<u32>,
}

pub fn read_sector(file: &mut File, sector: u64) -> Result<[u8; SECTOR_SIZE as usize]> {
    let mut buf = [0u8; SECTOR_SIZE as usize];
    file.seek(SeekFrom::Start(sector * SECTOR_SIZE))?;
    file.read_exact(&mut buf).map_err(|err| {
        if err.kind() == std::io::ErrorKind::UnexpectedEof {
            IsoError::Truncated
        } else {
            IsoError::Io(err)
        }
    })?;
    Ok(buf)
}

/// Scan volume descriptors starting at sector 16, plus the UDF volume
/// recognition sequence which bridge images place right after them.
pub fn read_volume_descriptors(file: &mut File, size: u64) -> Result<VolumeDescriptors> {
    let mut out = VolumeDescriptors::default();
    let total_sectors = size / SECTOR_SIZE;
    if total_sectors < 17 {
        return Err(IsoError::Truncated);
    }

    let mut sector = 16u64;
    let max_descriptor_sector = total_sectors.min(16 + 64);
    while sector < max_descriptor_sector {
        let buf = read_sector(file, sector)?;
        match buf[0] {
            1 => {
                let volume_id = latin1_trim(&buf[40..72]);
                let volume_space_size = le_u32(&buf[80..84]);
                let logical_block_size = le_u16(&buf[128..130]);
                let root = parse_dir_record(&buf[156..], false)?;
                out.has_rock_ridge = has_rock_ridge(&buf[156..]);
                out.primary = Some(PrimaryVolume {
                    volume_id,
                    volume_space_size,
                    logical_block_size,
                    root,
                });
            }
            2 => {
                let escape = &buf[88..91];
                if is_joliet_escape(escape) {
                    out.joliet = Some(parse_dir_record(&buf[156..], true)?);
                    out.has_joliet = true;
                }
            }
            0 => {
                let identifier = &buf[7..39];
                if identifier.starts_with(b"EL TORITO SPECIFICATION") {
                    out.el_torito_catalog = Some(le_u32(&buf[71..75]));
                }
            }
            255 => break,
            _ => {}
        }
        sector += 1;
    }

    // UDF bridge images put the volume recognition sequence after the ISO 9660
    // descriptor area (typically sector 32). "NSR02"/"NSR03" is authoritative.
    let udf_scan_end = total_sectors.min(96);
    let mut s = 16u64;
    while s < udf_scan_end {
        let buf = read_sector(file, s)?;
        let id = &buf[1..6];
        if id == b"NSR02" || id == b"NSR03" {
            out.has_udf = true;
            break;
        }
        s += 1;
    }

    if out.primary.is_none() {
        return Err(IsoError::NotIso(
            "no ISO 9660 primary volume descriptor at sector 16".to_owned(),
        ));
    }
    Ok(out)
}

fn is_joliet_escape(escape: &[u8]) -> bool {
    escape.len() == 3
        && escape[0] == 0x25
        && escape[1] == 0x2F
        && matches!(escape[2], 0x40 | 0x43 | 0x45)
}

fn has_rock_ridge(record: &[u8]) -> bool {
    if record.is_empty() {
        return false;
    }
    let name_len = record.get(32).copied().unwrap_or(0) as usize;
    let start = (33 + name_len).next_multiple_of(2);
    let end = (record[0] as usize).min(record.len());
    if start >= end {
        return false;
    }
    record[start..end]
        .windows(2)
        .any(|w| w == b"SP" || w == b"RR" || w == b"ER")
}

pub fn parse_dir_record(bytes: &[u8], joliet: bool) -> Result<DirRecord> {
    if bytes.len() < 34 {
        return Err(IsoError::InvalidDirectoryRecord(0));
    }
    let len = bytes[0] as usize;
    if len < 34 || len > bytes.len() {
        return Err(IsoError::InvalidDirectoryRecord(len as u64));
    }
    let extent = le_u32(&bytes[2..6]);
    let data_len = le_u32(&bytes[10..14]);
    let flags = bytes[25];
    let name_len = bytes[32] as usize;
    if 33 + name_len > len {
        return Err(IsoError::InvalidDirectoryRecord(len as u64));
    }
    let raw_name = &bytes[33..33 + name_len];
    let name = decode_name(raw_name, joliet);
    Ok(DirRecord {
        extent,
        len: data_len,
        flags,
        name,
    })
}

fn decode_name(raw: &[u8], joliet: bool) -> String {
    if raw.len() == 1 && (raw[0] == 0x00 || raw[0] == 0x01) {
        return if raw[0] == 0x00 {
            ".".to_owned()
        } else {
            "..".to_owned()
        };
    }
    let mut name = if joliet && raw.len() >= 2 {
        let units: Vec<u16> = raw
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| u16::from_be_bytes(*c))
            .take_while(|&c| c != 0)
            .collect();
        String::from_utf16_lossy(&units)
    } else {
        latin1_trim(raw)
    };
    if let Some(pos) = name.find(';') {
        name.truncate(pos);
    }
    if name.ends_with('.') && !name.starts_with('.') {
        name.pop();
    }
    name.trim_end().to_owned()
}

fn latin1_trim(bytes: &[u8]) -> String {
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    String::from_utf8_lossy(&bytes[..end]).trim_end().to_owned()
}

/// Read the entries of a directory extent.
pub fn read_dir(
    file: &mut File,
    dir: &DirRecord,
    joliet: bool,
    max_entries: usize,
) -> Result<Vec<DirRecord>> {
    if !dir.is_dir() || dir.len == 0 || max_entries == 0 {
        return Ok(Vec::new());
    }
    let len = (dir.len as usize).min(MAX_DIR_BYTES);
    let mut data = vec![0u8; len];
    file.seek(SeekFrom::Start(dir.extent as u64 * SECTOR_SIZE))?;
    file.read_exact(&mut data).map_err(|err| {
        if err.kind() == std::io::ErrorKind::UnexpectedEof {
            IsoError::Truncated
        } else {
            IsoError::Io(err)
        }
    })?;

    let mut out = Vec::new();
    let mut pos = 0usize;
    while pos < data.len() && out.len() < max_entries {
        let record_len = data[pos] as usize;
        if record_len == 0 {
            pos = (pos / SECTOR_SIZE as usize + 1) * SECTOR_SIZE as usize;
            continue;
        }
        if record_len < 34 || pos + record_len > data.len() {
            break;
        }
        let record = &data[pos..pos + record_len];
        match parse_dir_record(record, joliet) {
            Ok(entry) => {
                if entry.name != "." && entry.name != ".." && !entry.name.is_empty() {
                    out.push(entry);
                }
            }
            Err(err) => tracing::debug!(%err, pos, "skipping invalid directory record"),
        }
        pos += record_len;
    }
    Ok(out)
}

/// A flattened directory entry produced by [`walk_tree`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WalkEntry {
    /// Path relative to the root, separated by `/` (e.g. `casper/vmlinuz`).
    pub path: String,
    /// Logical block address of the file or directory data.
    pub extent: u32,
    pub len: u32,
    pub is_dir: bool,
}

/// Recursively list a directory tree, parents before children.
///
/// The walker is iterative and bounded by `max_entries`, so malformed images
/// with directory loops cannot exhaust memory or the stack.
pub fn walk_tree(
    file: &mut File,
    root: &DirRecord,
    joliet: bool,
    max_entries: usize,
) -> Result<Vec<WalkEntry>> {
    let mut out = Vec::new();
    let mut visited = std::collections::HashSet::new();
    visited.insert(root.extent);
    let mut stack = vec![(root.clone(), String::new(), 0u32)];

    while let Some((dir, prefix, depth)) = stack.pop() {
        if depth > 32 || out.len() >= max_entries {
            continue;
        }
        for record in read_dir(file, &dir, joliet, max_entries)? {
            let path = if prefix.is_empty() {
                record.name.clone()
            } else {
                format!("{prefix}/{}", record.name)
            };
            if record.is_dir() {
                if visited.insert(record.extent) {
                    out.push(WalkEntry {
                        path: path.clone(),
                        extent: record.extent,
                        len: record.len,
                        is_dir: true,
                    });
                    stack.push((record, path, depth + 1));
                }
            } else {
                out.push(WalkEntry {
                    path,
                    extent: record.extent,
                    len: record.len,
                    is_dir: false,
                });
            }
            if out.len() >= max_entries {
                break;
            }
        }
    }
    Ok(out)
}

/// Read at most `max` bytes from a file extent.
pub fn read_file(file: &mut File, extent: u32, size: u32, max: usize) -> Result<Vec<u8>> {
    let len = (size as usize).min(max);
    if len == 0 {
        return Ok(Vec::new());
    }
    let mut data = vec![0u8; len];
    file.seek(SeekFrom::Start(extent as u64 * SECTOR_SIZE))?;
    file.read_exact(&mut data)?;
    Ok(data)
}

pub fn le_u16(bytes: &[u8]) -> u16 {
    u16::from_le_bytes([bytes[0], bytes[1]])
}

pub fn le_u32(bytes: &[u8]) -> u32 {
    u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_iso9660_names() {
        assert_eq!(decode_name(b"README.TXT;1", false), "README.TXT");
        assert_eq!(decode_name(b"BOOT;1", false), "BOOT");
        assert_eq!(decode_name(&[0x00], false), ".");
        assert_eq!(decode_name(&[0x01], false), "..");
    }

    #[test]
    fn decodes_joliet_names() {
        let name: Vec<u8> = "Café"
            .encode_utf16()
            .flat_map(|u| u.to_be_bytes())
            .collect();
        assert_eq!(decode_name(&name, true), "Café");
    }

    #[test]
    fn parses_minimal_record() {
        let mut record = vec![0u8; 34];
        record[0] = 34;
        record[2..6].copy_from_slice(&42u32.to_le_bytes());
        record[10..14].copy_from_slice(&512u32.to_le_bytes());
        record[25] = 0x02;
        record[32] = 1;
        record[33] = 0x00;
        let parsed = parse_dir_record(&record, false).unwrap();
        assert_eq!(parsed.extent, 42);
        assert_eq!(parsed.len, 512);
        assert!(parsed.is_dir());
    }
}
