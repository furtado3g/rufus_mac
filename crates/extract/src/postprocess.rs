//! Post-processing for filesystems created by `fatfs`.
//!
//! The `fatfs` crate produces a valid, mountable FAT32 volume, but it writes
//! long-file-name entries for the `.`/`..` entries of every directory and it
//! does not maintain the FAT32 FSInfo free-cluster count. Real firmware and
//! `fsck` implementations expect `.` and `..` to be the first two directory
//! entries, and macOS `fsck_msdos` rejects the volume otherwise.
//!
//! This pass rewrites each non-root directory so that `.` and `..` come first,
//! drops the unnecessary LFN entries, fixes the `..` parent cluster, and writes
//! the free-cluster count to the FSInfo sector (and its backup).

use std::collections::HashSet;
use std::io::{self, Read, Seek, SeekFrom, Write};

use image2usb_core::{CancelToken, Error, Result as CoreResult};

use crate::partition::PartitionIo;

const DOT: &[u8; 11] = b".          ";
const DOTDOT: &[u8; 11] = b"..         ";
const END: u8 = 0x00;
const DELETED: u8 = 0xE5;
const ATTR_VOLUME: u8 = 0x08;
const ATTR_DIRECTORY: u8 = 0x10;
const ATTR_LFN: u8 = 0x0F;
const MAX_DIR_BYTES: u64 = 64 * 1024 * 1024;

struct Geometry {
    bytes_per_sector: u64,
    sectors_per_cluster: u64,
    reserved_sectors: u64,
    num_fats: u64,
    total_sectors: u64,
    fat_sectors: u64,
    root_cluster: u32,
    fsinfo_sector: u16,
    backup_sector: u16,
}

impl Geometry {
    fn cluster_size(&self) -> usize {
        (self.sectors_per_cluster * self.bytes_per_sector) as usize
    }

    fn fat_offset(&self) -> u64 {
        self.reserved_sectors * self.bytes_per_sector
    }

    fn data_start(&self) -> u64 {
        self.reserved_sectors + self.num_fats * self.fat_sectors
    }

    fn cluster_count(&self) -> u64 {
        (self.total_sectors - self.data_start()) / self.sectors_per_cluster
    }

    fn cluster_offset(&self, cluster: u32) -> u64 {
        (self.data_start() + (cluster as u64 - 2) * self.sectors_per_cluster)
            * self.bytes_per_sector
    }
}

pub fn finalize(disk: &mut PartitionIo<'_>, cancel: &CancelToken) -> CoreResult<()> {
    cancel.check()?;
    let geometry = read_geometry(disk)?;
    let mut visited = HashSet::new();
    fix_directory(
        disk,
        &geometry,
        geometry.root_cluster,
        0,
        true,
        true,
        &mut visited,
        cancel,
    )?;
    cancel.check()?;
    update_fsinfo(disk, &geometry, cancel)?;
    Ok(())
}

fn read_geometry(disk: &mut PartitionIo<'_>) -> CoreResult<Geometry> {
    let mut bpb = [0u8; 512];
    read_at(disk, 0, &mut bpb)?;
    let bytes_per_sector = u16::from_le_bytes([bpb[11], bpb[12]]) as u64;
    let sectors_per_cluster = bpb[13] as u64;
    let reserved_sectors = u16::from_le_bytes([bpb[14], bpb[15]]) as u64;
    let num_fats = bpb[16] as u64;
    let total_sectors = u32::from_le_bytes([bpb[32], bpb[33], bpb[34], bpb[35]]) as u64;
    let fat_sectors = u32::from_le_bytes([bpb[36], bpb[37], bpb[38], bpb[39]]) as u64;
    let root_cluster = u32::from_le_bytes([bpb[44], bpb[45], bpb[46], bpb[47]]);
    let fsinfo_sector = u16::from_le_bytes([bpb[48], bpb[49]]);
    let backup_sector = u16::from_le_bytes([bpb[50], bpb[51]]);

    let invalid = || io::Error::new(io::ErrorKind::InvalidData, "invalid FAT32 geometry");
    if !(512..=4096).contains(&bytes_per_sector)
        || !bytes_per_sector.is_power_of_two()
        || sectors_per_cluster == 0
        || !sectors_per_cluster.is_power_of_two()
        || fat_sectors == 0
        || total_sectors == 0
        || root_cluster < 2
    {
        return Err(invalid().into());
    }
    let data_start = reserved_sectors + num_fats * fat_sectors;
    if data_start >= total_sectors {
        return Err(invalid().into());
    }
    Ok(Geometry {
        bytes_per_sector,
        sectors_per_cluster,
        reserved_sectors,
        num_fats,
        total_sectors,
        fat_sectors,
        root_cluster,
        fsinfo_sector,
        backup_sector,
    })
}

