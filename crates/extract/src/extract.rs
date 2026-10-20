//! Extraction pipeline: MBR + FAT32 + files copied from the ISO tree.
//!
//! UEFI boot works by copying the image's own `EFI/BOOT/BOOT*.EFI` payload.
//! Legacy BIOS bootloaders (syslinux/GRUB core images) are not bundled yet, so
//! extraction is currently UEFI-only; hybrid images can still be written with
//! DD mode.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::time::{Duration, Instant};

use fatfs::{FatType, FileSystem, FormatVolumeOptions, FsOptions};
use image2usb_core::{
    CancelToken, Error, FileSystem as FilesystemKind, PartitionScheme, Phase, ProgressEvent,
    ProgressSink, RawIo, Result, VerifyMode, WriteOptions, util::format_bytes,
};
use image2usb_iso::{IsoInfo, SECTOR_SIZE, read_volume_descriptors, walk_tree};
use sha2::{Digest, Sha256};

use crate::layout::{PartitionLayout, plan_layout, write_mbr};
use crate::names::NameMap;
use crate::partition::PartitionIo;

const MAX_ENTRIES: usize = 250_000;
const COPY_CHUNK: usize = 1024 * 1024;
const REPORT_INTERVAL: Duration = Duration::from_millis(100);

#[derive(Debug, Clone)]
pub struct ExtractStats {
    pub files: u64,
    pub directories: u64,
    pub bytes: u64,
    pub elapsed: Duration,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone)]
struct ManifestEntry {
    path: String,
    size: u64,
    sha256: Option<[u8; 32]>,
}

