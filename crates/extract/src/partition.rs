//! Offset-limited, buffered view of a partition inside a raw device.
//!
//! `fatfs` expects a `Read + Write + Seek` stream whose offset zero is the
//! start of the volume. This adapter translates those accesses to absolute
//! device offsets and coalesces them into 1 MiB windows, so formatting and
//! copying do not turn into one syscall per 512-byte sector.

use std::io::{self, Read, Seek, SeekFrom, Write};

use image2usb_core::RawIo;

const WINDOW: usize = 1024 * 1024;

pub struct PartitionIo<'a> {
    device: &'a mut dyn RawIo,
    start: u64,
    len: u64,
    pos: u64,
    window: Vec<u8>,
    window_start: u64,
    window_len: usize,
    dirty: bool,
}

impl<'a> PartitionIo<'a> {
    pub fn new(device: &'a mut dyn RawIo, start: u64, len: u64) -> Self {
        Self {
            device,
            start,
            len,
            pos: 0,
            window: vec![0u8; WINDOW],
            window_start: 0,
            window_len: 0,
            dirty: false,
        }
    }

    fn flush_window(&mut self) -> io::Result<()> {
        if self.dirty && self.window_len > 0 {
            self.device
                .write_all_at(
                    self.start + self.window_start,
                    &self.window[..self.window_len],
                )
                .map_err(io::Error::other)?;
            self.dirty = false;
        }
        Ok(())
    }

    fn load_window(&mut self, pos: u64) -> io::Result<()> {
        if self.window_len > 0
            && pos >= self.window_start
            && pos < self.window_start + self.window_len as u64
        {
            return Ok(());
        }
        self.flush_window()?;
        let window_start = pos / WINDOW as u64 * WINDOW as u64;
        let window_len = (self.len - window_start).min(WINDOW as u64) as usize;
        self.device
            .read_exact_at(self.start + window_start, &mut self.window[..window_len])
            .map_err(io::Error::other)?;
        self.window_start = window_start;
        self.window_len = window_len;
        self.dirty = false;
        Ok(())
    }

    /// Flush buffered changes to the device. `fatfs` does not call this.
    pub fn flush(&mut self) -> io::Result<()> {
        self.flush_window()
    }
}

impl Read for PartitionIo<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.pos >= self.len {
            return Ok(0);
        }
        let want = buf.len().min((self.len - self.pos) as usize);
        let mut done = 0;
        while done < want {
            self.load_window(self.pos)?;
            let offset = (self.pos - self.window_start) as usize;
            let n = (self.window_len - offset).min(want - done);
            buf[done..done + n].copy_from_slice(&self.window[offset..offset + n]);
            self.pos += n as u64;
            done += n;
        }
        Ok(done)
    }
}

impl Write for PartitionIo<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.pos >= self.len {
            return Err(io::Error::new(
                io::ErrorKind::WriteZero,
                "write past the end of the partition",
            ));
        }
        let want = buf.len().min((self.len - self.pos) as usize);
        let mut done = 0;
        while done < want {
            self.load_window(self.pos)?;
            let offset = (self.pos - self.window_start) as usize;
            let n = (self.window_len - offset).min(want - done);
            self.window[offset..offset + n].copy_from_slice(&buf[done..done + n]);
            self.dirty = true;
            self.pos += n as u64;
            done += n;
        }
        Ok(done)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.flush_window()
    }
}

impl Seek for PartitionIo<'_> {
    fn seek(&mut self, from: SeekFrom) -> io::Result<u64> {
        let target: i128 = match from {
            SeekFrom::Start(offset) => offset as i128,
            SeekFrom::Current(delta) => self.pos as i128 + delta as i128,
            SeekFrom::End(delta) => self.len as i128 + delta as i128,
        };
        if !(0..=self.len as i128).contains(&target) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("seek outside partition: {target}"),
            ));
        }
        self.pos = target as u64;
        Ok(self.pos)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image2usb_core::FileDevice;

    #[test]
    fn reads_and_writes_stay_inside_the_partition() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("disk.img");
        std::fs::write(&path, vec![0xAAu8; 8 * 1024 * 1024]).unwrap();
        let mut device = FileDevice::open(&path).unwrap();
        {
            let mut part = PartitionIo::new(&mut device, 4096, 2 * 1024 * 1024);
            part.seek(SeekFrom::Start(0)).unwrap();
            part.write_all(&[0x11; 4096]).unwrap();
            part.seek(SeekFrom::Start(WINDOW as u64 - 16)).unwrap();
            part.write_all(&[0x22; 64]).unwrap();
            part.flush().unwrap();
            part.seek(SeekFrom::Start(0)).unwrap();
            let mut buf = vec![0u8; 4096];
            part.read_exact(&mut buf).unwrap();
            assert!(buf.iter().all(|b| *b == 0x11));
            assert_eq!(part.seek(SeekFrom::End(0)).unwrap(), 2 * 1024 * 1024);
        }
        let bytes = std::fs::read(&path).unwrap();
        assert!(bytes[..4096].iter().all(|b| *b == 0xAA));
        assert!(bytes[4096..8192].iter().all(|b| *b == 0x11));
        let start = 4096 + WINDOW - 16;
        assert!(bytes[start..start + 64].iter().all(|b| *b == 0x22));
    }

    #[test]
    fn refuses_to_seek_outside() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("disk.img");
        std::fs::write(&path, vec![0u8; 2 * 1024 * 1024]).unwrap();
        let mut device = FileDevice::open(&path).unwrap();
        let mut part = PartitionIo::new(&mut device, 0, 1024 * 1024);
        assert!(part.seek(SeekFrom::End(1)).is_err());
        assert!(part.seek(SeekFrom::Start(2 * 1024 * 1024)).is_err());
    }
}
