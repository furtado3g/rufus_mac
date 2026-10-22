//! Windows installation-media preparation.
//!
//! Preparation never receives a writable device. It captures a private
//! snapshot of the ISO, validates the Windows boot chain against that snapshot,
//! and returns the only bytes the commit path is allowed to copy. The snapshot
//! stabilizes the bytes for one operation; it does not authenticate the image
//! or detect every adversarial modification made during capture.

use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use image2usb_core::{
    CancelToken, Error, FileSystem as FilesystemKind, FileSystemChoice, PartitionScheme, Phase,
    ProgressEvent, ProgressSink, RawIo, ResolvedMode, Result, VerifyMode, WriteOptions,
};
use image2usb_iso::{
    IsoInfo, SECTOR_SIZE, WalkEntry, WindowsArchitecture, WindowsInfo, analyze_path,
    read_volume_descriptors, walk_tree,
};
use sha2::{Digest, Sha256};

use crate::extract::{
    ExtractStats, ManifestEntry, check_extraction_space, format_fat32, verify_manifest,
};
use crate::gpt::plan_gpt_layout;
use crate::layout::{plan_layout, write_mbr};
use crate::names::NameMap;
use crate::partition::PartitionIo;
#[cfg(all(feature = "windows-media", target_os = "macos"))]
use crate::windows_layout::{
    UEFI_NTFS_IMAGE, WindowsScheme, plan_windows_layout, write_windows_layout,
};
#[cfg(all(feature = "windows-media", target_os = "macos"))]
use image2usb_platform::media::{NtfsFormatOptions, NtfsVolume, format_ntfs};

const MAX_ENTRIES: usize = 250_000;
const COPY_CHUNK: usize = 1024 * 1024;
const FAT32_MAX_FILE: u64 = 4_294_967_295;
const REPORT_INTERVAL: Duration = Duration::from_millis(100);

/// Resolve the user's filesystem selection without changing an explicit
/// choice silently. Shared by CLI and GUI so both frontends show the same plan.
pub fn resolve_filesystem(
    requested: FileSystemChoice,
    mode: ResolvedMode,
    info: &IsoInfo,
) -> Result<(FilesystemKind, String)> {
    if mode == ResolvedMode::Dd {
        if requested != FileSystemChoice::Auto {
            return Err(Error::Unsupported(
                "--filesystem applies only to extraction mode; DD writes the image 1:1".to_owned(),
            ));
        }
        return Ok((
            FilesystemKind::Fat32,
            "DD mode does not create a filesystem".to_owned(),
        ));
    }
    if info.distro != image2usb_iso::Distro::Windows {
        return match requested {
            FileSystemChoice::Auto | FileSystemChoice::Fat32 => Ok((
                FilesystemKind::Fat32,
                "non-Windows extraction uses FAT32".to_owned(),
            )),
            FileSystemChoice::Ntfs => Err(Error::Unsupported(
                "NTFS extraction is currently supported only for Windows media".to_owned(),
            )),
        };
    }

    let largest = info
        .windows
        .as_ref()
        .map(|windows| {
            windows
                .install_parts
                .iter()
                .map(|part| part.size)
                .max()
                .unwrap_or(0)
        })
        .unwrap_or(0);
    let over_fat32 = largest > FAT32_MAX_FILE;
    match requested {
        FileSystemChoice::Fat32 if over_fat32 => Err(Error::Unsupported(format!(
            "Windows installation image is {} bytes; FAT32 cannot store files larger than {} \
             bytes. Choose NTFS/UEFI:NTFS or use split-capable media",
            largest, FAT32_MAX_FILE
        ))),
        FileSystemChoice::Fat32 => Ok((
            FilesystemKind::Fat32,
            "FAT32 selected manually; Windows media fits the FAT32 file limit".to_owned(),
        )),
        FileSystemChoice::Auto if over_fat32 => Ok((
            FilesystemKind::Ntfs,
            format!(
                "automatic selection: installation image is {} bytes and exceeds the FAT32 file \
                 limit; NTFS data plus UEFI:NTFS ESP will be used",
                largest
            ),
        )),
        FileSystemChoice::Auto => Ok((
            FilesystemKind::Fat32,
            "automatic selection: Windows media fits FAT32".to_owned(),
        )),
        FileSystemChoice::Ntfs => Ok((FilesystemKind::Ntfs, "NTFS selected manually".to_owned())),
    }
}

/// Fully prepared Windows source. Dropping it removes the private snapshot.
#[derive(Debug)]
pub struct PreparedWindows {
    _workspace: tempfile::TempDir,
    snapshot_path: PathBuf,
    pub info: IsoInfo,
    pub windows: WindowsInfo,
    pub catalog: Vec<WalkEntry>,
    pub source_sha256: [u8; 32],
    pub warnings: Vec<String>,
}

