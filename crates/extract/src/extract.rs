//! Extraction pipeline: MBR or GPT + FAT32 + files copied from the ISO tree.
//!
//! UEFI boot works by copying the image's own `EFI/BOOT/BOOT*.EFI` payload.
//! For legacy BIOS, a small bundled GRUB core is installed in the gap before
//! the partition; it loads the distribution's own `boot/grub/grub.cfg` or
//! `boot/grub2/grub.cfg`. Images without either config keep UEFI boot only and
//! are expected to use DD mode when legacy BIOS is required.

use std::collections::HashMap;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::time::{Duration, Instant};

use fatfs::{FatType, FileSystem, FormatVolumeOptions, FsOptions, ReadWriteSeek};
use image2usb_core::{
    CancelToken, Error, FileSystem as FilesystemKind, PartitionScheme, Phase, ProgressEvent,
    ProgressSink, RawIo, Result, VerifyMode, WriteOptions, util::format_bytes,
};
use image2usb_iso::{IsoInfo, SECTOR_SIZE, WalkEntry, read_volume_descriptors, walk_tree};
use sha2::{Digest, Sha256};

use crate::gpt::{plan_gpt_layout, write_gpt};
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
    if info.distro == image2usb_iso::Distro::Windows {
        return Err(windows_unsupported());
    }

    let started = Instant::now();
    let mut warnings = Vec::new();
    let image_len = std::fs::metadata(iso_path)?.len();
    let (layout, gpt) = match options.scheme {
        PartitionScheme::Mbr => (
            plan_layout(device.size(), device.logical_sector_size())?,
            None,
        ),
        PartitionScheme::Gpt => {
            let gpt = plan_gpt_layout(device.size(), device.logical_sector_size())?;
            (gpt.partition, Some(gpt))
        }
    };

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
    let grub_config = crate::bios::grub_config(&entries);
    if !has_uefi {
        warnings.push(
            "no EFI/BOOT/BOOT*.EFI found: the result will not boot on UEFI firmware".to_owned(),
        );
    }
    if grub_config.is_none() {
        warnings.push(
            "no boot/grub/grub.cfg or boot/grub2/grub.cfg found: legacy BIOS boot will not be \
             installed; use DD mode if this image must boot on BIOS firmware"
                .to_owned(),
        );
    } else if !crate::bios::fits(&layout, options.scheme) {
        warnings.push(format!(
            "legacy BIOS boot will not be installed: {} requires 512-byte logical sectors \
             (4Kn/4096-byte media is UEFI-only) and a 1 MiB partition gap; UEFI boot remains \
             available, or use DD mode",
            grub_config
                .as_ref()
                .map(|entry| entry.path.as_str())
                .unwrap_or("the GRUB configuration")
        ));
    }

    let symlinks = entries
        .iter()
        .filter(|entry| entry.is_symlink())
        .collect::<Vec<_>>();
    if !symlinks.is_empty() {
        let examples = symlinks
            .iter()
            .take(5)
            .map(|entry| {
                format!(
                    "{} -> {}",
                    entry.path,
                    entry.symlink_target.as_deref().unwrap_or("?")
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        let suffix = if symlinks.len() > 5 { ", ..." } else { "" };
        warnings.push(format!(
            "skipped {} Rock Ridge symlink(s) because FAT32 has no symlinks: {examples}{suffix}",
            symlinks.len()
        ));
    }

    let total_bytes = entries
        .iter()
        .filter(|entry| !entry.is_dir && !entry.is_symlink())
        .map(|entry| entry.len as u64)
        .sum::<u64>();

    // Refuse before the MBR is touched if the tree cannot fit in the FAT32
    // data area. The formatter and the estimate use the same cluster rules.
    check_extraction_space(&entries, &layout, options)?;

    sink.report(ProgressEvent::message(
        Phase::Formatting,
        format!(
            "creating {} partition table + FAT32 ({} partition)",
            options.scheme.to_string().to_uppercase(),
            format_bytes(layout.len)
        ),
    ));
    cancel.check()?;
    match &gpt {
        Some(gpt) => write_gpt(device, gpt)?,
        None => write_mbr(device, &layout)?,
    }

    let mut part = PartitionIo::new(device, layout.start_byte, layout.len);
    format_fat32(&mut part, &layout, options, info)?;
    sink.report(ProgressEvent::message(Phase::Formatting, "FAT32 formatted"));
    sink.report(ProgressEvent::message(
        Phase::Copying,
        format!(
            "copying {} file(s)",
            entries
                .iter()
                .filter(|e| !e.is_dir && !e.is_symlink())
                .count()
        ),
    ));

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
            if entry.is_symlink() {
                continue;
            }
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
                        Phase::Copying,
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
            Phase::Copying,
            total_bytes,
            total_progress,
            started,
            Some("files copied"),
        ));
        let source_label = info.volume_id.as_deref().unwrap_or("IMAGE2USB");
        let target_label = String::from_utf8_lossy(&fat_label(extraction_label(options, info)))
            .trim_end()
            .to_owned();
        if !source_label.eq_ignore_ascii_case(&target_label) {
            let changed =
                rewrite_boot_config_labels(&root_dir, &manifest, source_label, &target_label)
                    .map_err(|err| fat_error("boot config", err))?;
            for path in changed {
                if let Some(entry) = manifest.iter_mut().find(|entry| entry.path == path) {
                    let mut file = root_dir
                        .open_file(&entry.path)
                        .map_err(|err| fat_error(&entry.path, err))?;
                    let mut data = Vec::new();
                    file.read_to_end(&mut data)
                        .map_err(|err| fat_error(&entry.path, err))?;
                    entry.size = data.len() as u64;
                    if options.verify == VerifyMode::Sha256 {
                        entry.sha256 = Some(Sha256::digest(&data).into());
                    }
                }
            }
        }
        drop(root_dir);
        drop(fs);
        (manifest, directories, copied)
    };

    part.flush().map_err(Error::Io)?;
    cancel.check()?;
    sink.report(ProgressEvent::message(
        Phase::Finalizing,
        "finalizing FAT32 metadata",
    ));
    crate::postprocess::finalize(&mut part, cancel)?;
    part.flush().map_err(Error::Io)?;
    cancel.check()?;
    drop(part);
    device.sync()?;

    if let Some(config) = grub_config
        && crate::bios::fits(&layout, options.scheme)
    {
        cancel.check()?;
        sink.report(ProgressEvent::message(
            Phase::Finalizing,
            format!("installing GRUB BIOS bootloader for {}", config.path),
        ));
        crate::bios::install(device, &layout, options.scheme)?;
    }

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

