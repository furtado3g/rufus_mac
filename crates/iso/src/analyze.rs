use std::fs::File;
use std::path::PathBuf;

use crate::eltorito::{ElToritoInfo, parse_catalog};
use crate::hybrid::{GptInfo, MbrInfo, inspect_gpt, inspect_mbr};
use crate::iso9660::{DirRecord, read_volume_descriptors};
use crate::{IsoError, Result};

/// Compressed view of the boot-related files found in an image.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct BootFiles {
    pub isolinux: Option<String>,
    pub syslinux: Option<String>,
    pub grub_cfg: Option<String>,
    pub efi_boot: Vec<String>,
    pub bootmgr: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Distro {
    Ubuntu,
    Debian,
    Fedora,
    Arch,
    Alpine,
    Windows,
    Other(String),
}

impl Distro {
    pub fn label(&self) -> &str {
        match self {
            Distro::Ubuntu => "Ubuntu",
            Distro::Debian => "Debian",
            Distro::Fedora => "Fedora",
            Distro::Arch => "Arch Linux",
            Distro::Alpine => "Alpine Linux",
            Distro::Windows => "Windows",
            Distro::Other(name) => name,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WindowsArchitecture {
    X64,
    Arm64,
    #[default]
    Unknown,
}

impl WindowsArchitecture {
    pub fn label(self) -> &'static str {
        match self {
            WindowsArchitecture::X64 => "x64",
            WindowsArchitecture::Arm64 => "arm64",
            WindowsArchitecture::Unknown => "unknown",
        }
    }

    /// EFI removable-media fallback required for this architecture.
    pub fn efi_fallback(self) -> Option<&'static str> {
        match self {
            WindowsArchitecture::X64 => Some("efi/boot/bootx64.efi"),
            WindowsArchitecture::Arm64 => Some("efi/boot/bootaa64.efi"),
            WindowsArchitecture::Unknown => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct WindowsPart {
    pub name: String,
    pub size: u64,
}

/// Windows installation-media metadata.
///
/// These fields are additive for JSON consumers. A detected candidate is not a
/// claim that the chain boots: preparation still validates the fallback PE
/// machine and every required boot file before writing.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct WindowsInfo {
    /// Backwards-compatible `sources/install.wim` size.
    pub install_wim_size: Option<u64>,
    #[serde(default)]
    pub architecture: WindowsArchitecture,
    /// `wim`, `esd` or `swm` when an installation image is present.
    #[serde(default)]
    pub install_format: Option<String>,
    /// WIM/ESD/SWM files carrying the installation images.
    #[serde(default)]
    pub install_parts: Vec<WindowsPart>,
    /// Architecture reported by `sources/idwbinfo.txt`, when present.
    #[serde(default)]
    pub idwbinfo_architecture: WindowsArchitecture,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub build: Option<String>,
    #[serde(default)]
    pub edition: Option<String>,
    #[serde(default)]
    pub bootmgr: bool,
    #[serde(default)]
    pub bcd: bool,
    /// Architecture-specific removable-media fallback found in `EFI/BOOT`.
    #[serde(default)]
    pub efi_fallback: Option<String>,
    /// PE machine decoded from the fallback, independent of its filename.
    #[serde(default)]
    pub pe_architecture: WindowsArchitecture,
}

/// Everything image2usb knows about an image before planning a write.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct IsoInfo {
    pub path: PathBuf,
    pub size: u64,
    pub volume_id: Option<String>,
    pub has_joliet: bool,
    pub has_rock_ridge: bool,
    pub has_udf: bool,
    pub el_torito: Option<ElToritoInfo>,
    pub mbr: Option<MbrInfo>,
    pub gpt: Option<GptInfo>,
    /// Writing the image 1:1 produces a device bootable in legacy BIOS mode.
    pub dd_bios: bool,
    /// Writing the image 1:1 produces a device bootable in UEFI mode.
    pub dd_uefi: bool,
    pub distro: Distro,
    pub windows: Option<WindowsInfo>,
    pub boot_files: BootFiles,
    pub warnings: Vec<String>,
}

impl IsoInfo {
    pub fn dd_safe(&self) -> bool {
        self.dd_bios || self.dd_uefi
    }
}

/// Analyze an image on disk.
pub fn analyze_path(path: impl Into<PathBuf>) -> Result<IsoInfo> {
    let path = path.into();
    let mut file = File::open(&path)?;
    let size = file.metadata()?.len();
    if size < 17 * 2048 {
        return Err(IsoError::Truncated);
    }

    let descriptors = read_volume_descriptors(&mut file, size)?;
    let pvd = descriptors
        .primary
        .clone()
        .ok_or_else(|| IsoError::NotIso("no primary volume descriptor found".to_owned()))?;

    let mbr = inspect_mbr(&mut file)?;
    let gpt = inspect_gpt(&mut file, size)?;

    let el_torito = match descriptors.el_torito_catalog {
        Some(lba) => match parse_catalog(&mut file, lba) {
            Ok(info) => Some(info),
            Err(err) => {
                tracing::warn!(%err, "failed to parse El Torito catalog");
                None
            }
        },
        None => None,
    };

    let mut warnings = Vec::new();
    let mut boot_files = BootFiles::default();

    let joliet_root = descriptors.joliet.clone();
    let root = joliet_root.as_ref().unwrap_or(&pvd.root);
    let tree = build_tree(&mut file, root, joliet_root.is_some(), size)?;

    let (distro, windows) = detect_distro(&mut file, &tree, &mut boot_files, &mut warnings)?;

    let mut dd_bios = mbr.as_ref().is_some_and(|m| m.isohybrid_bootable());
    let mut dd_uefi = gpt.as_ref().is_some_and(|g| g.has_esp)
        || mbr.as_ref().is_some_and(|m| m.has_efi_partition());
    if el_torito.as_ref().is_some_and(|e| e.uefi) {
        dd_uefi = dd_uefi || mbr.as_ref().is_some_and(|m| m.isohybrid_bootable());
    }

    if distro == Distro::Windows {
        // Windows install media boot from USB in UEFI mode when copied 1:1,
        // but the legacy BIOS path requires an extracted, bootstrapped volume.
        dd_bios = false;
        if dd_uefi {
            warnings.push(
                "Windows image: DD mode boots in UEFI only; extraction is required for legacy BIOS"
                    .to_owned(),
            );
        }
    }

    if mbr.is_none() {
        warnings.push("image has no MBR; UEFI firmware will not boot a DD-written copy".to_owned());
    } else if !dd_bios && mbr.as_ref().is_some_and(|m| m.boot_code_nonzero) {
        warnings.push(
            "MBR boot code is present but no active partition was found; legacy BIOS may not boot"
                .to_owned(),
        );
    }

    if el_torito.is_some() && !dd_bios && !dd_uefi {
        warnings.push(
            "El Torito boot catalog found but the image is not hybrid; extraction mode is required"
                .to_owned(),
        );
    }

    if let Some(info) = &windows
        && info
            .install_wim_size
            .is_some_and(|s| s > 4 * 1024 * 1024 * 1024)
    {
        warnings.push(
            "sources/install.wim is larger than 4 GiB and must be split for FAT32".to_owned(),
        );
    }

    if descriptors.has_udf {
        warnings
            .push("image contains UDF data; file extraction prefers UDF over ISO 9660".to_owned());
    }

    Ok(IsoInfo {
        path,
        size,
        volume_id: Some(pvd.volume_id.clone()),
        has_joliet: descriptors.has_joliet,
        has_rock_ridge: descriptors.has_rock_ridge,
        has_udf: descriptors.has_udf,
        el_torito,
        mbr,
        gpt,
        dd_bios,
        dd_uefi,
        distro,
        windows,
        boot_files,
        warnings,
    })
}

#[derive(Debug, Clone)]
struct TreeEntry {
    path: String,
    extent: u64,
    size: u64,
    extents: Vec<crate::iso9660::Extent>,
    is_dir: bool,
}

fn build_tree(
    file: &mut File,
    root: &DirRecord,
    joliet: bool,
    image_size: u64,
) -> Result<Vec<TreeEntry>> {
    let entries = crate::iso9660::walk_tree(file, root, joliet, 20_000, image_size)?;
    Ok(entries
        .into_iter()
        .map(|entry| TreeEntry {
            path: entry.path.to_lowercase(),
            extent: entry.extent,
            size: entry.len,
            extents: entry.extents,
            is_dir: entry.is_dir,
        })
        .collect())
}

fn find<'a>(tree: &'a [TreeEntry], path: &str) -> Option<&'a TreeEntry> {
    tree.iter().find(|e| e.path == path)
}