impl PreparedWindows {
    pub fn open_snapshot(&self) -> Result<File> {
        Ok(File::open(&self.snapshot_path)?)
    }

    pub fn snapshot_path(&self) -> &Path {
        &self.snapshot_path
    }

    pub fn scratch_bytes(&self) -> u64 {
        self.info.size
    }
}

/// Capture the ISO, analyze the snapshot and validate a Windows boot chain.
///
/// This function performs no destination I/O. Every error, cancellation or
/// ENOSPC while capturing only removes the operation's own workspace.
pub fn prepare_windows(
    iso_path: &Path,
    cancel: &CancelToken,
    sink: &dyn ProgressSink,
) -> Result<PreparedWindows> {
    cancel.check()?;
    let source = File::open(iso_path)?;
    let path_metadata_before = std::fs::metadata(iso_path)?;
    let metadata_before = source.metadata()?;
    if !metadata_before.is_file() {
        return Err(Error::Unsupported(
            "Windows source must be a regular file".to_owned(),
        ));
    }
    let image_len = metadata_before.len();
    if image_len < 17 * SECTOR_SIZE {
        return Err(Error::InvalidImage(
            "image is truncated or too small".to_owned(),
        ));
    }

    let workspace = tempfile::Builder::new()
        .prefix("image2usb-windows-")
        .tempdir()?;
    secure_workspace(workspace.path())?;
    let snapshot_path = workspace.path().join("source.iso");
    let mut snapshot = secure_create(&snapshot_path)?;

    sink.report(prepare_progress(
        0,
        image_len,
        "capturing private Windows media snapshot",
    ));
    let source_sha256 = capture_snapshot(&source, &mut snapshot, image_len, cancel, sink)?;
    snapshot.sync_all()?;
    drop(snapshot);

    let metadata_after = source.metadata()?;
    let path_metadata_after = std::fs::metadata(iso_path)?;
    compare_metadata(&metadata_before, &metadata_after)?;
    compare_metadata(&path_metadata_before, &path_metadata_after)?;

    let info = analyze_path(&snapshot_path)
        .map_err(|err| Error::InvalidImage(format!("snapshot analysis failed: {err}")))?;
    if info.distro != image2usb_iso::Distro::Windows {
        return Err(Error::InvalidImage(
            "source is not recognized as Windows installation media".to_owned(),
        ));
    }
    let windows = info.windows.clone().ok_or_else(|| {
        Error::InvalidImage("Windows media has no installation metadata".to_owned())
    })?;
    let snapshot_len = std::fs::metadata(&snapshot_path)?.len();
    if snapshot_len != image_len {
        return Err(Error::InvalidImage(
            "private snapshot length differs from the source".to_owned(),
        ));
    }
    let catalog = read_catalog(&snapshot_path, snapshot_len)?;
    validate_catalog(&catalog, &windows)?;
    validate_windows_metadata(&windows)?;
    let mut warnings = info.warnings.clone();
    warnings.push(
        "Windows extraction is UEFI-only; legacy BIOS/BOOTMGR boot is not installed".to_owned(),
    );
    warnings.push(
        "the private ISO snapshot stabilizes the bytes copied by this operation; it does not \
         authenticate the source image"
            .to_owned(),
    );

    Ok(PreparedWindows {
        _workspace: workspace,
        snapshot_path,
        info,
        windows,
        catalog,
        source_sha256,
        warnings,
    })
}

fn validate_windows_metadata(windows: &WindowsInfo) -> Result<()> {
    if !windows.bootmgr || !windows.bcd {
        return Err(Error::InvalidImage(
            "Windows boot chain is incomplete: BOOTMGR and boot/BCD are required".to_owned(),
        ));
    }
    let fallback = windows.efi_fallback.as_deref().ok_or_else(|| {
        Error::InvalidImage(
            "Windows media has no EFI/BOOT/BOOT<arch>.EFI removable-media fallback".to_owned(),
        )
    })?;
    if windows.install_parts.is_empty() {
        return Err(Error::InvalidImage(
            "Windows media has no installation image".to_owned(),
        ));
    }
    if windows.install_format.as_deref() == Some("swm") {
        validate_swm(&windows.install_parts)?;
    }
    let monolithic = windows
        .install_parts
        .iter()
        .filter(|part| part.name == "sources/install.wim" || part.name == "sources/install.esd")
        .count();
    if monolithic > 1 {
        return Err(Error::InvalidImage(
            "Windows media contains both install.wim and install.esd; refusing an ambiguous set"
                .to_owned(),
        ));
    }
    if windows.install_format.as_deref() == Some("swm") && monolithic > 0 {
        return Err(Error::InvalidImage(
            "Windows media contains both install.wim/install.esd and install.swm parts".to_owned(),
        ));
    }

    let expected = match windows.pe_architecture {
        WindowsArchitecture::X64 => "efi/boot/bootx64.efi",
        WindowsArchitecture::Arm64 => "efi/boot/bootaa64.efi",
        WindowsArchitecture::Unknown => {
            return Err(Error::InvalidImage(
                "Windows EFI fallback has no recognized PE machine; refusing to write".to_owned(),
            ));
        }
    };
    if !fallback.eq_ignore_ascii_case(expected) {
        return Err(Error::InvalidImage(format!(
            "Windows EFI fallback {fallback} does not match the PE architecture {}",
            windows.pe_architecture.label()
        )));
    }
    if windows.architecture != windows.pe_architecture {
        return Err(Error::InvalidImage(format!(
            "Windows architecture {} conflicts with the EFI PE machine {}",
            windows.architecture.label(),
            windows.pe_architecture.label()
        )));
    }
    Ok(())
}

