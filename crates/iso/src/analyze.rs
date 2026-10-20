use std::fs::File;
use std::path::PathBuf;

use crate::eltorito::{ElToritoInfo, parse_catalog};
use crate::hybrid::{GptInfo, MbrInfo, inspect_gpt, inspect_mbr};
use crate::iso9660::{DirRecord, read_dir, read_file, read_volume_descriptors};
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

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct WindowsInfo {
    pub install_wim_size: Option<u64>,
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
    let tree = build_tree(&mut file, root, joliet_root.is_some())?;

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
    extent: u32,
    size: u32,
    is_dir: bool,
}

fn build_tree(file: &mut File, root: &DirRecord, joliet: bool) -> Result<Vec<TreeEntry>> {
    let mut out = Vec::new();
    let mut stack = vec![(root.clone(), String::new(), 0usize)];
    while let Some((dir, prefix, depth)) = stack.pop() {
        if depth > 6 || out.len() > 20_000 {
            break;
        }
        let entries = read_dir(file, &dir, joliet, 20_000)?;
        for entry in entries {
            let name = entry.name.trim().to_owned();
            if name.is_empty() {
                continue;
            }
            let path = if prefix.is_empty() {
                name.to_lowercase()
            } else {
                format!("{prefix}/{}", name.to_lowercase())
            };
            let is_dir = entry.is_dir();
            out.push(TreeEntry {
                path: path.clone(),
                extent: entry.extent,
                size: entry.len,
                is_dir,
            });
            if is_dir {
                stack.push((entry, path, depth + 1));
            }
        }
    }
    Ok(out)
}

fn find<'a>(tree: &'a [TreeEntry], path: &str) -> Option<&'a TreeEntry> {
    tree.iter().find(|e| e.path == path)
}

fn has_dir(tree: &[TreeEntry], path: &str) -> bool {
    find(tree, path).is_some_and(|e| e.is_dir)
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

    let windows = if find(tree, "sources/install.wim").is_some() || find(tree, "bootmgr").is_some()
    {
        boot_files.bootmgr = find(tree, "bootmgr").is_some();
        Some(WindowsInfo {
            install_wim_size: find(tree, "sources/install.wim").map(|e| e.size as u64),
        })
    } else {
        None
    };
    if windows.is_some() {
        return Ok((Distro::Windows, windows));
    }

    if let Some(entry) = find(tree, ".disk/info") {
        match read_file(file, entry.extent, entry.size, 1024) {
            Ok(bytes) => {
                let text = String::from_utf8_lossy(&bytes);
                if text.contains("Ubuntu") {
                    return Ok((Distro::Ubuntu, None));
                }
                if text.contains("Debian") {
                    return Ok((Distro::Debian, None));
                }
            }
            Err(err) => warnings.push(format!("could not read .disk/info: {err}")),
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
            && let Ok(bytes) = read_file(file, entry.extent, entry.size, 4096)
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