fn has_dir(tree: &[TreeEntry], path: &str) -> bool {
    find(tree, path).is_some_and(|e| e.is_dir)
}

fn detect_windows(
    file: &mut File,
    tree: &[TreeEntry],
    boot_files: &mut BootFiles,
    warnings: &mut Vec<String>,
) -> Result<Option<WindowsInfo>> {
    let install_files = tree
        .iter()
        .filter(|entry| !entry.is_dir && is_install_image_path(&entry.path))
        .collect::<Vec<_>>();
    let bootmgr = find(tree, "bootmgr");
    if install_files.is_empty() && bootmgr.is_none() {
        return Ok(None);
    }

    boot_files.bootmgr = bootmgr.is_some();
    let mut info = WindowsInfo {
        bootmgr: bootmgr.is_some(),
        bcd: find(tree, "boot/bcd").is_some(),
        ..WindowsInfo::default()
    };

    info.install_parts = install_files
        .iter()
        .map(|entry| WindowsPart {
            name: entry.path.clone(),
            size: entry.size,
        })
        .collect();
    info.install_parts.sort_by(|a, b| a.name.cmp(&b.name));
    info.install_wim_size = info
        .install_parts
        .iter()
        .find(|part| part.name == "sources/install.wim")
        .map(|part| part.size);
    info.install_format = install_format(&info.install_parts);

    // Prefer the PE header over the fallback filename. A renamed or mismatched
    // fallback must be visible to preparation, which refuses the image.
    let x64 = find(tree, "efi/boot/bootx64.efi");
    let arm64 = find(tree, "efi/boot/bootaa64.efi");
    let fallback = match (x64, arm64) {
        (Some(entry), None) => Some(("efi/boot/bootx64.efi", entry, WindowsArchitecture::X64)),
        (None, Some(entry)) => Some(("efi/boot/bootaa64.efi", entry, WindowsArchitecture::Arm64)),
        (Some(_), Some(_)) => {
            warnings.push(
                "Windows image contains both BOOTX64.EFI and BOOTAA64.EFI; the target architecture \
                 is ambiguous"
                    .to_owned(),
            );
            None
        }
        (None, None) => None,
    };
    let mut filename_architecture = WindowsArchitecture::Unknown;
    if let Some((name, entry, architecture)) = fallback {
        filename_architecture = architecture;
        info.efi_fallback = Some(name.to_owned());
        if let Some(prefix) = read_tree_prefix(file, entry, 4096) {
            info.pe_architecture = pe_architecture(&prefix);
        }
        if info.pe_architecture == WindowsArchitecture::Unknown {
            warnings.push(format!(
                "{name} has no recognized PE machine; preparation will refuse extraction"
            ));
        } else if info.pe_architecture != architecture {
            warnings.push(format!(
                "{name} is named for {} but its PE machine is {}",
                architecture.label(),
                info.pe_architecture.label()
            ));
        }
    }

    let idwbinfo =
        find(tree, "sources/idwbinfo.txt").and_then(|entry| read_tree_text(file, entry, 64 * 1024));
    if let Some(text) = &idwbinfo {
        let values = key_values(text);
        info.idwbinfo_architecture = match values.get("buildarch").map(String::as_str) {
            Some("9") => WindowsArchitecture::X64,
            Some("12") => WindowsArchitecture::Arm64,
            _ => WindowsArchitecture::Unknown,
        };
        info.build = values
            .get("buildnumber")
            .filter(|value| !value.is_empty())
            .cloned();
        info.version = values
            .get("buildversion")
            .or_else(|| values.get("version"))
            .filter(|value| !value.is_empty())
            .cloned();
        info.edition = values
            .get("editionid")
            .or_else(|| values.get("edition"))
            .filter(|value| !value.is_empty())
            .cloned();
    }

    info.architecture = match info.pe_architecture {
        WindowsArchitecture::Unknown => {
            if filename_architecture != WindowsArchitecture::Unknown {
                filename_architecture
            } else {
                info.idwbinfo_architecture
            }
        }
        known => known,
    };

    if !info.bootmgr {
        warnings.push(
            "Windows installation image has no BOOTMGR; boot chain validation will fail".to_owned(),
        );
    }
    if !info.bcd {
        warnings.push(
            "Windows installation image has no boot/BCD; boot chain validation will fail"
                .to_owned(),
        );
    }
    if info.efi_fallback.is_none() {
        warnings.push(
            "Windows image has no architecture-specific EFI/BOOT fallback; preparation will \
             refuse extraction"
                .to_owned(),
        );
    }
    if info.architecture == WindowsArchitecture::Unknown {
        warnings
            .push("Windows architecture is unknown; preparation will refuse extraction".to_owned());
    }
    if let Some(format) = &info.install_format
        && format == "swm"
    {
        validate_swm_sequence(&info.install_parts, warnings);
    }
    if install_files.len() > 1
        && info
            .install_parts
            .iter()
            .filter(|part| matches!(install_extension(&part.name), Some("wim") | Some("esd")))
            .count()
            > 1
    {
        warnings.push(
            "Windows image contains more than one monolithic installation image format".to_owned(),
        );
    }

    Ok(Some(info))
}

