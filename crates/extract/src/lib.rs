//! Extraction mode: partition a device, format FAT32 and copy the ISO tree.
//!
//! This crate is the only writer of filesystems. It is deliberately separate
//! from `image2usb-iso` (read-only) and `image2usb-core` (raw I/O) so that the
//! dangerous operations are easy to audit in one place.

#![forbid(unsafe_code)]

mod bios;
mod extract;
mod gpt;
pub mod layout;
mod names;
mod partition;
mod postprocess;
mod windows;
#[cfg(all(feature = "windows-media", target_os = "macos"))]
mod windows_layout;

pub use extract::{
    ExtractStats, analyze_iso, extract_image, list_extracted, preview, read_extracted,
};
pub use gpt::{GptLayout, plan_gpt_layout, write_gpt};
pub use layout::{PartitionLayout, plan_layout, write_mbr};
pub use partition::PartitionIo;
pub use windows::{
    PreparedWindows, WindowsCatalogSource, extract_windows_fat32, extract_windows_ntfs,
    prepare_windows, resolve_filesystem,
};

/// Whether this build can commit Windows media to NTFS plus a UEFI:NTFS ESP.
///
/// The native NTFS formatter is currently macOS-only and is enabled with the
/// `windows-media` Cargo feature.
pub const WINDOWS_NTFS_SUPPORTED: bool = cfg!(all(feature = "windows-media", target_os = "macos"));
