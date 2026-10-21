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

pub use extract::{ExtractStats, extract_image, list_extracted, preview, read_extracted};
pub use gpt::{GptLayout, plan_gpt_layout, write_gpt};
pub use layout::{PartitionLayout, plan_layout, write_mbr};
pub use partition::PartitionIo;