pub fn extract_image(
    iso_path: &Path,
    info: &IsoInfo,
    device: &mut dyn RawIo,
    options: &WriteOptions,
    cancel: &CancelToken,
    sink: &dyn ProgressSink,
) -> Result<ExtractStats> {
    if options.filesystem != FilesystemKind::Fat32 {
        return Err(Error::Unsupported(
            "only FAT32 extraction is implemented in this build".to_owned(),
        ));
    }
    if options.scheme != PartitionScheme::Mbr {
        return Err(Error::Unsupported(
            "GPT extraction is not implemented yet; use --scheme mbr".to_owned(),
        ));
    }
    if info.distro == image2usb_iso::Distro::Windows {
        return Err(windows_unsupported());
    }

    let started = Instant::now();
    let mut warnings = Vec::new();
    let image_len = std::fs::metadata(iso_path)?.len();
    let layout = plan_layout(device.size(), device.logical_sector_size())?;

    let mut iso = File::open(iso_path)?;
    let descriptors = read_volume_descriptors(&mut iso, image_len).map_err(iso_error)?;
    let root = descriptors.joliet.as_ref().unwrap_or(
        &descriptors
            .primary
            .as_ref()
            .ok_or_else(|| Error::InvalidImage("no primary volume descriptor".to_owned()))?
            .root,
    );
    let entries =
        walk_tree(&mut iso, root, descriptors.has_joliet, MAX_ENTRIES).map_err(iso_error)?;
    if entries.is_empty() {
        return Err(Error::InvalidImage(
            "image has no files to extract".to_owned(),
        ));
    }
    if is_windows_payload(&entries) {
        return Err(windows_unsupported());
    }
    if info.has_udf {
        warnings
            .push("image contains UDF; extraction reads the ISO 9660/Joliet tree only".to_owned());
    }
    let has_uefi = entries.iter().any(|entry| {
        let path = entry.path.to_ascii_lowercase();
        path.starts_with("efi/boot/boot") && path.ends_with(".efi")
    });
    if !has_uefi {
        warnings.push(
            "no EFI/BOOT/BOOT*.EFI found: the result will not boot on UEFI firmware, and legacy \
             BIOS bootloaders are not installed yet"
                .to_owned(),
        );
    }

    let total_bytes = entries
        .iter()
        .filter(|entry| !entry.is_dir)
        .map(|entry| entry.len as u64)
        .sum::<u64>();

    sink.report(ProgressEvent::message(
        Phase::Writing,
        format!(
            "creating MBR + FAT32 ({} partition)",
            format_bytes(layout.len)
        ),
    ));
    cancel.check()?;
    write_mbr(device, &layout)?;

    let mut part = PartitionIo::new(device, layout.start_byte, layout.len);
    format_fat32(&mut part, &layout, options, info)?;

    let (manifest, stats_dirs, copied_bytes) = {
        let fs =
            FileSystem::new(&mut part, FsOptions::new()).map_err(|err| fat_error("mount", err))?;
        let root_dir = fs.root_dir();
        let mut names = NameMap::new();
        let mut manifest = Vec::new();
        let mut directories = 0u64;
        let mut copied = 0u64;
        let mut last_report = started;
        let total_progress = total_bytes.max(1);

        for entry in &entries {
            cancel.check()?;
            let dest = names.resolve(&entry.path, entry.is_dir);
            if entry.is_dir {
                root_dir
                    .create_dir(&dest)
                    .map_err(|err| fat_error(&dest, err))?;
                directories += 1;
                continue;
            }

            let mut out = root_dir
                .create_file(&dest)
                .map_err(|err| fat_error(&dest, err))?;
            out.truncate().map_err(|err| fat_error(&dest, err))?;

            let mut hasher = (options.verify == VerifyMode::Sha256).then(Sha256::new);
            iso.seek(SeekFrom::Start(entry.extent as u64 * SECTOR_SIZE))?;
            let mut remaining = entry.len as u64;
            let mut buf = vec![0u8; COPY_CHUNK.min(remaining.max(1) as usize)];
            while remaining > 0 {
                cancel.check()?;
                let want = (remaining as usize).min(buf.len());
                iso.read_exact(&mut buf[..want])?;
                out.write_all(&buf[..want])
                    .map_err(|err| fat_error(&dest, err))?;
                if let Some(hasher) = hasher.as_mut() {
                    hasher.update(&buf[..want]);
                }
                remaining -= want as u64;
                copied += want as u64;

                let now = Instant::now();
                if now.duration_since(last_report) >= REPORT_INTERVAL {
                    sink.report(progress(
                        Phase::Writing,
                        copied,
                        total_progress,
                        started,
                        Some(&dest),
                    ));
                    last_report = now;
                }
            }
            out.flush().map_err(|err| fat_error(&dest, err))?;
            manifest.push(ManifestEntry {
                path: dest,
                size: entry.len as u64,
                sha256: hasher.map(|h| h.finalize().into()),
            });
        }
        sink.report(progress(
            Phase::Writing,
            total_bytes,
            total_progress,
            started,
            Some("files copied"),
        ));
        drop(root_dir);
        drop(fs);
        (manifest, directories, copied)
    };

    part.flush().map_err(Error::Io)?;
    crate::postprocess::finalize(&mut part).map_err(Error::Io)?;
    part.flush().map_err(Error::Io)?;
    drop(part);
    device.sync()?;

    if options.verify == VerifyMode::Sha256 {
        verify_manifest(device, &layout, &manifest, cancel, sink)?;
    }

    let elapsed = started.elapsed();
    Ok(ExtractStats {
        files: manifest.len() as u64,
        directories: stats_dirs,
        bytes: copied_bytes,
        elapsed,
        warnings,
    })
}

