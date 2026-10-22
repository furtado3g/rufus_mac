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

/// Filesystem that the writer may actually create.
///
/// This type deliberately has no `Auto` variant: automatic selection must be
/// resolved before a write plan reaches the formatting/copying layer.
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

/// Filesystem requested by the user.
///
/// `Auto` is resolved during preparation, after the image has been analyzed.
/// It is never passed to the writer as a formatting decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileSystemChoice {
    Auto,
    #[default]
    Fat32,
    Ntfs,
}

impl FileSystemChoice {
    /// Return the concrete filesystem, or `None` when `Auto` still needs to be
    /// resolved from the image contents.
    pub const fn resolved(self) -> Option<FileSystem> {
        match self {
            FileSystemChoice::Auto => None,
            FileSystemChoice::Fat32 => Some(FileSystem::Fat32),
            FileSystemChoice::Ntfs => Some(FileSystem::Ntfs),
        }
    }
}

impl From<FileSystem> for FileSystemChoice {
    fn from(value: FileSystem) -> Self {
        match value {
            FileSystem::Fat32 => FileSystemChoice::Fat32,
            FileSystem::Ntfs => FileSystemChoice::Ntfs,
        }
    }
}

impl fmt::Display for FileSystemChoice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            FileSystemChoice::Auto => "auto",
            FileSystemChoice::Fat32 => "fat32",
            FileSystemChoice::Ntfs => "ntfs",
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

/// Independent Windows customization switches.
///
/// Every switch defaults to `false`; enabling one must never enable another.
/// Missing fields are accepted so older configuration files keep loading.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct WindowsOptions {
    pub bypass_tpm: bool,
    pub bypass_secure_boot: bool,
    pub offline_account: bool,
}

/// User-tunable options for extraction mode.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct WriteOptions {
    pub scheme: PartitionScheme,
    pub filesystem: FileSystemChoice,
    pub label: String,
    pub cluster_size: Option<u32>,
    pub verify: VerifyMode,
    pub split_wim: bool,
    pub windows: WindowsOptions,
}