const PREFLIGHT_MARGIN_CLUSTERS: u64 = 8;

fn check_extraction_space(
    entries: &[WalkEntry],
    layout: &PartitionLayout,
    options: &WriteOptions,
) -> Result<()> {
    let cluster = resolve_cluster_size(layout, options)?;
    let required_clusters = required_data_clusters(entries, cluster);
    let available_clusters = available_data_clusters(layout.len, cluster);

    if required_clusters.saturating_add(PREFLIGHT_MARGIN_CLUSTERS) > available_clusters {
        return Err(Error::ExtractionTooLarge {
            required: required_clusters.saturating_mul(cluster),
            available: available_clusters.saturating_mul(cluster),
        });
    }
    Ok(())
}

fn resolve_cluster_size(layout: &PartitionLayout, options: &WriteOptions) -> Result<u64> {
    let Some(cluster) = options.cluster_size else {
        return Ok(default_cluster_size(layout.len));
    };
    let cluster = cluster.next_power_of_two();
    if !(512..=32 * 1024).contains(&cluster) {
        return Err(Error::Unsupported(format!(
            "cluster size must be between 512 and 32768 bytes, got {cluster}"
        )));
    }
    let clusters = layout.len / u64::from(cluster);
    if clusters < 65_525 {
        return Err(Error::Unsupported(format!(
            "cluster size {cluster} is too large for this device (would create {clusters} \
             clusters, FAT32 needs at least 65525)"
        )));
    }
    Ok(u64::from(cluster))
}

/// Mirrors `fatfs::FormatVolumeOptions` when no cluster size is requested:
/// 512 bytes up to 260 MiB, 4 KiB up to 8 GiB, then 1 KiB per 2 GiB capped at
/// 32 KiB.
fn default_cluster_size(total_bytes: u64) -> u64 {
    const MIB: u64 = 1024 * 1024;
    const GIB: u64 = 1024 * MIB;
    let cluster = if total_bytes <= 260 * MIB {
        512
    } else if total_bytes <= 8 * GIB {
        4096
    } else {
        total_bytes.next_power_of_two() / (2 * GIB) * 1024
    };
    cluster.clamp(512, 32 * 1024)
}