fn validate_swm(parts: &[image2usb_iso::WindowsPart]) -> Result<()> {
    let mut parsed = Vec::new();
    for part in parts {
        let Some(rest) = part.name.strip_prefix("sources/install") else {
            continue;
        };
        let Some(number) = rest.strip_suffix(".swm") else {
            continue;
        };
        let number = if number.is_empty() {
            1
        } else {
            number
                .parse::<u64>()
                .map_err(|_| Error::InvalidImage(format!("invalid SWM part name: {}", part.name)))?
        };
        if part.size == 0 {
            return Err(Error::InvalidImage(format!(
                "SWM part is empty: {}",
                part.name
            )));
        }
        parsed.push(number);
    }
    parsed.sort_unstable();
    parsed.dedup();
    let expected = (1..=parsed.len() as u64).collect::<Vec<_>>();
    if parsed != expected {
        return Err(Error::InvalidImage(format!(
            "SWM set is incomplete or has duplicate/gapped parts: {parsed:?}"
        )));
    }
    Ok(())
}

fn read_catalog(path: &Path, image_size: u64) -> Result<Vec<WalkEntry>> {
    let mut file = File::open(path)?;
    let descriptors = read_volume_descriptors(&mut file, image_size).map_err(iso_error)?;
    let root = descriptors
        .joliet
        .as_ref()
        .or(descriptors.primary.as_ref().map(|volume| &volume.root))
        .ok_or_else(|| Error::InvalidImage("no ISO 9660 root directory".to_owned()))?;
    walk_tree(
        &mut file,
        root,
        descriptors.has_joliet,
        MAX_ENTRIES,
        image_size,
    )
    .map_err(iso_error)
}

fn validate_catalog(catalog: &[WalkEntry], windows: &WindowsInfo) -> Result<()> {
    if catalog.is_empty() {
        return Err(Error::InvalidImage(
            "Windows image has no files to extract".to_owned(),
        ));
    }
    let mut seen = std::collections::HashMap::<String, &str>::new();
    for entry in catalog {
        if entry.is_symlink() {
            return Err(Error::InvalidImage(format!(
                "Windows media contains a symlink: {}",
                entry.path
            )));
        }
        let normalized = entry.path.to_lowercase();
        if let Some(previous) = seen.insert(normalized, &entry.path) {
            return Err(Error::InvalidImage(format!(
                "Windows paths are ambiguous by case: {} and {}",
                previous, entry.path
            )));
        }
        if let Some(part) = windows
            .install_parts
            .iter()
            .find(|part| part.name.eq_ignore_ascii_case(&entry.path))
            && part.size != entry.len
        {
            return Err(Error::InvalidImage(format!(
                "installation image size changed between analysis and catalog: {}",
                entry.path
            )));
        }
    }
    let fallback = windows
        .efi_fallback
        .as_deref()
        .ok_or_else(|| Error::InvalidImage("Windows EFI fallback is missing".to_owned()))?;
    if !catalog
        .iter()
        .any(|entry| entry.path.eq_ignore_ascii_case(fallback) && !entry.is_dir)
    {
        return Err(Error::InvalidImage(format!(
            "Windows EFI fallback is missing from the catalog: {fallback}"
        )));
    }
    for required in ["bootmgr", "boot/bcd"] {
        if !catalog
            .iter()
            .any(|entry| entry.path.eq_ignore_ascii_case(required) && !entry.is_dir)
        {
            return Err(Error::InvalidImage(format!(
                "Windows boot file is missing: {required}"
            )));
        }
    }
    Ok(())
}