#[allow(clippy::too_many_arguments)]
fn fix_directory(
    disk: &mut PartitionIo<'_>,
    geometry: &Geometry,
    cluster: u32,
    parent: u32,
    parent_is_root: bool,
    is_root: bool,
    visited: &mut HashSet<u32>,
    cancel: &CancelToken,
) -> CoreResult<()> {
    cancel.check()?;
    if !visited.insert(cluster) {
        return Ok(());
    }
    let clusters = chain(disk, geometry, cluster, cancel)?;
    let cluster_size = geometry.cluster_size();
    let total = clusters.len() * cluster_size;
    if total as u64 > MAX_DIR_BYTES {
        return Err(Error::Io(io::Error::new(
            io::ErrorKind::InvalidData,
            "directory is implausibly large",
        )));
    }
    let mut data = vec![0u8; total];
    for (index, &current) in clusters.iter().enumerate() {
        cancel.check()?;
        let offset = index * cluster_size;
        read_at(
            disk,
            geometry.cluster_offset(current),
            &mut data[offset..offset + cluster_size],
        )?;
    }

    let mut entries = parse_entries(&data);
    if is_root {
        recurse_children(disk, geometry, &entries, cluster, visited, cancel)?;
        return Ok(());
    }

    let dot = entries
        .iter()
        .find(|entry| entry[0] != END && is_dir(entry) && &entry[0..11] == DOT)
        .copied();
    let dotdot = entries
        .iter()
        .find(|entry| entry[0] != END && is_dir(entry) && &entry[0..11] == DOTDOT)
        .copied();
    let (Some(mut dot), Some(mut dotdot)) = (dot, dotdot) else {
        // Not created by this extractor (or already finalized): leave it alone.
        return Ok(());
    };

    set_cluster(&mut dot, cluster);
    set_cluster(&mut dotdot, if parent_is_root { 0 } else { parent });

    // Drop the long-name records that `fatfs` emitted for `.` and `..`, as
    // well as the original dot entries; both are re-added at the front.
    let mut drop = vec![false; entries.len()];
    for index in 0..entries.len() {
        if entries[index][0] == END {
            break;
        }
        if is_dir(&entries[index])
            && (&entries[index][0..11] == DOT || &entries[index][0..11] == DOTDOT)
        {
            let mut previous = index;
            while previous > 0 {
                let candidate = entries[previous - 1];
                if candidate[11] == ATTR_LFN && candidate[0] != DELETED {
                    drop[previous - 1] = true;
                    previous -= 1;
                } else {
                    break;
                }
            }
        }
    }

    let mut rebuilt = Vec::with_capacity(entries.len());
    rebuilt.push(dot);
    rebuilt.push(dotdot);
    for (index, entry) in entries.iter().enumerate() {
        if drop[index] || entry[0] == DELETED {
            continue;
        }
        if is_dir(entry) && (&entry[0..11] == DOT || &entry[0..11] == DOTDOT) {
            continue;
        }
        if entry[0] == END {
            break;
        }
        rebuilt.push(*entry);
    }
    entries = rebuilt;

    let mut output = vec![0u8; total];
    for (index, entry) in entries.iter().enumerate() {
        let offset = index * 32;
        if offset + 32 > output.len() {
            break;
        }
        output[offset..offset + 32].copy_from_slice(entry);
    }
    for (index, &current) in clusters.iter().enumerate() {
        let offset = index * cluster_size;
        write_at(
            disk,
            geometry.cluster_offset(current),
            &output[offset..offset + cluster_size],
        )?;
    }

    recurse_children(disk, geometry, &entries, cluster, visited, cancel)
}

fn recurse_children(
    disk: &mut PartitionIo<'_>,
    geometry: &Geometry,
    entries: &[[u8; 32]],
    parent: u32,
    visited: &mut HashSet<u32>,
    cancel: &CancelToken,
) -> CoreResult<()> {
    for entry in entries {
        cancel.check()?;
        if entry[0] == END || entry[0] == DELETED || entry[0] == 0 {
            continue;
        }
        if entry[11] & ATTR_VOLUME != 0 || !is_dir(entry) {
            continue;
        }
        if &entry[0..11] == DOT || &entry[0..11] == DOTDOT {
            continue;
        }
        let child = entry_cluster(entry);
        if child >= 2 {
            fix_directory(
                disk,
                geometry,
                child,
                parent,
                parent == geometry.root_cluster,
                false,
                visited,
                cancel,
            )?;
        }
    }
    Ok(())
}