/// Number of data clusters available after the reserved sectors and the two
/// FATs. This follows the geometry calculation used by `fatfs`.
fn available_data_clusters(partition_len: u64, cluster: u64) -> u64 {
    let total_sectors = partition_len / 512;
    const RESERVED_SECTORS: u64 = 8;
    if total_sectors <= RESERVED_SECTORS {
        return 0;
    }
    let sectors_per_cluster = (cluster / 512).max(1);
    let t0 = total_sectors - RESERVED_SECTORS;
    let bits_per_cluster = sectors_per_cluster * 512 * 8;
    let denominator = bits_per_cluster / 32 + 2;
    let fat_sectors = (t0 + 2 * sectors_per_cluster).div_ceil(denominator);
    let data_sectors = total_sectors.saturating_sub(RESERVED_SECTORS + 2 * fat_sectors);
    data_sectors / sectors_per_cluster
}

/// Estimated data clusters for the file and directory contents. Directory
/// entry slots are deliberately overestimated (one short entry plus LFN
/// entries for every name), because `fatfs` may allocate LFN records for names
/// that do not fit 8.3.
fn required_data_clusters(entries: &[WalkEntry], cluster: u64) -> u64 {
    let cluster = cluster.max(512);
    let mut names = NameMap::new();
    let mut dir_slots: HashMap<String, u64> = HashMap::new();
    dir_slots.insert(String::new(), 0);
    let mut data_clusters = 0u64;

    for entry in entries {
        if entry.is_symlink() {
            continue;
        }
        let dest = names.resolve(&entry.path, entry.is_dir);
        let (parent, base) = dest
            .rsplit_once('/')
            .map_or(("", dest.as_str()), |(parent, base)| (parent, base));
        *dir_slots.entry(parent.to_owned()).or_insert(0) += entry_slots(base);
        if entry.is_dir {
            // Subdirectories contain `.` and `..`.
            dir_slots.entry(dest).or_insert(2);
        } else {
            data_clusters = data_clusters.saturating_add(u64::from(entry.len).div_ceil(cluster));
        }
    }

    let dir_clusters = dir_slots
        .values()
        .map(|slots| (slots * 32).div_ceil(cluster).max(1))
        .sum::<u64>();
    data_clusters.saturating_add(dir_clusters)
}

