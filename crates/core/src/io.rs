use std::fs::{File, OpenOptions};
use std::io::{ErrorKind, Read, Seek, SeekFrom, Write};
use std::path::Path;

use crate::error::{Error, Result};

/// Raw, offset-addressed block device.
///
/// Implementations are backed by the platform's raw device node (macOS
/// `/dev/rdiskN`, Linux `/dev/sdX`, Windows `\\.\PhysicalDriveN`) or by a plain
/// file for tests and dry runs.
pub trait RawIo: Send {
    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<usize>;
    fn write_at(&mut self, offset: u64, buf: &[u8]) -> Result<usize>;
    fn size(&self) -> u64;
    fn logical_sector_size(&self) -> u32;

    fn physical_sector_size(&self) -> u32 {
        self.logical_sector_size()
    }

    /// Flush every cache level. Implementations must use the strongest
    /// primitive available (`F_FULLFSYNC`, `FlushFileBuffers`, `fsync`).
    fn sync(&mut self) -> Result<()>;

    /// Ask the OS to re-read the partition table.
    fn invalidate(&mut self) -> Result<()> {
        Ok(())
    }

    fn read_exact_at(&mut self, mut offset: u64, mut buf: &mut [u8]) -> Result<()> {
        while !buf.is_empty() {
            let n = self.read_at(offset, buf)?;
            if n == 0 {
                return Err(Error::Io(std::io::Error::from(ErrorKind::UnexpectedEof)));
            }
            offset += n as u64;
            buf = &mut buf[n..];
        }
        Ok(())
    }

    fn write_all_at(&mut self, mut offset: u64, mut buf: &[u8]) -> Result<()> {
        while !buf.is_empty() {
            let n = self.write_at(offset, buf)?;
            if n == 0 {
                return Err(Error::Io(std::io::Error::from(ErrorKind::WriteZero)));
            }
            offset += n as u64;
            buf = &buf[n..];
        }
        Ok(())
    }
}

/// A [`RawIo`] backed by a regular file.
///
/// Used by tests and by `--dry-run --output <file>` to exercise the real write
/// pipeline without touching a physical device.
#[derive(Debug)]
pub struct FileDevice {
    file: File,
    path: std::path::PathBuf,
    size: u64,
    sector: u32,
}

impl FileDevice {
    /// Open an existing file for reading and writing.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Self::open_with(path, true)
    }

    /// Open an existing file read-only.
    pub fn open_read_only(path: impl AsRef<Path>) -> Result<Self> {
        Self::open_with(path, false)
    }

    fn open_with(path: impl AsRef<Path>, writable: bool) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let file = OpenOptions::new().read(true).write(writable).open(&path)?;
        let size = file.metadata()?.len();
        Ok(Self {
            file,
            path,
            size,
            sector: 512,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl RawIo for FileDevice {
    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<usize> {
        self.file.seek(SeekFrom::Start(offset))?;
        Ok(self.file.read(buf)?)
    }

    fn write_at(&mut self, offset: u64, buf: &[u8]) -> Result<usize> {
        self.file.seek(SeekFrom::Start(offset))?;
        self.file.write_all(buf)?;
        self.file.flush()?;
        Ok(buf.len())
    }

    fn size(&self) -> u64 {
        self.size
    }

    fn logical_sector_size(&self) -> u32 {
        self.sector
    }

    fn sync(&mut self) -> Result<()> {
        self.file.sync_all()?;
        Ok(())
    }
}