impl Default for WriteOptions {
    fn default() -> Self {
        Self {
            scheme: PartitionScheme::Mbr,
            filesystem: FileSystemChoice::Fat32,
            label: "IMAGE2USB".to_owned(),
            cluster_size: None,
            verify: VerifyMode::None,
            split_wim: true,
            windows: WindowsOptions::default(),
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
    /// Selection requested by the user, retained for the final explanation.
    #[serde(default)]
    pub requested_filesystem: FileSystemChoice,
    /// Concrete filesystem the writer will use; never `Auto`.
    pub filesystem: FileSystem,
    /// Why automatic/manual selection produced `filesystem`.
    #[serde(default)]
    pub filesystem_reason: String,
    pub label: String,
    pub cluster_size: Option<u32>,
    pub verify: VerifyMode,
    #[serde(default)]
    pub windows: WindowsOptions,
    pub warnings: Vec<String>,
}

impl WritePlan {
    pub fn summary(&self) -> String {
        let mut summary = format!(
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
        );
        if !self.filesystem_reason.is_empty() {
            summary.push_str(": ");
            summary.push_str(&self.filesystem_reason);
        }
        if self.requested_filesystem == FileSystemChoice::Auto {
            summary.push_str(" [requested auto]");
        }
        summary
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filesystem_choice_never_resolves_auto_to_a_writer_value() {
        assert_eq!(FileSystemChoice::Auto.resolved(), None);
        assert_eq!(FileSystemChoice::Fat32.resolved(), Some(FileSystem::Fat32));
        assert_eq!(FileSystemChoice::Ntfs.resolved(), Some(FileSystem::Ntfs));
        assert_eq!(FileSystemChoice::default(), FileSystemChoice::Fat32);
        assert_eq!(
            serde_json::to_string(&FileSystemChoice::Auto).unwrap(),
            "\"auto\""
        );
        assert_eq!(
            serde_json::from_str::<FileSystemChoice>("\"ntfs\"").unwrap(),
            FileSystemChoice::Ntfs
        );
        assert_eq!(
            FileSystemChoice::from(FileSystem::Ntfs),
            FileSystemChoice::Ntfs
        );
        assert_eq!(FileSystemChoice::Auto.to_string(), "auto");
    }

    #[test]
    fn windows_options_defaults_are_independent_and_missing_fields_load() {
        let defaults = WindowsOptions::default();
        assert!(!defaults.bypass_tpm);
        assert!(!defaults.bypass_secure_boot);
        assert!(!defaults.offline_account);
        assert_eq!(
            serde_json::from_str::<WindowsOptions>("{}").unwrap(),
            defaults
        );

        let tpm: WindowsOptions = serde_json::from_str(r#"{"bypass_tpm":true}"#).unwrap();
        assert!(tpm.bypass_tpm);
        assert!(!tpm.bypass_secure_boot);
        assert!(!tpm.offline_account);

        let secure_boot: WindowsOptions =
            serde_json::from_str(r#"{"bypass_secure_boot":true}"#).unwrap();
        assert!(!secure_boot.bypass_tpm);
        assert!(secure_boot.bypass_secure_boot);
        assert!(!secure_boot.offline_account);

        let account: WindowsOptions = serde_json::from_str(r#"{"offline_account":true}"#).unwrap();
        assert!(!account.bypass_tpm);
        assert!(!account.bypass_secure_boot);
        assert!(account.offline_account);

        let all: WindowsOptions = serde_json::from_str(
            r#"{"bypass_tpm":true,"bypass_secure_boot":true,"offline_account":true}"#,
        )
        .unwrap();
        assert_eq!(
            all,
            WindowsOptions {
                bypass_tpm: true,
                bypass_secure_boot: true,
                offline_account: true,
            }
        );
    }

    #[test]
    fn write_options_roundtrip_and_accept_missing_fields() {
        let options: WriteOptions = serde_json::from_str("{}").unwrap();
        assert_eq!(options, WriteOptions::default());
        assert_eq!(options.filesystem, FileSystemChoice::Fat32);
        assert_eq!(options.windows, WindowsOptions::default());

        let encoded = serde_json::to_value(WriteOptions::default()).unwrap();
        assert!(encoded.get("bypass_win11").is_none());
        assert_eq!(
            encoded["windows"],
            serde_json::json!({
                "bypass_tpm": false,
                "bypass_secure_boot": false,
                "offline_account": false,
            })
        );

        let options: WriteOptions =
            serde_json::from_str(r#"{"filesystem":"auto","windows":{"bypass_secure_boot":true}}"#)
                .unwrap();
        assert_eq!(options.filesystem, FileSystemChoice::Auto);
        assert!(options.windows.bypass_secure_boot);
        assert!(!options.windows.bypass_tpm);
        assert!(!options.windows.offline_account);

        let roundtrip: WriteOptions =
            serde_json::from_str(&serde_json::to_string(&options).unwrap()).unwrap();
        assert_eq!(roundtrip, options);
    }

    #[test]
    fn write_plan_roundtrips_requested_and_resolved_filesystems() {
        let plan = WritePlan {
            image_path: PathBuf::from("/images/Win11.iso"),
            image_len: 5_000_000_000,
            mode: ResolvedMode::Extract,
            scheme: PartitionScheme::Gpt,
            requested_filesystem: FileSystemChoice::Auto,
            filesystem: FileSystem::Ntfs,
            filesystem_reason: "large non-splittable ESD".to_owned(),
            label: "WIN11".to_owned(),
            cluster_size: None,
            verify: VerifyMode::Sha256,
            windows: WindowsOptions {
                bypass_tpm: true,
                bypass_secure_boot: false,
                offline_account: true,
            },
            warnings: vec!["Secure Boot must be disabled manually".to_owned()],
        };
        let json = serde_json::to_string(&plan).unwrap();
        let decoded: WritePlan = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, plan);
        assert_eq!(decoded.requested_filesystem, FileSystemChoice::Auto);
        assert_eq!(decoded.filesystem, FileSystem::Ntfs);
        assert!(decoded.summary().contains("large non-splittable ESD"));
        assert!(decoded.summary().contains("requested auto"));
        assert!(!json.contains("\"filesystem\":\"auto\""));
    }

    #[test]
    fn write_plan_accepts_missing_optional_fields() {
        let plan: WritePlan = serde_json::from_str(
            r#"{
                "image_path":"/tmp/image.iso",
                "image_len":1024,
                "mode":"extract",
                "scheme":"mbr",
                "filesystem":"fat32",
                "label":"TEST",
                "cluster_size":null,
                "verify":"none",
                "warnings":[]
            }"#,
        )
        .unwrap();
        assert_eq!(plan.requested_filesystem, FileSystemChoice::Fat32);
        assert_eq!(plan.filesystem_reason, "");
        assert_eq!(plan.windows, WindowsOptions::default());
    }
}
