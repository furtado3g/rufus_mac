//! Shared, platform-independent domain model for image2usb.
//!
//! Everything in this crate is safe Rust and has no knowledge of the operating
//! system it runs on. Platform specifics live in `image2usb-platform`.

#![forbid(unsafe_code)]

pub mod cancel;
pub mod device;
pub mod error;
pub mod io;
pub mod plan;
pub mod progress;
pub mod safety;
pub mod util;
pub mod write;

pub use cancel::CancelToken;
pub use device::{Bus, DeviceId, DeviceInfo, IdentityToken};
pub use error::{Error, Result};
pub use io::{FileDevice, RawIo};
pub use plan::{
    FileSystem, PartitionScheme, ResolvedMode, VerifyMode, WriteMode, WriteOptions, WritePlan,
};
pub use progress::{CallbackSink, ChannelSink, NoopSink, Phase, ProgressEvent, ProgressSink};
pub use safety::{SafetyError, check_target};
pub use write::{VerifyStats, WriteStats, verify_image, write_image};

/// A platform backend: device discovery, exclusive raw access and ejection.
///
/// Implementations are expected to re-enumerate devices and compare the
/// [`IdentityToken`] before opening a device, to avoid time-of-check /
/// time-of-use races.
pub trait Platform: Send + Sync {
    /// Short platform name, e.g. `macos`, `linux`, `windows`.
    fn name(&self) -> &'static str;

    /// List candidate block devices. Implementations may include internal
    /// devices, but must flag them with [`DeviceInfo::is_system`].
    fn list_devices(&self) -> Result<Vec<DeviceInfo>>;

    /// Open a device for exclusive raw I/O.
    ///
    /// Implementations must unmount volumes, take an exclusive lock when the
    /// platform supports it, and verify the identity token first.
    fn open_exclusive(&self, target: &DeviceInfo) -> Result<Box<dyn RawIo>>;

    /// Eject the media after the [`RawIo`] handle has been dropped.
    fn eject(&self, target: &DeviceInfo) -> Result<()>;
}
