use thiserror::Error;

use crate::device::{Bus, DeviceInfo};

/// Reasons a device must never be used as a write target.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum SafetyError {
    #[error("device hosts the running operating system")]
    SystemDisk,

    #[error("device is read-only")]
    ReadOnly,

    #[error("device bus `{0}` is not removable media")]
    NotRemovable(Bus),

    #[error("device size is unknown (0 bytes)")]
    UnknownSize,

    #[error("image ({required} bytes) is larger than the device ({available} bytes)")]
    TooSmall { required: u64, available: u64 },

    #[error("image is empty")]
    EmptyImage,
}

/// Enforce the non-negotiable safety invariants before any write.
///
/// This is intentionally conservative: unknown buses are rejected unless the
/// platform explicitly reports the media as removable.
pub fn check_target(device: &DeviceInfo, image_len: u64) -> Result<(), SafetyError> {
    if image_len == 0 {
        return Err(SafetyError::EmptyImage);
    }
    if device.is_system {
        return Err(SafetyError::SystemDisk);
    }
    if device.read_only {
        return Err(SafetyError::ReadOnly);
    }
    if matches!(device.bus, Bus::Internal | Bus::Virtual | Bus::Network) {
        return Err(SafetyError::NotRemovable(device.bus));
    }
    if device.bus == Bus::Unknown && !device.removable {
        return Err(SafetyError::NotRemovable(device.bus));
    }
    if device.size == 0 {
        return Err(SafetyError::UnknownSize);
    }
    if image_len > device.size {
        return Err(SafetyError::TooSmall {
            required: image_len,
            available: device.size,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::DeviceId;

    fn dev(bus: Bus, removable: bool, system: bool, size: u64) -> DeviceInfo {
        DeviceInfo {
            id: DeviceId::from("/dev/disk9"),
            display_name: "test".into(),
            model: "test".into(),
            serial: None,
            size,
            bus,
            removable,
            mount_points: vec![],
            is_system: system,
            read_only: false,
        }
    }

    #[test]
    fn rejects_system_disk() {
        let d = dev(Bus::Usb, true, true, 8 << 30);
        assert_eq!(check_target(&d, 1 << 20), Err(SafetyError::SystemDisk));
    }

    #[test]
    fn rejects_internal_bus_even_if_marked_removable() {
        let d = dev(Bus::Internal, true, false, 8 << 30);
        assert!(matches!(
            check_target(&d, 1 << 20),
            Err(SafetyError::NotRemovable(Bus::Internal))
        ));
    }

    #[test]
    fn rejects_unknown_bus_when_not_removable() {
        let d = dev(Bus::Unknown, false, false, 8 << 30);
        assert!(matches!(
            check_target(&d, 1 << 20),
            Err(SafetyError::NotRemovable(Bus::Unknown))
        ));
    }

    #[test]
    fn accepts_usb_with_enough_space() {
        let d = dev(Bus::Usb, true, false, 8 << 30);
        assert_eq!(check_target(&d, 4 << 30), Ok(()));
    }

    #[test]
    fn rejects_image_larger_than_device() {
        let d = dev(Bus::Usb, true, false, 1 << 30);
        assert!(matches!(
            check_target(&d, 2 << 30),
            Err(SafetyError::TooSmall { .. })
        ));
    }

    #[test]
    fn rejects_empty_image() {
        let d = dev(Bus::Usb, true, false, 1 << 30);
        assert_eq!(check_target(&d, 0), Err(SafetyError::EmptyImage));
    }
}
