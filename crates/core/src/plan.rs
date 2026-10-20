use std::fmt;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Requested write strategy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WriteMode {
    /// Pick the best strategy from the image analysis.
    #[default]
    Auto,
    /// Copy the image 1:1 to the device.
    Dd,
    /// Create a partition table, format a filesystem and copy files.
    Extract,
}

impl fmt::Display for WriteMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            WriteMode::Auto => "auto",
            WriteMode::Dd => "dd",
            WriteMode::Extract => "extract",
        })
    }
}

/// Strategy actually selected for a write.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResolvedMode {
    Dd,
    Extract,
}

impl fmt::Display for ResolvedMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            ResolvedMode::Dd => "dd",
            ResolvedMode::Extract => "extract",
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PartitionScheme {
    #[default]
    Mbr,
    Gpt,
}

impl fmt::Display for PartitionScheme {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            PartitionScheme::Mbr => "mbr",
            PartitionScheme::Gpt => "gpt",
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileSystem {
    #[default]
    Fat32,
    Ntfs,
}

impl fmt::Display for FileSystem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            FileSystem::Fat32 => "fat32",
            FileSystem::Ntfs => "ntfs",
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VerifyMode {
    #[default]
    None,
    Sha256,
}

/// User-tunable options for extraction mode.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WriteOptions {
    pub scheme: PartitionScheme,
    pub filesystem: FileSystem,
    pub label: String,
    pub cluster_size: Option<u32>,
    pub verify: VerifyMode,
    pub split_wim: bool,
    pub bypass_win11: bool,
}

impl Default for WriteOptions {
    fn default() -> Self {
        Self {
            scheme: PartitionScheme::Mbr,
            filesystem: FileSystem::Fat32,
            label: "IMAGE2USB".to_owned(),
            cluster_size: None,
            verify: VerifyMode::None,
            split_wim: true,
            bypass_win11: false,
        }
    }
}

/// Fully resolved plan, produced before touching a device and shown to the user.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WritePlan {
    pub image_path: PathBuf,
    pub image_len: u64,
    pub mode: ResolvedMode,
    pub scheme: PartitionScheme,
    pub filesystem: FileSystem,
    pub label: String,
    pub cluster_size: Option<u32>,
    pub verify: VerifyMode,
    pub warnings: Vec<String>,
}

impl WritePlan {
    pub fn summary(&self) -> String {
        format!(
            "{} image -> {} device, {} ({}), label {}",
            self.mode,
            if self.mode == ResolvedMode::Dd {
                "raw"
            } else {
                "partitioned"
            },
            self.filesystem,
            self.scheme,
            self.label
        )
    }
}