fn parse_entries(data: &[u8]) -> Vec<[u8; 32]> {
    data.as_chunks::<32>().0.to_vec()
}

fn is_dir(entry: &[u8; 32]) -> bool {
    entry[11] & ATTR_DIRECTORY != 0
}

fn entry_cluster(entry: &[u8; 32]) -> u32 {
    let low = u16::from_le_bytes([entry[26], entry[27]]) as u32;
    let high = u16::from_le_bytes([entry[20], entry[21]]) as u32;
    (high << 16) | low
}

fn set_cluster(entry: &mut [u8; 32], cluster: u32) {
    entry[26..28].copy_from_slice(&(cluster as u16).to_le_bytes());
    entry[20..22].copy_from_slice(&((cluster >> 16) as u16).to_le_bytes());
}

fn chain(
    disk: &mut PartitionIo<'_>,
    geometry: &Geometry,
    start: u32,
    cancel: &CancelToken,
) -> CoreResult<Vec<u32>> {
    let mut out = Vec::new();
    let mut current = start;
    let max = (geometry.cluster_count() + 2).min(u32::MAX as u64) as u32;
    let mut buffer = [0u8; 4];
    while (2..max).contains(&current) {
        cancel.check()?;
        if out.len() as u64 > max as u64 {
            return Err(Error::Io(io::Error::new(
                io::ErrorKind::InvalidData,
                "FAT32 cluster chain loops",
            )));
        }
        out.push(current);
        read_at(
            disk,
            geometry.fat_offset() + current as u64 * 4,
            &mut buffer,
        )?;
        let next = u32::from_le_bytes(buffer) & 0x0FFF_FFFF;
        if !(2..0x0FFF_FFF8).contains(&next) {
            break;
        }
        current = next;
    }
    Ok(out)
}

fn update_fsinfo(
    disk: &mut PartitionIo<'_>,
    geometry: &Geometry,
    cancel: &CancelToken,
) -> CoreResult<()> {
    let count = geometry.cluster_count();
    let mut free = 0u32;
    let mut next_free = 0xFFFF_FFFFu32;
    let mut buffer = vec![0u8; 256 * 1024];
    let mut cluster = 2u64;
    while cluster < count + 2 {
        cancel.check()?;
        let entries = ((count + 2 - cluster) as usize).min(buffer.len() / 4);
        let offset = geometry.fat_offset() + cluster * 4;
        let bytes = entries * 4;
        read_at(disk, offset, &mut buffer[..bytes])?;
        for index in 0..entries {
            let value = u32::from_le_bytes([
                buffer[index * 4],
                buffer[index * 4 + 1],
                buffer[index * 4 + 2],
                buffer[index * 4 + 3],
            ]);
            if value == 0 {
                free += 1;
                if next_free == 0xFFFF_FFFF {
                    next_free = (cluster + index as u64) as u32;
                }
            }
        }
        cluster += entries as u64;
    }

    let mut sectors = vec![geometry.fsinfo_sector];
    if geometry.backup_sector != 0 {
        let backup = geometry.backup_sector as u64 + geometry.fsinfo_sector as u64;
        if backup < geometry.reserved_sectors {
            sectors.push(backup as u16);
        }
    }
    for sector in sectors {
        let offset = sector as u64 * geometry.bytes_per_sector;
        let mut fsinfo = [0u8; 512];
        read_at(disk, offset, &mut fsinfo)?;
        let lead = u32::from_le_bytes([fsinfo[0], fsinfo[1], fsinfo[2], fsinfo[3]]);
        let structure = u32::from_le_bytes([fsinfo[484], fsinfo[485], fsinfo[486], fsinfo[487]]);
        if lead != 0x4161_5252 || structure != 0x6141_7272 {
            continue;
        }
        fsinfo[488..492].copy_from_slice(&free.to_le_bytes());
        fsinfo[492..496].copy_from_slice(&next_free.to_le_bytes());
        write_at(disk, offset, &fsinfo)?;
    }
    Ok(())
}

fn read_at<T: Read + Seek>(disk: &mut T, offset: u64, buffer: &mut [u8]) -> io::Result<()> {
    disk.seek(SeekFrom::Start(offset))?;
    disk.read_exact(buffer)
}