fn is_install_image_path(path: &str) -> bool {
    let Some(rest) = path.strip_prefix("sources/") else {
        return false;
    };
    matches!(rest, "install.wim" | "install.esd" | "install.swm")
        || (rest.starts_with("install")
            && rest.ends_with(".swm")
            && rest[7..rest.len() - 4]
                .chars()
                .all(|ch| ch.is_ascii_digit())
            && !rest[7..rest.len() - 4].is_empty())
}

fn install_extension(path: &str) -> Option<&str> {
    path.rsplit_once('.').map(|(_, extension)| extension)
}

fn install_format(parts: &[WindowsPart]) -> Option<String> {
    for format in ["wim", "esd", "swm"] {
        if parts
            .iter()
            .any(|part| install_extension(&part.name) == Some(format))
        {
            return Some(format.to_owned());
        }
    }
    None
}

fn validate_swm_sequence(parts: &[WindowsPart], warnings: &mut Vec<String>) {
    let mut numbers = Vec::new();
    for part in parts {
        let Some(rest) = part.name.strip_prefix("sources/install") else {
            continue;
        };
        let Some(number) = rest.strip_suffix(".swm") else {
            continue;
        };
        if number.is_empty() {
            numbers.push(1u64);
        } else if let Ok(value) = number.parse::<u64>() {
            numbers.push(value);
        } else {
            warnings.push(format!("invalid SWM part name: {}", part.name));
        }
    }
    numbers.sort_unstable();
    numbers.dedup();
    if numbers.is_empty() {
        return;
    }
    let expected = (1..=numbers.len() as u64).collect::<Vec<_>>();
    if numbers != expected {
        warnings.push(format!(
            "SWM part sequence is incomplete or has gaps: found {:?}",
            numbers
        ));
    }
}

