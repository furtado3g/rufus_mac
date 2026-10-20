//! Offset-limited, cached view of a partition inside a raw device.
//!
//! `fatfs` expects a `Read + Write + Seek` stream whose offset zero is the
//! start of the volume. This adapter translates those accesses to absolute
//! device offsets and caches several windows at once.
//!
//! A single window is not enough: while copying a file `fatfs` alternates
//! between the file data, the FAT (and its mirror), and the directory entry.
//! With one window every such switch flushed and reloaded a megabyte, which
//! reduced large extractions to a few MiB/s. Keeping the hot windows resident
//! makes those switches memory operations.

use std::io::{self, Read, Seek, SeekFrom, Write};

use image2usb_core::RawIo;

/// Cache window size. Windows are aligned to this value inside the partition.
const WINDOW: u64 = 2 * 1024 * 1024;
/// Number of windows kept resident: FAT1, FAT2, data, directory entries and a
/// little headroom for the tree walk.
const CACHE_WINDOWS: usize = 8;

struct Window {
    start: u64,
    len: usize,
    data: Vec<u8>,
    dirty: bool,
    last_used: u64,
}

pub struct PartitionIo<'a> {
    device: &'a mut dyn RawIo,
    start: u64,
    len: u64,
    pos: u64,
    windows: Vec<Window>,
    clock: u64,
}

impl<'a> PartitionIo<'a> {
    pub fn new(device: &'a mut dyn RawIo, start: u64, len: u64) -> Self {
        Self {
            device,
            start,
            len,
            pos: 0,
            windows: Vec::with_capacity(CACHE_WINDOWS),
            clock: 0,
        }
    }

    fn flush_all(&mut self) -> io::Result<()> {
        for index in 0..self.windows.len() {
            if self.windows[index].dirty {
                let start = self.windows[index].start;
                let len = self.windows[index].len;
                self.device
                    .write_all_at(self.start + start, &self.windows[index].data[..len])
                    .map_err(io::Error::other)?;
                self.windows[index].dirty = false;
            }
        }
        Ok(())
    }

    /// Find or load the window containing `pos`, returning its index.
    fn window_for(&mut self, pos: u64) -> io::Result<usize> {
        debug_assert!(pos < self.len);
        let start = pos / WINDOW * WINDOW;
        self.clock += 1;
        let clock = self.clock;

        if let Some(index) = self.windows.iter().position(|window| window.start == start) {
            self.windows[index].last_used = clock;
            return Ok(index);
        }

        let len = (self.len - start).min(WINDOW) as usize;
        let index = if self.windows.len() < CACHE_WINDOWS {
            self.windows.push(Window {
                start,
                len: 0,
                data: Vec::new(),
                dirty: false,
                last_used: clock,
            });
            self.windows.len() - 1
        } else {
            self.windows
                .iter()
                .enumerate()
                .min_by_key(|(_, window)| window.last_used)
                .map(|(index, _)| index)
                .expect("cache is not empty")
        };

        if self.windows[index].dirty {
            let old_start = self.windows[index].start;
            let old_len = self.windows[index].len;
            self.device
                .write_all_at(self.start + old_start, &self.windows[index].data[..old_len])
                .map_err(io::Error::other)?;
        }

        let window = &mut self.windows[index];
        window.start = start;
        window.len = len;
        window.data.resize(len, 0);
        window.dirty = false;
        window.last_used = clock;

        let data = &mut window.data[..len];
        self.device
            .read_exact_at(self.start + start, data)
            .map_err(io::Error::other)?;
        Ok(index)
    }

    /// Flush buffered changes to the device. `fatfs` does not call this.
    pub fn flush(&mut self) -> io::Result<()> {
        self.flush_all()
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
            let index = self.window_for(self.pos)?;
            let window = &self.windows[index];
            let offset = (self.pos - window.start) as usize;
            let n = (window.len - offset).min(want - done);
            buf[done..done + n].copy_from_slice(&window.data[offset..offset + n]);
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
            let index = self.window_for(self.pos)?;
            let window = &mut self.windows[index];
            let offset = (self.pos - window.start) as usize;
            let n = (window.len - offset).min(want - done);
            window.data[offset..offset + n].copy_from_slice(&buf[done..done + n]);
            window.dirty = true;
            self.pos += n as u64;
            done += n;
        }
        Ok(done)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.flush_all()
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
            let mut part = PartitionIo::new(&mut device, 4096, 2 * WINDOW);
            part.seek(SeekFrom::Start(0)).unwrap();
            part.write_all(&[0x11; 4096]).unwrap();
            part.seek(SeekFrom::Start(WINDOW - 16)).unwrap();
            part.write_all(&[0x22; 64]).unwrap();
            part.flush().unwrap();
            part.seek(SeekFrom::Start(0)).unwrap();
            let mut buf = vec![0u8; 4096];
            part.read_exact(&mut buf).unwrap();
            assert!(buf.iter().all(|b| *b == 0x11));
            assert_eq!(part.seek(SeekFrom::End(0)).unwrap(), 2 * WINDOW);
        }
        let bytes = std::fs::read(&path).unwrap();
        assert!(bytes[..4096].iter().all(|b| *b == 0xAA));
        assert!(bytes[4096..8192].iter().all(|b| *b == 0x11));
        let start = 4096 + WINDOW as usize - 16;
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

    #[test]
    fn keeps_several_windows_without_losing_evicted_data() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("disk.img");
        let windows = CACHE_WINDOWS + 3;
        let size = windows as u64 * WINDOW;
        std::fs::write(&path, vec![0u8; size as usize]).unwrap();
        let mut device = FileDevice::open(&path).unwrap();
        {
            let mut part = PartitionIo::new(&mut device, 0, size);
            for window in 0..windows {
                part.seek(SeekFrom::Start(window as u64 * WINDOW + 128))
                    .unwrap();
                part.write_all(&[window as u8 + 1; 64]).unwrap();
            }
            // Reading everything back before an explicit flush proves that
            // evicted dirty windows were written to the device.
            for window in 0..windows {
                part.seek(SeekFrom::Start(window as u64 * WINDOW + 128))
                    .unwrap();
                let mut buf = [0u8; 64];
                part.read_exact(&mut buf).unwrap();
                assert!(buf.iter().all(|b| *b == window as u8 + 1));
            }
            part.flush().unwrap();
        }
    }
}