fn capture_snapshot(
    source: &File,
    snapshot: &mut File,
    image_len: u64,
    cancel: &CancelToken,
    sink: &dyn ProgressSink,
) -> Result<[u8; 32]> {
    let mut source = source;
    snapshot.set_len(0)?;
    let mut hasher = Sha256::new();
    let mut copied = 0u64;
    let mut buffer = vec![0u8; COPY_CHUNK];
    let started = Instant::now();
    let mut last_report = started;
    while copied < image_len {
        cancel.check()?;
        let want = (image_len - copied).min(buffer.len() as u64) as usize;
        let read = source.read(&mut buffer[..want])?;
        if read == 0 {
            return Err(Error::InvalidImage(
                "source ended while capturing the Windows snapshot".to_owned(),
            ));
        }
        snapshot
            .write_all(&buffer[..read])
            .map_err(map_snapshot_write_error)?;
        hasher.update(&buffer[..read]);
        copied += read as u64;
        let now = Instant::now();
        if now.duration_since(last_report) >= REPORT_INTERVAL {
            sink.report(prepare_progress(
                copied,
                image_len,
                "capturing private Windows media snapshot",
            ));
            last_report = now;
        }
    }
    if source.metadata()?.len() < image_len {
        return Err(Error::InvalidImage(
            "source changed while capturing the Windows snapshot".to_owned(),
        ));
    }
    Ok(hasher.finalize().into())
}

fn map_snapshot_write_error(error: std::io::Error) -> Error {
    if error.raw_os_error() == Some(28) {
        Error::Other(
            "not enough temporary space for the private Windows snapshot; free disk space and \
             retry"
                .to_owned(),
        )
    } else {
        Error::Io(error)
    }
}

fn prepare_progress(done: u64, total: u64, message: &str) -> ProgressEvent {
    ProgressEvent {
        phase: Phase::Preparing,
        done_bytes: done,
        total_bytes: total,
        bytes_per_sec: None,
        eta: None,
        message: Some(message.to_owned()),
        target_incomplete: false,
    }
}

