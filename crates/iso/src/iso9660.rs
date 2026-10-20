use std::fs::File;
use std::io::{Read, Seek, SeekFrom};

use crate::{IsoError, Result};

pub const SECTOR_SIZE: u64 = 2048;
const MAX_DIR_BYTES: usize = 8 * 1024 * 1024;
const MAX_SUSP_BYTES: usize = 64 * 1024;

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
    /// Rock Ridge `SL` target, when this record is a symlink.
    pub symlink_target: Option<String>,
}

impl DirRecord {
    pub fn is_dir(&self) -> bool {
        self.flags & 0x02 != 0
    }

    pub fn is_symlink(&self) -> bool {
        self.symlink_target.is_some()
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
        .any(|w| w == b"SP" || w == b"RR" || w == b"ER" || w == b"SL")
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
    let symlink_target = parse_rock_ridge_symlink(&bytes[..len]);
    Ok(DirRecord {
        extent,
        len: data_len,
        flags,
        name,
        symlink_target,
    })
}

/// Parse Rock Ridge `SL` entries from the System Use area of a directory
/// record. FAT32 has no symlinks, so callers use this to skip them instead of
/// materializing empty files.
fn parse_rock_ridge_symlink(record: &[u8]) -> Option<String> {
    let start = susp_start(record)?;
    let mut target = String::new();
    parse_susp_sl_into(&record[start..], &mut target).then_some(target)
}

fn susp_start(record: &[u8]) -> Option<usize> {
    if record.len() < 34 {
        return None;
    }
    let name_len = record[32] as usize;
    if 33 + name_len > record.len() {
        return None;
    }
    Some((33 + name_len).next_multiple_of(2))
}

/// Parse a sequence of SUSP entries starting at `entries[0]`. Returns true if
/// at least one `SL` entry was found.
fn parse_susp_sl_into(entries: &[u8], target: &mut String) -> bool {
    let mut pos = 0usize;
    let mut found = false;
    while pos + 4 <= entries.len() {
        let signature = &entries[pos..pos + 2];
        let entry_len = entries[pos + 2] as usize;
        if entry_len < 4 || pos + entry_len > entries.len() {
            break;
        }
        if signature == b"SL" {
            found = true;
            append_sl_components(&entries[pos + 4..pos + entry_len], target);
        }
        pos += entry_len;
    }
    found
}

/// Locate a SUSP continuation area (`CE`) in a sequence of SUSP entries.
fn find_susp_ce(entries: &[u8]) -> Option<(u32, u32, u32)> {
    let mut pos = 0usize;
    while pos + 4 <= entries.len() {
        let signature = &entries[pos..pos + 2];
        let entry_len = entries[pos + 2] as usize;
        if entry_len < 4 || pos + entry_len > entries.len() {
            break;
        }
        if signature == b"CE" && entry_len >= 28 {
            let block = le_u32(&entries[pos + 4..pos + 8]);
            let offset = le_u32(&entries[pos + 12..pos + 16]);
            let length = le_u32(&entries[pos + 20..pos + 24]);
            return Some((block, offset, length));
        }
        pos += entry_len;
    }
    None
}

/// Read `SL` entries from `CE` continuation areas. Rock Ridge uses these for
/// metadata that does not fit in the directory record, including long symlink
/// targets. Invalid or missing continuation areas are ignored so one bad
/// record cannot abort the whole image.
fn read_continuation_symlink(file: &mut File, record: &[u8]) -> Option<String> {
    let start = susp_start(record)?;
    let mut ce = find_susp_ce(&record[start..]);
    let mut target = String::new();
    let mut found = false;
    let mut depth = 0u8;
    while let Some((block, offset, length)) = ce {
        if depth >= 4 || length == 0 || length as usize > MAX_SUSP_BYTES {
            break;
        }
        let mut data = vec![0u8; length as usize];
        let position = u64::from(block) * SECTOR_SIZE + u64::from(offset);
        if file.seek(SeekFrom::Start(position)).is_err() || file.read_exact(&mut data).is_err() {
            break;
        }
        if parse_susp_sl_into(&data, &mut target) {
            found = true;
        }
        ce = find_susp_ce(&data);
        depth += 1;
    }
    found.then_some(target)
}

fn parse_dir_record_with_continuation(
    file: &mut File,
    bytes: &[u8],
    joliet: bool,
) -> Result<DirRecord> {
    let mut entry = parse_dir_record(bytes, joliet)?;
    if let Some(extra) = read_continuation_symlink(file, &bytes[..bytes[0] as usize]) {
        if let Some(target) = entry.symlink_target.as_mut() {
            if !target.is_empty() && !target.ends_with('/') && !extra.starts_with('/') {
                target.push('/');
            }
            target.push_str(&extra);
        } else {
            entry.symlink_target = Some(extra);
        }
    }
    Ok(entry)
}

fn append_sl_components(data: &[u8], target: &mut String) {
    let mut pos = 1usize; // first byte is the SL flags
    while pos + 2 <= data.len() {
        let component_len = data[pos] as usize;
        if component_len < 2 || pos + component_len > data.len() {
            break;
        }
        let flags = data[pos + 1];
        let bytes = &data[pos + 2..pos + component_len];
        let component = match flags {
            0x01 => ".".to_owned(),
            0x02 => "..".to_owned(),
            0x03 | 0x04 => "/".to_owned(),
            _ => String::from_utf8_lossy(bytes).into_owned(),
        };
        if component == "/" {
            if !target.starts_with('/') {
                target.insert(0, '/');
            }
        } else {
            if !target.is_empty() && !target.ends_with('/') {
                target.push('/');
            }
            target.push_str(&component);
        }
        pos += component_len;
    }
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
        match parse_dir_record_with_continuation(file, record, joliet) {
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
    /// Rock Ridge symlink target, when the entry is a symlink.
    pub symlink_target: Option<String>,
}

impl WalkEntry {
    pub fn is_symlink(&self) -> bool {
        self.symlink_target.is_some()
    }
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
            let symlink = record.symlink_target.clone();
            if record.is_dir() && symlink.is_none() {
                if visited.insert(record.extent) {
                    out.push(WalkEntry {
                        path: path.clone(),
                        extent: record.extent,
                        len: record.len,
                        is_dir: true,
                        symlink_target: None,
                    });
                    stack.push((record, path, depth + 1));
                }
            } else {
                out.push(WalkEntry {
                    path,
                    extent: record.extent,
                    len: record.len,
                    is_dir: record.is_dir() && symlink.is_none(),
                    symlink_target: symlink,
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
        assert!(!parsed.is_symlink());
    }

    #[test]
    fn parses_rock_ridge_symlink() {
        let record = record_with_symlink(b"LINK", "../../target");
        let parsed = parse_dir_record(&record, false).unwrap();
        assert_eq!(parsed.name, "LINK");
        assert!(parsed.is_symlink());
        assert_eq!(parsed.symlink_target.as_deref(), Some("../../target"));
    }

    #[test]
    fn reads_symlink_from_susp_continuation_area() {
        use std::io::Write;

        let mut file = tempfile::tempfile().unwrap();
        file.set_len(2 * SECTOR_SIZE).unwrap();
        let record = record_with_ce_symlink(b"LINK", 1, 0, sl_entry("../target").len() as u32);
        file.write_all(&record).unwrap();
        file.seek(SeekFrom::Start(SECTOR_SIZE)).unwrap();
        file.write_all(&sl_entry("../target")).unwrap();

        let dir = DirRecord {
            extent: 0,
            len: SECTOR_SIZE as u32,
            flags: 0x02,
            name: ".".to_owned(),
            symlink_target: None,
        };
        let entries = read_dir(&mut file, &dir, false, 10).unwrap();
        assert_eq!(entries.len(), 1);
        assert!(entries[0].is_symlink());
        assert_eq!(entries[0].symlink_target.as_deref(), Some("../target"));
    }

    fn record_with_symlink(name: &[u8], target: &str) -> Vec<u8> {
        let mut record = base_record(name);
        record.extend_from_slice(&[b'S', b'P', 7, 1, 0xBE, 0xEF, 0]);
        record.extend_from_slice(&sl_entry(target));
        finish_record(record)
    }

    fn record_with_ce_symlink(name: &[u8], block: u32, offset: u32, len: u32) -> Vec<u8> {
        let mut record = base_record(name);
        record.extend_from_slice(&[b'S', b'P', 7, 1, 0xBE, 0xEF, 0]);
        let mut ce = vec![b'C', b'E', 28, 1];
        ce.extend_from_slice(&block.to_le_bytes());
        ce.extend_from_slice(&block.to_be_bytes());
        ce.extend_from_slice(&offset.to_le_bytes());
        ce.extend_from_slice(&offset.to_be_bytes());
        ce.extend_from_slice(&len.to_le_bytes());
        ce.extend_from_slice(&len.to_be_bytes());
        record.extend_from_slice(&ce);
        finish_record(record)
    }

    fn base_record(name: &[u8]) -> Vec<u8> {
        let name_len = name.len();
        let mut record = vec![0u8; (33 + name_len).next_multiple_of(2)];
        record[32] = name_len as u8;
        record[33..33 + name_len].copy_from_slice(name);
        record
    }

    fn sl_entry(target: &str) -> Vec<u8> {
        let mut sl = vec![b'S', b'L', 0, 1, 0];
        sl.push((target.len() + 2) as u8);
        sl.push(0);
        sl.extend_from_slice(target.as_bytes());
        sl[2] = sl.len() as u8;
        sl
    }

    fn finish_record(mut record: Vec<u8>) -> Vec<u8> {
        if !record.len().is_multiple_of(2) {
            record.push(0);
        }
        record[0] = record.len() as u8;
        record
    }
}
