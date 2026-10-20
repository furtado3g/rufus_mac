use std::fmt;

use serde::{Deserialize, Serialize};

/// Opaque identifier of a block device, in the platform's native form
/// (`/dev/disk4`, `/dev/sdb`, `\\.\PhysicalDrive2`).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DeviceId(pub String);

impl DeviceId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for DeviceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<String> for DeviceId {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for DeviceId {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}

/// Physical transport of a device.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Bus {
    Usb,
    SdCard,
    Internal,
    Virtual,
    Network,
    Unknown,
}

impl Bus {
    /// Whether media on this bus is safe to offer as a write target.
    pub fn is_removable_candidate(self) -> bool {
        matches!(self, Bus::Usb | Bus::SdCard)
    }
}

impl fmt::Display for Bus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            Bus::Usb => "usb",
            Bus::SdCard => "sd",
            Bus::Internal => "internal",
            Bus::Virtual => "virtual",
            Bus::Network => "network",
            Bus::Unknown => "unknown",
        };
        f.write_str(name)
    }
}

/// Snapshot of a block device as reported by the platform.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceInfo {
    pub id: DeviceId,
    /// Human readable name, usually model + size.
    pub display_name: String,
    pub model: String,
    pub serial: Option<String>,
    /// Total capacity in bytes.
    pub size: u64,
    pub bus: Bus,
    pub removable: bool,
    pub mount_points: Vec<String>,
    /// True when the device hosts the running operating system.
    pub is_system: bool,
    pub read_only: bool,
}

impl DeviceInfo {
    /// Build the token used to re-validate this exact device before writing.
    pub fn identity(&self) -> IdentityToken {
        IdentityToken {
            model: self.model.clone(),
            serial: self.serial.clone(),
            size: self.size,
        }
    }

    pub fn is_safe_target(&self) -> bool {
        !self.is_system && !self.read_only && self.bus.is_removable_candidate()
    }
}

/// Identity of a device at enumeration time.
///
/// Re-checked immediately before opening a device so that swapping the USB
/// stick between confirmation and write cannot target the wrong disk.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IdentityToken {
    pub model: String,
    pub serial: Option<String>,
    pub size: u64,
}

impl IdentityToken {
    pub fn matches(&self, device: &DeviceInfo) -> bool {
        self.size == device.size && self.model == device.model && self.serial == device.serial
    }
}