fn format_fat32(
    part: &mut PartitionIo<'_>,
    layout: &PartitionLayout,
    options: &WriteOptions,
    info: &IsoInfo,
) -> Result<()> {
    let total_512 = layout.len / 512;
    if total_512 > u32::MAX as u64 {
        return Err(Error::Unsupported(
            "partition larger than 2 TiB cannot be FAT32".to_owned(),
        ));
    }
    let label = if options.label.trim().is_empty() {
        info.volume_id.as_deref().unwrap_or("IMAGE2USB")
    } else {
        &options.label
    };
    let mut opts = FormatVolumeOptions::new()
        .fat_type(FatType::Fat32)
        .bytes_per_sector(512)
        .total_sectors(total_512 as u32)
        .fats(2)
        .volume_label(fat_label(label));
    if let Some(cluster) = options.cluster_size {
        let cluster = cluster.next_power_of_two();
        if !(512..=32 * 1024).contains(&cluster) {
            return Err(Error::Unsupported(format!(
                "cluster size must be between 512 and 32768 bytes, got {cluster}"
            )));
        }
        let clusters = layout.len / cluster as u64;
        if clusters < 65_525 {
            return Err(Error::Unsupported(format!(
                "cluster size {cluster} is too large for this device (would create {clusters} \
                 clusters, FAT32 needs at least 65525)"
            )));
        }
        opts = opts.bytes_per_cluster(cluster);
    }

    part.seek(SeekFrom::Start(0)).map_err(Error::Io)?;
    fatfs::format_volume(&mut *part, opts).map_err(|err| fat_error("format", err))?;
    part.flush().map_err(Error::Io)?;
    Ok(())
}

fn verify_manifest(
    device: &mut dyn RawIo,
    layout: &PartitionLayout,
    manifest: &[ManifestEntry],
    cancel: &CancelToken,
    sink: &dyn ProgressSink,
) -> Result<()> {
    let started = Instant::now();
    let total = manifest.iter().map(|entry| entry.size).sum::<u64>().max(1);
    let mut done = 0u64;
    let mut part = PartitionIo::new(device, layout.start_byte, layout.len);
    {
        let fs =
            FileSystem::new(&mut part, FsOptions::new()).map_err(|err| fat_error("verify", err))?;
        let root = fs.root_dir();
        for entry in manifest {
            cancel.check()?;
            let Some(expected) = entry.sha256 else {
                continue;
            };
            let mut file = root
                .open_file(&entry.path)
                .map_err(|err| fat_error(&entry.path, err))?;
            let size = file
                .seek(SeekFrom::End(0))
                .map_err(|err| fat_error(&entry.path, err))?;
            if size != entry.size {
                return Err(Error::ExtractedFileMismatch {
                    path: entry.path.clone(),
                    expected: entry.size,
                    actual: size,
                });
            }
            file.seek(SeekFrom::Start(0))
                .map_err(|err| fat_error(&entry.path, err))?;
            let mut hasher = Sha256::new();
            let mut buf = vec![0u8; COPY_CHUNK];
            loop {
                cancel.check()?;
                let n = file
                    .read(&mut buf)
                    .map_err(|err| fat_error(&entry.path, err))?;
                if n == 0 {
                    break;
                }
                hasher.update(&buf[..n]);
                done += n as u64;
                sink.report(progress(
                    Phase::Verifying,
                    done,
                    total,
                    started,
                    Some(&entry.path),
                ));
            }
            let actual: [u8; 32] = hasher.finalize().into();
            if actual != expected {
                return Err(Error::Other(format!(
                    "verification failed for {}: content hash differs",
                    entry.path
                )));
            }
        }
        drop(root);
        drop(fs);
    }
    part.flush().map_err(Error::Io)?;
    drop(part);
    device.sync()?;
    sink.report(progress(
        Phase::Verifying,
        total,
        total,
        started,
        Some("verified"),
    ));
    Ok(())
}

fn progress(
    phase: Phase,
    done: u64,
    total: u64,
    started: Instant,
    message: Option<&str>,
) -> ProgressEvent {
    let elapsed = started.elapsed().as_secs_f64().max(0.001);
    let bps = (done as f64 / elapsed) as u64;
    let eta = if done > 0 && total > done {
        Some(Duration::from_secs_f64(
            (total - done) as f64 / (done as f64 / elapsed),
        ))
    } else {
        Some(Duration::ZERO)
    };
    ProgressEvent {
        phase,
        done_bytes: done,
        total_bytes: total,
        bytes_per_sec: Some(bps),
        eta,
        message: message.map(str::to_owned),
    }
}

fn fat_label(label: &str) -> [u8; 11] {
    let mut out = [b' '; 11];
    let mut written = 0;
    for byte in label.bytes() {
        if byte.is_ascii_alphanumeric() || byte == b' ' || byte == b'-' || byte == b'_' {
            out[written] = byte.to_ascii_uppercase();
            written += 1;
            if written == 11 {
                break;
            }
        }
    }
    out
}

