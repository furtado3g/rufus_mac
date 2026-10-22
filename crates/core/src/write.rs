use std::io::Read;
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};

use crate::cancel::CancelToken;
use crate::error::{Error, Result};
use crate::io::RawIo;
use crate::progress::{Phase, ProgressEvent, ProgressSink};
use crate::util::align_up;

/// Chunk size used for raw writes. A multiple of every supported sector size.
pub const CHUNK_SIZE: usize = 4 * 1024 * 1024;

/// Minimum interval between progress reports.
const REPORT_INTERVAL: Duration = Duration::from_millis(100);

#[derive(Debug, Clone)]
pub struct WriteStats {
    pub bytes_written: u64,
    /// SHA-256 of the image bytes actually written (padding excluded).
    pub sha256: String,
    pub elapsed: Duration,
}

#[derive(Debug, Clone)]
pub struct VerifyStats {
    pub bytes_compared: u64,
    pub elapsed: Duration,
}

/// Write `image_len` bytes from `image` to `device`, sector-aligned.
///
/// The final partial chunk is zero-padded to a sector boundary because raw
/// devices reject unaligned writes. Padding is not included in the hash nor in
/// the reported byte count.
pub fn write_image(
    image: &mut dyn Read,
    image_len: u64,
    device: &mut dyn RawIo,
    cancel: &CancelToken,
    sink: &dyn ProgressSink,
) -> Result<WriteStats> {
    if image_len == 0 {
        return Err(Error::EmptyImage);
    }
    if image_len > device.size() {
        return Err(Error::ImageTooLarge {
            required: image_len,
            available: device.size(),
        });
    }

    let sector = (device.logical_sector_size().max(512) as usize).next_power_of_two();
    let chunk = CHUNK_SIZE.max(sector * 4);
    // Over-allocate so the slice used for I/O can start at a sector boundary.
    let mut raw = vec![0u8; chunk + sector];
    let offset_in_buf = raw.as_ptr().align_offset(sector);
    let buf = &mut raw[offset_in_buf..offset_in_buf + chunk];

    let started = Instant::now();
    let mut last_report = started.checked_sub(REPORT_INTERVAL).unwrap_or(started);
    let mut hasher = Sha256::new();
    let mut done: u64 = 0;
    let mut device_offset: u64 = 0;

    sink.report(ProgressEvent {
        phase: Phase::Writing,
        done_bytes: 0,
        total_bytes: image_len,
        bytes_per_sec: None,
        eta: None,
        message: None,
        target_incomplete: false,
    });

    while done < image_len {
        cancel.check()?;

        let want = ((image_len - done) as usize).min(chunk);
        image.read_exact(&mut buf[..want])?;

        let padded = align_up(want, sector);
        if padded > want {
            buf[want..padded].fill(0);
        }

        if device_offset + padded as u64 > device.size() {
            return Err(Error::Unsupported(format!(
                "image is not sector-aligned and cannot be padded at the end of the device \
                 (offset {device_offset}, padded length {padded}, device size {})",
                device.size()
            )));
        }

        device.write_all_at(device_offset, &buf[..padded])?;
        hasher.update(&buf[..want]);

        done += want as u64;
        device_offset += padded as u64;

        let now = Instant::now();
        if now.duration_since(last_report) >= REPORT_INTERVAL {
            report_progress(sink, done, image_len, started, &mut last_report);
        }
    }

    device.sync()?;

    let elapsed = started.elapsed();
    sink.report(ProgressEvent {
        phase: Phase::Writing,
        done_bytes: image_len,
        total_bytes: image_len,
        bytes_per_sec: bytes_per_sec(image_len, elapsed),
        eta: Some(Duration::ZERO),
        message: None,
        target_incomplete: false,
    });

    Ok(WriteStats {
        bytes_written: image_len,
        sha256: hex::encode(hasher.finalize()),
        elapsed,
    })
}

/// Read the device back and compare it with the image, byte for byte.
pub fn verify_image(
    image: &mut dyn Read,
    image_len: u64,
    device: &mut dyn RawIo,
    cancel: &CancelToken,
    sink: &dyn ProgressSink,
) -> Result<VerifyStats> {
    if image_len == 0 {
        return Err(Error::EmptyImage);
    }
    if image_len > device.size() {
        return Err(Error::ImageTooLarge {
            required: image_len,
            available: device.size(),
        });
    }

    let sector = (device.logical_sector_size().max(512) as usize).next_power_of_two();
    let chunk = CHUNK_SIZE.max(sector * 4);
    let mut image_raw = vec![0u8; chunk + sector];
    let mut device_raw = vec![0u8; chunk + sector];
    let image_off = image_raw.as_ptr().align_offset(sector);
    let device_off = device_raw.as_ptr().align_offset(sector);
    let image_buf = &mut image_raw[image_off..image_off + chunk];
    let device_buf = &mut device_raw[device_off..device_off + chunk];

    let started = Instant::now();
    let mut last_report = started.checked_sub(REPORT_INTERVAL).unwrap_or(started);
    let mut done: u64 = 0;
    let mut offset: u64 = 0;

    while done < image_len {
        cancel.check()?;

        let want = ((image_len - done) as usize).min(chunk);
        image.read_exact(&mut image_buf[..want])?;

        let padded = align_up(want, sector);
        let mut read_len = padded;
        if offset + read_len as u64 > device.size() {
            read_len = want;
        }
        device.read_exact_at(offset, &mut device_buf[..read_len])?;

        if let Some(pos) = image_buf[..want]
            .iter()
            .zip(&device_buf[..want])
            .position(|(a, b)| a != b)
        {
            return Err(Error::VerifyMismatch {
                offset: offset + pos as u64,
                expected: image_buf[pos],
                actual: device_buf[pos],
            });
        }

        done += want as u64;
        offset += padded as u64;

        let now = Instant::now();
        if now.duration_since(last_report) >= REPORT_INTERVAL {
            let elapsed = now.duration_since(started);
            sink.report(ProgressEvent {
                phase: Phase::Verifying,
                done_bytes: done,
                total_bytes: image_len,
                bytes_per_sec: bytes_per_sec(done, elapsed),
                eta: eta(done, image_len, bytes_per_sec(done, elapsed)),
                message: None,
                target_incomplete: false,
            });
            last_report = now;
        }
    }

    let elapsed = started.elapsed();
    sink.report(ProgressEvent {
        phase: Phase::Verifying,
        done_bytes: image_len,
        total_bytes: image_len,
        bytes_per_sec: bytes_per_sec(image_len, elapsed),
        eta: Some(Duration::ZERO),
        message: None,
        target_incomplete: false,
    });

    Ok(VerifyStats {
        bytes_compared: image_len,
        elapsed,
    })
}

