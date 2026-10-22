//! ISO image analysis: ISO 9660, Joliet, Rock Ridge, El Torito and hybrid
//! boot metadata.
//!
//! This crate only reads images. It never writes to a device.

#![forbid(unsafe_code)]

mod analyze;
mod eltorito;
mod hybrid;
mod iso9660;

pub use analyze::{
    BootFiles, Distro, IsoInfo, WindowsArchitecture, WindowsCapabilities, WindowsInfo, WindowsPart,
    WindowsTreeEntry, analyze_path, detect_windows_from_tree,
};
pub use eltorito::{BootEntry, ElToritoInfo, MediaType, Platform};
pub use hybrid::{GptInfo, MbrInfo, MbrPartition};
pub use iso9660::{
    DirRecord, Extent, SECTOR_SIZE, VolumeDescriptors, WalkEntry, read_dir, read_file,
    read_volume_descriptors, read_walk_entry, walk_tree,
};

use thiserror::Error;

#[derive(Debug, Error)]
pub enum IsoError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("not an ISO 9660 image: {0}")]
    NotIso(String),

    #[error("image is truncated or too small")]
    Truncated,

    #[error("invalid directory record at offset {0}")]
    InvalidDirectoryRecord(u64),

    #[error("invalid El Torito catalog: {0}")]
    InvalidElTorito(String),

    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, IsoError>;
