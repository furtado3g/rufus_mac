use thiserror::Error;

use crate::safety::SafetyError;

/// Domain error shared by all image2usb crates.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum Error {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("permission denied: {0}")]
    Permission(String),

    #[error("device not found: {0}")]
    DeviceNotFound(String),

    #[error("device identity changed since enumeration (expected {expected}, found {found})")]
    DeviceChanged { expected: String, found: String },

    #[error("unsafe target: {0}")]
    UnsafeTarget(#[from] SafetyError),

    #[error("image is empty")]
    EmptyImage,

    #[error("image ({required} bytes) does not fit on the device ({available} bytes)")]
    ImageTooLarge { required: u64, available: u64 },

    #[error(
        "not enough space for extraction: files and directories need about {required} bytes, \
         but the partition data area has {available} bytes"
    )]
    ExtractionTooLarge { required: u64, available: u64 },

    #[error("operation cancelled")]
    Cancelled,

    #[error("verification failed for {path}: expected {expected} bytes, found {actual}")]
    ExtractedFileMismatch {
        path: String,
        expected: u64,
        actual: u64,
    },

    #[error(
        "verification failed at byte offset {offset} (expected 0x{expected:02x}, found 0x{actual:02x})"
    )]
    VerifyMismatch {
        offset: u64,
        expected: u8,
        actual: u8,
    },

    #[error("unsupported: {0}")]
    Unsupported(String),

    #[error("invalid image: {0}")]
    InvalidImage(String),

    #[error("external command `{program}` failed with exit code {code}: {stderr}")]
    Command {
        program: String,
        code: i32,
        stderr: String,
    },

    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, Error>;