fn report_progress(
    sink: &dyn ProgressSink,
    done: u64,
    total: u64,
    started: Instant,
    last_report: &mut Instant,
) {
    let now = Instant::now();
    let elapsed = now.duration_since(started);
    let bps = bytes_per_sec(done, elapsed);
    sink.report(ProgressEvent {
        phase: Phase::Writing,
        done_bytes: done,
        total_bytes: total,
        bytes_per_sec: bps,
        eta: eta(done, total, bps),
        message: None,
        target_incomplete: false,
    });
    *last_report = now;
}

fn bytes_per_sec(done: u64, elapsed: Duration) -> Option<u64> {
    let secs = elapsed.as_secs_f64();
    if secs <= 0.0 {
        None
    } else {
        Some((done as f64 / secs) as u64)
    }
}

fn eta(done: u64, total: u64, bps: Option<u64>) -> Option<Duration> {
    match bps {
        Some(bps) if bps > 0 && total > done => Some(Duration::from_secs((total - done) / bps)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io::FileDevice;
    use crate::progress::NoopSink;
    use std::io::Cursor;

    fn temp_device(len: u64) -> (tempfile::NamedTempFile, FileDevice) {
        let file = tempfile::NamedTempFile::new().unwrap();
        file.as_file().set_len(len).unwrap();
        let dev = FileDevice::open(file.path()).unwrap();
        (file, dev)
    }

    fn pseudo_random(len: usize) -> Vec<u8> {
        (0..len)
            .map(|i| ((i.wrapping_mul(31) ^ (i >> 8)) & 0xff) as u8)
            .collect()
    }

    #[test]
    fn writes_and_verifies() {
        let payload = pseudo_random(3 * 1024 * 1024 + 1234);
        let (file, mut dev) = temp_device(8 * 1024 * 1024);
        let cancel = CancelToken::new();
        let mut cursor = Cursor::new(&payload);

        let stats = write_image(
            &mut cursor,
            payload.len() as u64,
            &mut dev,
            &cancel,
            &NoopSink,
        )
        .unwrap();
        assert_eq!(stats.bytes_written, payload.len() as u64);

        let mut hasher = Sha256::new();
        hasher.update(&payload);
        assert_eq!(stats.sha256, hex::encode(hasher.finalize()));

        // Padded tail must be zero.
        let mut tail = vec![0u8; 512];
        dev.read_exact_at(payload.len() as u64, &mut tail).unwrap();
        assert!(tail.iter().all(|&b| b == 0));

        let mut image = Cursor::new(&payload);
        let stats = verify_image(
            &mut image,
            payload.len() as u64,
            &mut dev,
            &cancel,
            &NoopSink,
        )
        .unwrap();
        assert_eq!(stats.bytes_compared, payload.len() as u64);
        drop(file);
    }

    #[test]
    fn detects_corruption() {
        let payload = pseudo_random(4096);
        let (_file, mut dev) = temp_device(64 * 1024);
        let cancel = CancelToken::new();
        write_image(
            &mut Cursor::new(&payload),
            payload.len() as u64,
            &mut dev,
            &cancel,
            &NoopSink,
        )
        .unwrap();

        dev.write_all_at(100, &[0xAA]).unwrap();
        let err = verify_image(
            &mut Cursor::new(&payload),
            payload.len() as u64,
            &mut dev,
            &cancel,
            &NoopSink,
        )
        .unwrap_err();
        match err {
            Error::VerifyMismatch { offset, .. } => assert_eq!(offset, 100),
            other => panic!("unexpected error: {other}"),
        }
    }

    #[test]
    fn refuses_image_larger_than_device() {
        let (_file, mut dev) = temp_device(4096);
        let cancel = CancelToken::new();
        let err = write_image(
            &mut Cursor::new(vec![0u8; 8192]),
            8192,
            &mut dev,
            &cancel,
            &NoopSink,
        )
        .unwrap_err();
        assert!(matches!(err, Error::ImageTooLarge { .. }));
    }

    #[test]
    fn honours_cancellation() {
        let (_file, mut dev) = temp_device(4096);
        let cancel = CancelToken::new();
        cancel.cancel();
        let err = write_image(
            &mut Cursor::new(vec![0u8; 2048]),
            2048,
            &mut dev,
            &cancel,
            &NoopSink,
        )
        .unwrap_err();
        assert!(matches!(err, Error::Cancelled));
    }
}