fn write_at<T: Write + Seek>(disk: &mut T, offset: u64, buffer: &[u8]) -> io::Result<()> {
    disk.seek(SeekFrom::Start(offset))?;
    disk.write_all(buffer)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fatfs::{FileSystem, FormatVolumeOptions, FsOptions};
    use image2usb_core::{CancelToken, FileDevice};
    use std::io::{Seek, SeekFrom};

    #[test]
    fn finalize_stops_when_cancelled() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("volume.img");
        std::fs::write(&path, vec![0u8; 64 * 1024 * 1024]).unwrap();
        let mut device = FileDevice::open(&path).unwrap();
        let mut part = PartitionIo::new(&mut device, 0, 64 * 1024 * 1024);
        let cancel = CancelToken::new();
        cancel.cancel();
        let err = finalize(&mut part, &cancel).unwrap_err();
        assert!(matches!(err, Error::Cancelled));
    }

    #[test]
    fn rewrites_dot_entries_and_fsinfo() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("volume.img");
        std::fs::write(&path, vec![0u8; 64 * 1024 * 1024]).unwrap();
        let mut device = FileDevice::open(&path).unwrap();
        {
            let mut part = PartitionIo::new(&mut device, 0, 64 * 1024 * 1024);
            let options = FormatVolumeOptions::new()
                .fat_type(fatfs::FatType::Fat32)
                .bytes_per_sector(512)
                .total_sectors((64 * 1024 * 1024 / 512) as u32)
                .fats(2);
            fatfs::format_volume(&mut part, options).unwrap();
            let fs = FileSystem::new(&mut part, FsOptions::new()).unwrap();
            fs.root_dir().create_dir("a").unwrap();
            fs.root_dir().create_dir("a/b").unwrap();
            drop(fs);
            part.flush().unwrap();
            finalize(&mut part, &CancelToken::new()).unwrap();
            part.flush().unwrap();
            // Mount again: the rewritten directories must still be readable.
            part.seek(SeekFrom::Start(0)).unwrap();
            let fs = FileSystem::new(&mut part, FsOptions::new()).unwrap();
            assert!(fs.root_dir().open_dir("a").unwrap().open_dir("b").is_ok());
            drop(fs);
            part.flush().unwrap();

            let mut first = [0u8; 512];
            let mut bpb = [0u8; 512];
            read_at(&mut part, 0, &mut bpb).unwrap();
            let root_cluster = u32::from_le_bytes([bpb[44], bpb[45], bpb[46], bpb[47]]);
            let reserved = u16::from_le_bytes([bpb[14], bpb[15]]) as u64;
            let fats = bpb[16] as u64;
            let fat_size = u32::from_le_bytes([bpb[36], bpb[37], bpb[38], bpb[39]]) as u64;
            let data_start = reserved + fats * fat_size;
            read_at(
                &mut part,
                (data_start + (root_cluster as u64 - 2)) * 512,
                &mut first,
            )
            .unwrap();
            // Root entry 0 is the volume label or the first directory; find
            // the first real directory and check its first two entries.
            let mut child = None;
            for entry in first.as_chunks::<32>().0 {
                if entry[0] == 0
                    || entry[0] == DELETED
                    || entry[11] == ATTR_LFN
                    || entry[11] & ATTR_VOLUME != 0
                    || entry[11] & ATTR_DIRECTORY == 0
                {
                    continue;
                }
                if &entry[0..11] == DOT || &entry[0..11] == DOTDOT {
                    continue;
                }
                child = Some(entry_cluster(entry));
                break;
            }
            let child = child.expect("directory a");
            let mut dir_data = [0u8; 64];
            read_at(
                &mut part,
                (data_start + (child as u64 - 2)) * 512,
                &mut dir_data,
            )
            .unwrap();
            assert_eq!(&dir_data[0..11], DOT);
            assert_eq!(&dir_data[32..43], DOTDOT);
            let dot_entry: [u8; 32] = dir_data[0..32].try_into().unwrap();
            let dotdot_entry: [u8; 32] = dir_data[32..64].try_into().unwrap();
            assert_eq!(entry_cluster(&dot_entry), child);
            assert_eq!(entry_cluster(&dotdot_entry), 0);

            let fsinfo_sector = u16::from_le_bytes([bpb[48], bpb[49]]) as u64;
            let mut fsinfo = [0u8; 512];
            read_at(&mut part, fsinfo_sector * 512, &mut fsinfo).unwrap();
            let free = u32::from_le_bytes([fsinfo[488], fsinfo[489], fsinfo[490], fsinfo[491]]);
            assert_ne!(free, 0xFFFF_FFFF);
            assert_ne!(free, 0);
        }
    }
}