fn entry_slots(name: &str) -> u64 {
    let utf16_units = name.encode_utf16().count() as u64;
    1 + utf16_units.div_ceil(13)
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
    let label = extraction_label(options, info);
    let mut opts = FormatVolumeOptions::new()
        .fat_type(FatType::Fat32)
        .bytes_per_sector(512)
        .total_sectors(total_512 as u32)
        .fats(2)
        .volume_label(fat_label(label));
    if options.cluster_size.is_some() {
        let cluster = resolve_cluster_size(layout, options)? as u32;
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

fn extraction_label<'a>(options: &'a WriteOptions, info: &'a IsoInfo) -> &'a str {
    if options.label.trim().is_empty() {
        info.volume_id.as_deref().unwrap_or("IMAGE2USB")
    } else {
        &options.label
    }
}

/// Rewrite bootloader configuration files that refer to the ISO volume ID.
/// FAT32 volume labels are limited to 11 characters, so a long ISO label such
/// as Fedora's is stored truncated. GRUB and the installer both search for the
/// original label unless the config is updated to the actual FAT label.
fn rewrite_boot_config_labels<T: ReadWriteSeek>(
    root: &fatfs::Dir<'_, T>,
    manifest: &[ManifestEntry],
    source: &str,
    target: &str,
) -> io::Result<Vec<String>> {
    if source.is_empty() || source.eq_ignore_ascii_case(target) {
        return Ok(Vec::new());
    }
    let mut changed = Vec::new();
    for entry in manifest.iter().filter(|entry| is_boot_config(&entry.path)) {
        if entry.size > 2 * 1024 * 1024 {
            continue;
        }
        let mut file = root.open_file(&entry.path)?;
        let mut data = Vec::new();
        file.read_to_end(&mut data)?;
        let Ok(text) = String::from_utf8(data) else {
            continue;
        };
        let replaced = replace_ascii_case_insensitive(&text, source, target);
        if replaced == text {
            continue;
        }
        file.seek(SeekFrom::Start(0))?;
        file.truncate()?;
        file.write_all(replaced.as_bytes())?;
        file.flush()?;
        changed.push(entry.path.clone());
    }
    Ok(changed)
}

fn is_boot_config(path: &str) -> bool {
    let path = path.to_ascii_lowercase();
    matches!(
        path.as_str(),
        "efi/boot/grub.cfg"
            | "efi/boot/boot.conf"
            | "boot/grub/grub.cfg"
            | "boot/grub2/grub.cfg"
            | "boot/grub/loopback.cfg"
            | "isolinux/isolinux.cfg"
            | "syslinux/syslinux.cfg"
            | "isolinux.cfg"
            | "syslinux.cfg"
    ) || (path.starts_with("loader/entries/") && path.ends_with(".conf"))
        || (path.starts_with("boot/loader/entries/") && path.ends_with(".conf"))
}

fn replace_ascii_case_insensitive(text: &str, needle: &str, replacement: &str) -> String {
    let haystack = text.as_bytes();
    let needle = needle.as_bytes();
    if needle.is_empty() {
        return text.to_owned();
    }
    let mut out = String::with_capacity(text.len());
    let mut index = 0;
    while index < haystack.len() {
        if index + needle.len() <= haystack.len()
            && haystack[index..index + needle.len()].eq_ignore_ascii_case(needle)
        {
            out.push_str(replacement);
            index += needle.len();
        } else {
            let ch = text[index..].chars().next().expect("valid char boundary");
            out.push(ch);
            index += ch.len_utf8();
        }
    }
    out
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
        if entry.is_symlink() {
            continue;
        }
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
    let mut stack = vec![(fs.root_dir(), String::new(), 0u32)];
    while let Some((dir, prefix, depth)) = stack.pop() {
        if depth > 64 {
            continue;
        }
        for entry in dir.iter() {
            let entry = entry.map_err(|err| fat_error(&prefix, err))?;
            let name = entry.file_name();
            // `fatfs` exposes the dot entries of subdirectories; following
            // them would recurse forever (and `..` walks back up the tree).
            if name == "." || name == ".." {
                continue;
            }
            let path = if prefix.is_empty() {
                name.clone()
            } else {
                format!("{prefix}/{name}")
            };
            let size = entry.len();
            if entry.is_dir() {
                out.push((path.clone(), 0));
                if let Ok(sub) = dir.open_dir(&name) {
                    stack.push((sub, path, depth + 1));
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_cluster_size_matches_fatfs_thresholds() {
        assert_eq!(default_cluster_size(128 * 1024 * 1024), 512);
        assert_eq!(default_cluster_size(4 * 1024 * 1024 * 1024), 4096);
        assert!(default_cluster_size(16 * 1024 * 1024 * 1024) > 4096);
    }

    #[test]
    fn preflight_rejects_a_tree_that_does_not_fit() {
        let layout = plan_layout(96 * 1024 * 1024, 512).unwrap();
        let entries = vec![WalkEntry {
            path: "BIG.BIN".to_owned(),
            extent: 0,
            len: 200 * 1024 * 1024,
            is_dir: false,
            symlink_target: None,
        }];
        let err = check_extraction_space(&entries, &layout, &WriteOptions::default()).unwrap_err();
        assert!(matches!(err, Error::ExtractionTooLarge { .. }), "{err}");
    }

    #[test]
    fn rewrites_long_iso_labels_case_insensitively() {
        let text = "search -l 'Fedora-E-dvd-x86_64-44'\nlinux /vmlinuz inst.stage2=hd:LABEL=fedora-e-dvd-x86_64-44";
        let rewritten =
            replace_ascii_case_insensitive(text, "Fedora-E-dvd-x86_64-44", "FEDORA-E-DV");
        assert!(!rewritten.contains("dvd-x86_64"));
        assert_eq!(rewritten.matches("FEDORA-E-DV").count(), 2);
    }

    #[test]
    fn recognizes_bootloader_configs() {
        assert!(is_boot_config("EFI/BOOT/grub.cfg"));
        assert!(is_boot_config("boot/grub2/grub.cfg"));
        assert!(is_boot_config("isolinux/isolinux.cfg"));
        assert!(is_boot_config("loader/entries/archiso-x86_64.conf"));
        assert!(!is_boot_config("etc/fstab.conf"));
    }

    #[test]
    fn symlinks_do_not_consume_extraction_space() {
        let layout = plan_layout(96 * 1024 * 1024, 512).unwrap();
        let entries = vec![WalkEntry {
            path: "LINK".to_owned(),
            extent: 0,
            len: u32::MAX,
            is_dir: false,
            symlink_target: Some("BIG.BIN".to_owned()),
        }];
        check_extraction_space(&entries, &layout, &WriteOptions::default()).unwrap();
    }
}
