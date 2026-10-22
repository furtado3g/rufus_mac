//! Platform-specific backends.
//!
//! Each backend implements [`image2usb_core::Platform`] using the native raw
//! device API of the target operating system:
//!
//! | OS | Device node | Exclusive access |
//! | --- | --- | --- |
//! | macOS | `/dev/rdiskN` | `diskutil unmountDisk` |
//! | Linux | `/dev/sdX` | `umount2` + `O_EXCL` |
//! | Windows | `\\.\PhysicalDriveN` | `FSCTL_LOCK_VOLUME` + `FSCTL_DISMOUNT_VOLUME` |

#![deny(unsafe_op_in_unsafe_fn)]

use image2usb_core::{Error, Platform, Result};

#[cfg(all(feature = "windows-media", target_os = "macos"))]
pub mod media;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;

/// Return the backend for the current operating system.
pub fn native() -> Box<dyn Platform> {
    #[cfg(target_os = "macos")]
    {
        Box::new(macos::MacPlatform)
    }
    #[cfg(target_os = "linux")]
    {
        Box::new(linux::LinuxPlatform)
    }
    #[cfg(target_os = "windows")]
    {
        Box::new(windows::WindowsPlatform)
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        Box::new(UnsupportedPlatform)
    }
}

/// Fallback used on platforms without a backend.
pub struct UnsupportedPlatform;

impl Platform for UnsupportedPlatform {
    fn name(&self) -> &'static str {
        std::env::consts::OS
    }

    fn list_devices(&self) -> Result<Vec<image2usb_core::DeviceInfo>> {
        Err(Error::Unsupported(format!(
            "image2usb does not support {} yet",
            std::env::consts::OS
        )))
    }

    fn open_exclusive(
        &self,
        _target: &image2usb_core::DeviceInfo,
    ) -> Result<Box<dyn image2usb_core::RawIo>> {
        Err(Error::Unsupported(
            "platform backend unavailable".to_owned(),
        ))
    }

    fn eject(&self, _target: &image2usb_core::DeviceInfo) -> Result<()> {
        Ok(())
    }
}