fn compare_metadata(before: &std::fs::Metadata, after: &std::fs::Metadata) -> Result<()> {
    if before.len() != after.len() || before.modified().ok() != after.modified().ok() {
        return Err(Error::Other(
            "source metadata changed while capturing the Windows snapshot; analyze it again"
                .to_owned(),
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if before.dev() != after.dev()
            || before.ino() != after.ino()
            || before.mtime() != after.mtime()
            || before.mtime_nsec() != after.mtime_nsec()
            || before.ctime() != after.ctime()
            || before.ctime_nsec() != after.ctime_nsec()
        {
            return Err(Error::Other(
                "source identity or timestamp changed while capturing the Windows snapshot; \
                 analyze it again"
                    .to_owned(),
            ));
        }
    }
    Ok(())
}

#[cfg(unix)]
fn secure_workspace(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    Ok(())
}

#[cfg(not(unix))]
fn secure_workspace(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(unix)]
fn secure_create(path: &Path) -> Result<File> {
    use std::os::unix::fs::OpenOptionsExt;
    Ok(OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?)
}

#[cfg(not(unix))]
fn secure_create(path: &Path) -> Result<File> {
    Ok(OpenOptions::new().write(true).create_new(true).open(path)?)
}

/// Commit a prepared Windows image to a FAT32 device.
pub fn extract_windows_fat32(
    prepared: &PreparedWindows,
    device: &mut dyn RawIo,
    options: &WriteOptions,
    cancel: &CancelToken,
    sink: &dyn ProgressSink,
) -> Result<ExtractStats> {
    if options.filesystem.resolved() != Some(FilesystemKind::Fat32) {
        return Err(Error::Unsupported(
            "only FAT32 Windows extraction is implemented in this build".to_owned(),
        ));
    }
    let started = Instant::now();
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
    for entry in &prepared.catalog {
        if !entry.is_dir && entry.len > FAT32_MAX_FILE {
            return Err(Error::Unsupported(format!(
                "{} is {} bytes and exceeds the FAT32 file limit of {} bytes; choose NTFS with \
                 the UEFI:NTFS path instead",
                entry.path, entry.len, FAT32_MAX_FILE
            )));
        }
    }
    check_extraction_space(&prepared.catalog, &layout, options)?;

    sink.report(ProgressEvent::message(
        Phase::Formatting,
        format!(
            "creating {} partition table + FAT32 ({} partition)",
            options.scheme.to_string().to_uppercase(),
            image2usb_core::util::format_bytes(layout.len)
        ),
    ));
    cancel.check()?;
    match &gpt {
        Some(gpt) => crate::gpt::write_gpt(device, gpt)?,
        None => write_mbr(device, &layout)?,
    }
    let mut part = PartitionIo::new(device, layout.start_byte, layout.len);
    format_fat32(&mut part, &layout, options, &prepared.info)?;
    sink.report(ProgressEvent::message(Phase::Formatting, "FAT32 formatted"));
    sink.report(ProgressEvent::message(
        Phase::Copying,
        format!(
            "copying {} file(s) from the private snapshot",
            prepared
                .catalog
                .iter()
                .filter(|entry| !entry.is_dir)
                .count()
        ),
    ));

    let total_bytes = prepared
        .catalog
        .iter()
        .filter(|entry| !entry.is_dir)
        .try_fold(0u64, |total, entry| total.checked_add(entry.len))
        .ok_or_else(|| Error::InvalidImage("Windows file sizes overflow".to_owned()))?;
    let mut iso = prepared.open_snapshot()?;
    let (manifest, directories, copied) = {
        let fs = fatfs::FileSystem::new(&mut part, fatfs::FsOptions::new())
            .map_err(|err| fat_error("mount", err))?;
        let root = fs.root_dir();
        let mut names = NameMap::new();
        let mut manifest = Vec::new();
        let mut directories = 0u64;
        let mut copied = 0u64;
        let mut last_report = started;
        for entry in &prepared.catalog {
            cancel.check()?;
            if entry.is_symlink() {
                return Err(Error::InvalidImage(format!(
                    "unexpected symlink in Windows catalog: {}",
                    entry.path
                )));
            }
            let dest = names.resolve(&entry.path, entry.is_dir);
            if entry.is_dir {
                root.create_dir(&dest)
                    .map_err(|err| fat_error(&dest, err))?;
                directories += 1;
                continue;
            }
            let mut out = root
                .create_file(&dest)
                .map_err(|err| fat_error(&dest, err))?;
            out.truncate().map_err(|err| fat_error(&dest, err))?;
            let mut hasher = (options.verify == VerifyMode::Sha256).then(Sha256::new);
            let mut entry_copied = 0u64;
            for extent in entry.source_extents() {
                use std::io::{Seek, SeekFrom};
                iso.seek(SeekFrom::Start(extent.offset))?;
                let mut remaining = extent.len;
                let mut buffer = vec![0u8; COPY_CHUNK.min(remaining.max(1) as usize)];
                while remaining > 0 {
                    cancel.check()?;
                    let want = (remaining as usize).min(buffer.len());
                    iso.read_exact(&mut buffer[..want])?;
                    out.write_all(&buffer[..want])
                        .map_err(|err| fat_error(&dest, err))?;
                    if let Some(hasher) = hasher.as_mut() {
                        hasher.update(&buffer[..want]);
                    }
                    remaining -= want as u64;
                    copied += want as u64;
                    entry_copied += want as u64;
                    let now = Instant::now();
                    if now.duration_since(last_report) >= REPORT_INTERVAL {
                        sink.report(copy_progress(copied, total_bytes, &dest, started));
                        last_report = now;
                    }
                }
            }
            if entry_copied != entry.len {
                return Err(Error::InvalidImage(format!(
                    "extent lengths for {} do not add up to its declared size",
                    entry.path
                )));
            }
            out.flush().map_err(|err| fat_error(&dest, err))?;
            manifest.push(ManifestEntry {
                path: dest,
                size: entry.len,
                sha256: hasher.map(|hasher| hasher.finalize().into()),
            });
        }
        sink.report(copy_progress(
            total_bytes,
            total_bytes,
            "files copied",
            started,
        ));
        drop(root);
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

    if options.verify == VerifyMode::Sha256 {
        verify_manifest(device, &layout, &manifest, cancel, sink)?;
    }

    let mut warnings = prepared.warnings.clone();
    warnings.push(format!(
        "snapshot SHA-256: {}",
        hex::encode(prepared.source_sha256)
    ));
    let elapsed = started.elapsed();
    Ok(ExtractStats {
        files: manifest.len() as u64,
        directories,
        bytes: copied,
        elapsed,
        warnings,
    })
}

/// Commit a prepared Windows image to an NTFS data partition plus a FAT
/// partition containing the pinned UEFI:NTFS chainloader.
///
/// This entry point exists in every build so callers get a deterministic
/// "not available" error, but the native implementation is compiled only when
/// the `windows-media` feature is enabled on macOS.
pub fn extract_windows_ntfs(
    prepared: &PreparedWindows,
    device: &mut dyn RawIo,
    options: &WriteOptions,
    cancel: &CancelToken,
    sink: &dyn ProgressSink,
) -> Result<ExtractStats> {
    #[cfg(all(feature = "windows-media", target_os = "macos"))]
    {
        extract_windows_ntfs_impl(prepared, device, options, cancel, sink)
    }
    #[cfg(not(all(feature = "windows-media", target_os = "macos")))]
    {
        let _ = (prepared, device, options, cancel, sink);
        Err(Error::Unsupported(
            "NTFS/UEFI:NTFS Windows extraction is not available in this build; use a build with \
             the windows-media feature on macOS"
                .to_owned(),
        ))
    }
}

#[cfg(all(feature = "windows-media", target_os = "macos"))]
fn extract_windows_ntfs_impl(
    prepared: &PreparedWindows,
    device: &mut dyn RawIo,
    options: &WriteOptions,
    cancel: &CancelToken,
    sink: &dyn ProgressSink,
) -> Result<ExtractStats> {
    if options.filesystem.resolved() != Some(FilesystemKind::Ntfs) {
        return Err(Error::Unsupported(
            "NTFS Windows extraction requires the NTFS filesystem choice".to_owned(),
        ));
    }
    cancel.check()?;
    let scheme = match options.scheme {
        PartitionScheme::Mbr => WindowsScheme::Mbr,
        PartitionScheme::Gpt => WindowsScheme::Gpt,
    };
    let layout = plan_windows_layout(device.size(), device.logical_sector_size(), scheme)?;
    let total_bytes = prepared
        .catalog
        .iter()
        .filter(|entry| !entry.is_dir)
        .try_fold(0u64, |total, entry| total.checked_add(entry.len))
        .ok_or_else(|| Error::InvalidImage("Windows file sizes overflow".to_owned()))?;
    validate_ntfs_catalog(&prepared.catalog)?;
    check_ntfs_space(total_bytes, layout.data.len)?;

    sink.report(ProgressEvent::message(
        Phase::Formatting,
        format!(
            "creating {} partition table + NTFS data + UEFI:NTFS ESP ({} data)",
            options.scheme.to_string().to_uppercase(),
            image2usb_core::util::format_bytes(layout.data.len)
        ),
    ));
    cancel.check()?;
    write_windows_layout(device, &layout)?;

    let started = Instant::now();
    let label = ntfs_label(options, &prepared.info);
    let mut part = PartitionIo::new(device, layout.data.start_byte, layout.data.len);
    format_ntfs(
        &mut part,
        &NtfsFormatOptions::new(&label, layout.data.start_lba),
    )?;
    part.flush().map_err(Error::Io)?;
    sink.report(ProgressEvent::message(Phase::Formatting, "NTFS formatted"));

    let mut iso = prepared.open_snapshot()?;
    let mut manifest = Vec::new();
    let mut directories = 0u64;
    let mut copied = 0u64;
    let mut last_report = started;
    let mut directories_catalog: Vec<&WalkEntry> = prepared
        .catalog
        .iter()
        .filter(|entry| entry.is_dir)
        .collect();
    directories_catalog.sort_by_key(|entry| entry.path.matches('/').count());
    {
        let mut volume = NtfsVolume::mount(&mut part)?;
        sink.report(ProgressEvent::message(
            Phase::Copying,
            format!(
                "copying {} file(s) to NTFS from the private snapshot",
                prepared
                    .catalog
                    .iter()
                    .filter(|entry| !entry.is_dir)
                    .count()
            ),
        ));
        for entry in directories_catalog {
            cancel.check()?;
            volume.mkdir(&entry.path)?;
            directories += 1;
        }
        for entry in prepared.catalog.iter().filter(|entry| !entry.is_dir) {
            cancel.check()?;
            if entry.is_symlink() {
                return Err(Error::InvalidImage(format!(
                    "unexpected symlink in Windows catalog: {}",
                    entry.path
                )));
            }
            let mut out = volume.create_file(&entry.path)?;
            out.truncate(0)?;
            let mut hasher = (options.verify == VerifyMode::Sha256).then(Sha256::new);
            let mut entry_copied = 0u64;
            for extent in entry.source_extents() {
                use std::io::{Seek, SeekFrom};
                iso.seek(SeekFrom::Start(extent.offset))?;
                let mut remaining = extent.len;
                let mut buffer = vec![0u8; COPY_CHUNK.min(remaining.max(1) as usize)];
                while remaining > 0 {
                    cancel.check()?;
                    let want = (remaining as usize).min(buffer.len());
                    iso.read_exact(&mut buffer[..want])?;
                    out.write_at(entry_copied, &buffer[..want])?;
                    if let Some(hasher) = hasher.as_mut() {
                        hasher.update(&buffer[..want]);
                    }
                    remaining -= want as u64;
                    copied += want as u64;
                    entry_copied += want as u64;
                    let now = Instant::now();
                    if now.duration_since(last_report) >= REPORT_INTERVAL {
                        sink.report(copy_progress(copied, total_bytes, &entry.path, started));
                        last_report = now;
                    }
                }
            }
            if entry_copied != entry.len {
                return Err(Error::InvalidImage(format!(
                    "extent lengths for {} do not add up to its declared size",
                    entry.path
                )));
            }
            out.close()?;
            manifest.push(ManifestEntry {
                path: entry.path.clone(),
                size: entry.len,
                sha256: hasher.map(|hasher| hasher.finalize().into()),
            });
        }
        sink.report(copy_progress(
            total_bytes,
            total_bytes,
            "files copied",
            started,
        ));
        volume.sync()?;
        volume.unmount()?;
    }
    part.flush().map_err(Error::Io)?;
    drop(part);
    cancel.check()?;

    sink.report(ProgressEvent::message(
        Phase::Finalizing,
        "writing pinned UEFI:NTFS chainloader to its FAT partition",
    ));
    device.write_all_at(layout.esp.start_byte, UEFI_NTFS_IMAGE)?;
    device.sync()?;
    verify_uefi_ntfs_asset(device, &layout.esp)?;

    if options.verify == VerifyMode::Sha256 {
        let mut verify_part = PartitionIo::new(device, layout.data.start_byte, layout.data.len);
        verify_windows_ntfs(&manifest, &mut verify_part, cancel, sink)?;
        verify_part.flush().map_err(Error::Io)?;
        drop(verify_part);
        device.sync()?;
    }

    let mut warnings = prepared.warnings.clone();
    warnings.push(format!(
        "snapshot SHA-256: {}",
        hex::encode(prepared.source_sha256)
    ));
    warnings.push(
        "NTFS media boots through the UEFI:NTFS chainloader; Secure Boot/DB/DBX acceptance and \
         4Kn behavior are not validated in this build"
            .to_owned(),
    );
    let elapsed = started.elapsed();
    Ok(ExtractStats {
        files: manifest.len() as u64,
        directories,
        bytes: copied,
        elapsed,
        warnings,
    })
}

#[cfg(all(feature = "windows-media", target_os = "macos"))]
fn validate_ntfs_catalog(catalog: &[WalkEntry]) -> Result<()> {
    for entry in catalog {
        for component in entry.path.split('/') {
            if component.is_empty()
                || component == "."
                || component == ".."
                || component.ends_with([' ', '.'])
                || component.encode_utf16().count() > 255
                || component
                    .bytes()
                    .any(|byte| byte < 0x20 || byte == 0x7f || b"<>:\"|?*\\".contains(&byte))
            {
                return Err(Error::InvalidImage(format!(
                    "Windows path is not representable on NTFS: {}",
                    entry.path
                )));
            }
        }
    }
    Ok(())
}

#[cfg(all(feature = "windows-media", target_os = "macos"))]
fn check_ntfs_space(total_bytes: u64, partition_bytes: u64) -> Result<()> {
    // NTFS reserves space for its MFT and metadata. Leave a conservative 12.5%
    // plus 64 MiB so a nearly full target fails before the partition table is
    // touched rather than midway through the installation tree.
    let overhead = (total_bytes / 8).max(64 * 1024 * 1024);
    let required = total_bytes
        .checked_add(overhead)
        .ok_or_else(|| Error::Unsupported("Windows media size overflow".to_owned()))?;
    if partition_bytes < required {
        return Err(Error::Unsupported(format!(
            "NTFS data partition is {} bytes but the Windows tree plus NTFS metadata needs \
             approximately {} bytes",
            image2usb_core::util::format_bytes(partition_bytes),
            image2usb_core::util::format_bytes(required)
        )));
    }
    Ok(())
}

#[cfg(all(feature = "windows-media", target_os = "macos"))]
fn ntfs_label(options: &WriteOptions, info: &IsoInfo) -> String {
    let raw = if options.label.trim().is_empty() {
        info.volume_id.as_deref().unwrap_or("WINDOWS")
    } else {
        &options.label
    };
    let mut label = raw
        .chars()
        .map(|character| {
            if character.is_ascii_graphic() || character == ' ' {
                character
            } else {
                ' '
            }
        })
        .collect::<String>();
    label = label.trim().to_owned();
    if label.is_empty() {
        label = "WINDOWS".to_owned();
    }
    label.chars().take(32).collect()
}

#[cfg(all(feature = "windows-media", target_os = "macos"))]
fn verify_uefi_ntfs_asset(
    device: &mut dyn RawIo,
    esp: &crate::layout::PartitionLayout,
) -> Result<()> {
    if esp.len < UEFI_NTFS_IMAGE.len() as u64 {
        return Err(Error::Other(
            "UEFI:NTFS partition is smaller than the embedded image".to_owned(),
        ));
    }
    let mut offset = 0usize;
    let mut buffer = vec![0u8; 64 * 1024];
    while offset < UEFI_NTFS_IMAGE.len() {
        let count = buffer.len().min(UEFI_NTFS_IMAGE.len() - offset);
        device.read_exact_at(esp.start_byte + offset as u64, &mut buffer[..count])?;
        if buffer[..count] != UEFI_NTFS_IMAGE[offset..offset + count] {
            return Err(Error::Other(
                "UEFI:NTFS boot image readback does not match the pinned asset".to_owned(),
            ));
        }
        offset += count;
    }
    Ok(())
}

#[cfg(all(feature = "windows-media", target_os = "macos"))]
fn verify_windows_ntfs(
    manifest: &[ManifestEntry],
    part: &mut PartitionIo<'_>,
    cancel: &CancelToken,
    sink: &dyn ProgressSink,
) -> Result<()> {
    let total = manifest.iter().map(|entry| entry.size).sum::<u64>().max(1);
    let started = Instant::now();
    let mut done = 0u64;
    let mut last_report = started;
    sink.report(ProgressEvent::message(
        Phase::Verifying,
        format!("verifying {} NTFS file(s)", manifest.len()),
    ));
    let mut volume = NtfsVolume::mount(part)?;
    for entry in manifest {
        cancel.check()?;
        let mut file = volume.open_file(&entry.path)?;
        let mut file_offset = 0u64;
        let mut hasher = Sha256::new();
        let mut buffer = vec![0u8; COPY_CHUNK];
        while file_offset < entry.size {
            cancel.check()?;
            let want = (entry.size - file_offset).min(buffer.len() as u64) as usize;
            let read = file.read_at(file_offset, &mut buffer[..want])?;
            if read == 0 {
                return Err(Error::Other(format!(
                    "NTFS verification reached EOF early for {}",
                    entry.path
                )));
            }
            hasher.update(&buffer[..read]);
            file_offset += read as u64;
            done += read as u64;
            let now = Instant::now();
            if now.duration_since(last_report) >= REPORT_INTERVAL {
                sink.report(verify_progress(done, total, &entry.path, started));
                last_report = now;
            }
        }
        let digest: [u8; 32] = hasher.finalize().into();
        if let Some(expected) = entry.sha256
            && digest != expected
        {
            return Err(Error::Other(format!(
                "NTFS verification checksum mismatch for {}",
                entry.path
            )));
        }
    }
    sink.report(verify_progress(total, total, "files verified", started));
    volume.unmount()?;
    Ok(())
}

#[cfg(all(feature = "windows-media", target_os = "macos"))]
fn verify_progress(done: u64, total: u64, message: &str, started: Instant) -> ProgressEvent {
    let elapsed = started.elapsed().as_secs_f64().max(0.001);
    ProgressEvent {
        phase: Phase::Verifying,
        done_bytes: done,
        total_bytes: total,
        bytes_per_sec: Some((done as f64 / elapsed) as u64),
        eta: if done < total {
            Some(Duration::from_secs_f64(
                (total - done) as f64 / (done as f64 / elapsed).max(0.001),
            ))
        } else {
            Some(Duration::ZERO)
        },
        message: Some(message.to_owned()),
        target_incomplete: false,
    }
}

fn copy_progress(done: u64, total: u64, message: &str, started: Instant) -> ProgressEvent {
    let elapsed = started.elapsed().as_secs_f64().max(0.001);
    ProgressEvent {
        phase: Phase::Copying,
        done_bytes: done,
        total_bytes: total,
        bytes_per_sec: Some((done as f64 / elapsed) as u64),
        eta: if done < total {
            Some(Duration::from_secs_f64(
                (total - done) as f64 / (done as f64 / elapsed),
            ))
        } else {
            Some(Duration::ZERO)
        },
        message: Some(message.to_owned()),
        target_incomplete: false,
    }
}

fn iso_error(err: image2usb_iso::IsoError) -> Error {
    Error::InvalidImage(err.to_string())
}

fn fat_error(context: &str, err: std::io::Error) -> Error {
    Error::Other(format!("FAT32 {context}: {err}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use image2usb_core::NoopSink;

    #[test]
    fn cancelling_snapshot_capture_returns_cancelled() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("source.iso");
        std::fs::write(&path, vec![0x5A; 1024 * 1024]).unwrap();
        let source = File::open(&path).unwrap();
        let mut snapshot = tempfile::tempfile().unwrap();
        let cancel = CancelToken::new();
        cancel.cancel();
        let error =
            capture_snapshot(&source, &mut snapshot, 1024 * 1024, &cancel, &NoopSink).unwrap_err();
        assert!(matches!(error, Error::Cancelled), "{error}");
    }

    #[test]
    fn source_metadata_change_invalidates_the_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("source.iso");
        std::fs::write(&path, b"first").unwrap();
        let before = std::fs::metadata(&path).unwrap();
        std::thread::sleep(Duration::from_millis(10));
        std::fs::write(&path, b"second").unwrap();
        let after = std::fs::metadata(&path).unwrap();
        let error = compare_metadata(&before, &after).unwrap_err();
        assert!(error.to_string().contains("changed"), "{error}");
    }

    #[test]
    fn enospc_becomes_a_scratch_error() {
        let error = map_snapshot_write_error(std::io::Error::from_raw_os_error(28));
        assert!(error.to_string().contains("temporary space"), "{error}");
    }
}