fn read_tree_prefix(file: &mut File, entry: &TreeEntry, max: usize) -> Option<Vec<u8>> {
    let source = crate::WalkEntry {
        path: entry.path.clone(),
        extent: entry.extent,
        len: entry.size,
        extents: entry.extents.clone(),
        is_dir: entry.is_dir,
        symlink_target: None,
    };
    crate::iso9660::read_walk_entry(file, &source, max).ok()
}

fn read_tree_text(file: &mut File, entry: &TreeEntry, max: usize) -> Option<String> {
    let bytes = read_tree_prefix(file, entry, max)?;
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

fn key_values(text: &str) -> std::collections::HashMap<String, String> {
    text.lines()
        .filter_map(|line| {
            let (key, value) = line.split_once('=')?;
            Some((key.trim().to_ascii_lowercase(), value.trim().to_owned()))
        })
        .collect()
}

fn pe_architecture(bytes: &[u8]) -> WindowsArchitecture {
    if bytes.len() < 0x40 || &bytes[..2] != b"MZ" {
        return WindowsArchitecture::Unknown;
    }
    let offset = u32::from_le_bytes(bytes[0x3c..0x40].try_into().expect("four bytes")) as usize;
    let Some(header) = bytes.get(offset..offset.saturating_add(6)) else {
        return WindowsArchitecture::Unknown;
    };
    if &header[..4] != b"PE\0\0" {
        return WindowsArchitecture::Unknown;
    }
    match u16::from_le_bytes([header[4], header[5]]) {
        0x8664 => WindowsArchitecture::X64,
        0xAA64 => WindowsArchitecture::Arm64,
        _ => WindowsArchitecture::Unknown,
    }
}

fn detect_distro(
    file: &mut File,
    tree: &[TreeEntry],
    boot_files: &mut BootFiles,
    warnings: &mut Vec<String>,
) -> Result<(Distro, Option<WindowsInfo>)> {
    if find(tree, "isolinux/isolinux.cfg").is_some() {
        boot_files.isolinux = Some("isolinux/isolinux.cfg".to_owned());
        boot_files.syslinux = Some("isolinux/isolinux.cfg".to_owned());
    }
    if find(tree, "syslinux/syslinux.cfg").is_some() {
        boot_files.syslinux = Some("syslinux/syslinux.cfg".to_owned());
    }
    for candidate in [
        "efi/boot/grub.cfg",
        "boot/grub/grub.cfg",
        "efi/boot/bootx64.efi",
    ] {
        if find(tree, candidate).is_some() {
            boot_files.efi_boot.push(candidate.to_owned());
        }
    }
    boot_files.efi_boot.sort();
    boot_files.efi_boot.dedup();
    if find(tree, "efi/boot/grub.cfg").is_some() {
        boot_files.grub_cfg = Some("efi/boot/grub.cfg".to_owned());
    } else if find(tree, "boot/grub/grub.cfg").is_some() {
        boot_files.grub_cfg = Some("boot/grub/grub.cfg".to_owned());
    }

    let windows = detect_windows(file, tree, boot_files, warnings)?;
    if windows.is_some() {
        return Ok((Distro::Windows, windows));
    }

    if let Some(entry) = find(tree, ".disk/info") {
        match read_tree_prefix(file, entry, 1024) {
            Some(bytes) => {
                let text = String::from_utf8_lossy(&bytes);
                if text.contains("Ubuntu") {
                    return Ok((Distro::Ubuntu, None));
                }
                if text.contains("Debian") {
                    return Ok((Distro::Debian, None));
                }
            }
            None => warnings.push("could not read .disk/info".to_owned()),
        }
    }

    if has_dir(tree, "casper") {
        return Ok((Distro::Ubuntu, None));
    }
    if has_dir(tree, "apks") || find(tree, ".alpine-release").is_some() {
        return Ok((Distro::Alpine, None));
    }
    if has_dir(tree, "arch") || find(tree, "arch/version").is_some() {
        return Ok((Distro::Arch, None));
    }
    if find(tree, ".discinfo").is_some()
        && (find(tree, "images/pxeboot/vmlinuz").is_some()
            || find(tree, "Fedora-Legal-README.txt").is_some()
            || find(tree, "images/install.img").is_some())
    {
        return Ok((Distro::Fedora, None));
    }
    if find(tree, ".treeinfo").is_some() {
        if let Some(entry) = find(tree, ".treeinfo")
            && let Some(bytes) = read_tree_prefix(file, entry, 4096)
        {
            let text = String::from_utf8_lossy(&bytes);
            if text.to_lowercase().contains("fedora") {
                return Ok((Distro::Fedora, None));
            }
        }
        return Ok((Distro::Fedora, None));
    }
    if has_dir(tree, "isolinux") {
        return Ok((Distro::Debian, None));
    }

    let volume_marker = tree
        .iter()
        .find(|e| !e.is_dir && e.path.ends_with("readme.diskdefines"))
        .map(|e| e.path.clone());
    if volume_marker.is_some() {
        return Ok((Distro::Ubuntu, None));
    }

    Ok((Distro::Other("unknown".to_owned()), None))
}