fn iso_error(err: image2usb_iso::IsoError) -> Error {
    Error::InvalidImage(err.to_string())
}

fn is_windows_payload(entries: &[image2usb_iso::WalkEntry]) -> bool {
    entries.iter().any(|entry| {
        let path = entry.path.to_ascii_lowercase();
        matches!(
            path.as_str(),
            "sources/install.wim" | "sources/install.esd" | "sources/install.swm"
        )
    })
}

fn windows_unsupported() -> Error {
    Error::Unsupported(
        "Windows installation media needs wimlib/split-WIM support, which is not implemented \
         yet; use --mode dd (UEFI boot only) or wait for the next phase"
            .to_owned(),
    )
}

fn fat_error(context: &str, err: std::io::Error) -> Error {
    Error::Other(format!("FAT32 {context}: {err}"))
}

/// Extract the file list without touching a device. Used by `--dry-run` and by
/// the GUI to show how many files an extraction will copy.
pub fn preview(iso_path: &Path) -> Result<(u64, u64, u64)> {
    let image_len = std::fs::metadata(iso_path)?.len();
    let mut iso = File::open(iso_path)?;
    let descriptors = read_volume_descriptors(&mut iso, image_len).map_err(iso_error)?;
    let root = descriptors.joliet.as_ref().unwrap_or(
        &descriptors
            .primary
            .as_ref()
            .ok_or_else(|| Error::InvalidImage("no primary volume descriptor".to_owned()))?
            .root,
    );
    let entries =
        walk_tree(&mut iso, root, descriptors.has_joliet, MAX_ENTRIES).map_err(iso_error)?;
    if is_windows_payload(&entries) {
        return Err(windows_unsupported());
    }
    let mut names = NameMap::new();
    let mut files = 0u64;
    let mut dirs = 0u64;
    let mut bytes = 0u64;
    for entry in &entries {
        names.resolve(&entry.path, entry.is_dir);
        if entry.is_dir {
            dirs += 1;
        } else {
            files += 1;
            bytes += entry.len as u64;
        }
    }
    Ok((files, dirs, bytes))
}

/// Helper used by tests to list extracted files without booting an OS.
pub fn list_extracted(
    device: &mut dyn RawIo,
    layout: &PartitionLayout,
) -> Result<Vec<(String, u64)>> {
    let mut part = PartitionIo::new(device, layout.start_byte, layout.len);
    let fs = FileSystem::new(&mut part, FsOptions::new()).map_err(|err| fat_error("mount", err))?;
    let mut out = Vec::new();
    let mut stack = vec![(fs.root_dir(), String::new())];
    while let Some((dir, prefix)) = stack.pop() {
        for entry in dir.iter() {
            let entry = entry.map_err(|err| fat_error(&prefix, err))?;
            let name = entry.file_name();
            let path = if prefix.is_empty() {
                name.clone()
            } else {
                format!("{prefix}/{name}")
            };
            let size = entry.len();
            if entry.is_dir() {
                out.push((path.clone(), 0));
                if let Ok(sub) = dir.open_dir(&name) {
                    stack.push((sub, path));
                }
            } else {
                out.push((path, size));
            }
        }
    }
    out.sort();
    Ok(out)
}

/// Read one extracted file back through the FAT32 filesystem. Used by tests
/// and by callers that want an independent content check.
pub fn read_extracted(
    device: &mut dyn RawIo,
    layout: &PartitionLayout,
    path: &str,
) -> Result<Vec<u8>> {
    let mut part = PartitionIo::new(device, layout.start_byte, layout.len);
    let fs = FileSystem::new(&mut part, FsOptions::new()).map_err(|err| fat_error("mount", err))?;
    let root = fs.root_dir();
    let mut file = root.open_file(path).map_err(|err| fat_error(path, err))?;
    let mut out = Vec::new();
    file.read_to_end(&mut out)
        .map_err(|err| fat_error(path, err))?;
    Ok(out)
}
