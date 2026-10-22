#![cfg(all(feature = "windows-media", target_os = "macos"))]

use std::ffi::{CStr, c_char};

#[test]
fn udf_catalog_and_reads_match_native_image() {
    use image2usb_platform::media::Udf;
    use std::{fs, process::Command};
    let tmp = tempfile::tempdir().unwrap();
    let source = tmp.path().join("source");
    fs::create_dir_all(source.join("sources")).unwrap();
    fs::write(source.join("sources/install.wim"), b"fixture bytes").unwrap();
    let image = tmp.path().join("fixture.iso");
    let output = Command::new("/usr/bin/hdiutil")
        .args(["makehybrid", "-udf", "-o"])
        .arg(&image)
        .arg(&source)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let mut udf = Udf::open(fs::File::open(image).unwrap()).unwrap();
    let entries = udf.read_dir("").unwrap();
    assert!(
        entries
            .iter()
            .any(|entry| entry.name == "sources" && entry.is_dir)
    );
    let entries = udf.read_dir("sources").unwrap();
    assert!(
        entries
            .iter()
            .any(|entry| entry.name == "install.wim" && !entry.is_dir)
    );
    let mut bytes = [0; 32];
    let count = udf.read_at("sources/install.wim", 0, &mut bytes).unwrap();
    assert_eq!(&bytes[..count], b"fixture bytes");
    assert_eq!(
        udf.read_at("sources/install.wim", 13, &mut bytes).unwrap(),
        0
    );
    assert!(udf.read_at("../install.wim", 0, &mut bytes).is_err());
    assert!(udf.read_dir("/sources").is_err());
}

#[test]
fn udf_fragmented_sparse_file_reads_back() {
    use image2usb_platform::media::Udf;
    use std::{
        fs::File,
        os::unix::fs::{FileExt, MetadataExt},
        path::PathBuf,
        process::{Command, Stdio},
    };

    struct MountGuard {
        mount: PathBuf,
        active: bool,
    }
    impl Drop for MountGuard {
        fn drop(&mut self) {
            if self.active {
                let _ = Command::new("/usr/bin/hdiutil")
                    .arg("detach")
                    .arg(&self.mount)
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status();
            }
        }
    }

    let tmp = tempfile::tempdir().unwrap();
    let image = tmp.path().join("fragmented.img");
    let file = File::create(&image).unwrap();
    file.set_len(64 * 1024 * 1024).unwrap();
    drop(file);
    let output = Command::new("/sbin/newfs_udf")
        .args(["-b", "2048", "-v", "I2UFRAG"])
        .arg(&image)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let mount = tmp.path().join("mnt");
    std::fs::create_dir(&mount).unwrap();
    let output = Command::new("/usr/bin/hdiutil")
        .args(["attach", "-readwrite", "-noverify", "-mountpoint"])
        .arg(&mount)
        .args(["-imagekey", "diskimage-class=CRawDiskImage"])
        .arg(&image)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let mut guard = MountGuard {
        mount: mount.clone(),
        active: true,
    };

    let path = mount.join("frag.bin");
    let file = File::create(&path).unwrap();
    file.write_all_at(&[b'A'; 4096], 1024 * 1024).unwrap();
    file.write_all_at(&[b'B'; 4096], 9 * 1024 * 1024).unwrap();
    file.set_len(16 * 1024 * 1024).unwrap();
    file.sync_all().unwrap();
    let metadata = file.metadata().unwrap();
    assert!(
        metadata.blocks() * 512 < 8 * 1024 * 1024,
        "fixture must be sparse, blocks={}",
        metadata.blocks()
    );
    drop(file);

    let output = Command::new("/usr/bin/hdiutil")
        .arg("detach")
        .arg(&mount)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    guard.active = false;

    let output = Command::new("/sbin/fsck_udf")
        .arg("-n")
        .arg(&image)
        .output()
        .unwrap();
    let report = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.status.success(), "{report}");
    assert!(report.contains("clean"), "{report}");

    let mut udf = Udf::open(File::open(&image).unwrap()).unwrap();
    assert!(
        udf.read_dir("")
            .unwrap()
            .iter()
            .any(|entry| entry.name == "frag.bin" && !entry.is_dir)
    );
    let mut bytes = [0u8; 4];
    assert_eq!(udf.read_at("frag.bin", 0, &mut bytes).unwrap(), 4);
    assert_eq!(bytes, [0; 4]);
    assert_eq!(udf.read_at("frag.bin", 1024 * 1024, &mut bytes).unwrap(), 4);
    assert_eq!(bytes, *b"AAAA");
    assert_eq!(
        udf.read_at("frag.bin", 2 * 1024 * 1024, &mut bytes)
            .unwrap(),
        4
    );
    assert_eq!(bytes, [0; 4]);
    assert_eq!(
        udf.read_at("frag.bin", 9 * 1024 * 1024, &mut bytes)
            .unwrap(),
        4
    );
    assert_eq!(bytes, *b"BBBB");
    assert_eq!(
        udf.read_at("frag.bin", 16 * 1024 * 1024, &mut bytes)
            .unwrap(),
        0
    );
}

#[test]
fn ntfs_writer_creates_directories_and_files() {
    use image2usb_core::{FileDevice, RawIo};
    use image2usb_platform::media::{NtfsFormatOptions, NtfsVolume, format_ntfs};

    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("writer.img");
    const SIZE: u64 = 64 * 1024 * 1024;
    let file = std::fs::File::create(&path).unwrap();
    file.set_len(SIZE).unwrap();
    drop(file);

    let mut io = FileDevice::open(&path).unwrap();
    format_ntfs(&mut io, &NtfsFormatOptions::new("I2U WRITE", 2048)).unwrap();
    {
        let mut volume = NtfsVolume::mount(&mut io).unwrap();
        volume.mkdir("sources").unwrap();
        volume.mkdir("sources/nested").unwrap();
        let payload = vec![0x5a; 1024 * 1024];
        {
            let mut file = volume.create_file("sources/nested/data.bin").unwrap();
            file.write_at(0, &payload).unwrap();
            file.write_at(5 * 1024 * 1024, b"tail").unwrap();
            file.close().unwrap();
        }
        volume.sync().unwrap();
        volume.unmount().unwrap();
    }

    let mut boot = [0u8; 512];
    io.read_exact_at(0, &mut boot).unwrap();
    assert_eq!(&boot[3..11], b"NTFS    ");
}

#[test]
fn ntfs_writer_rejects_unsafe_paths() {
    use image2usb_core::FileDevice;
    use image2usb_platform::media::{NtfsFormatOptions, NtfsVolume, format_ntfs};

    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("paths.img");
    let file = std::fs::File::create(&path).unwrap();
    file.set_len(16 * 1024 * 1024).unwrap();
    drop(file);
    let mut io = FileDevice::open(&path).unwrap();
    format_ntfs(&mut io, &NtfsFormatOptions::new("I2U", 2048)).unwrap();
    let mut volume = NtfsVolume::mount(&mut io).unwrap();
    for bad in [
        "",
        "/absolute",
        "../escape",
        "a//b",
        "a\\b",
        "a:b",
        "trailing.",
        "trailing ",
        "ok/../no",
    ] {
        assert!(volume.mkdir(bad).is_err(), "{bad:?}");
        assert!(volume.create_file(bad).is_err(), "{bad:?}");
    }
    volume.mkdir("ok").unwrap();
    volume.unmount().unwrap();
}

#[test]
fn ntfs_writer_handles_short_io_and_flush() {
    use image2usb_core::{Error, FileDevice, RawIo, Result};
    use image2usb_platform::media::{NtfsFormatOptions, NtfsVolume, format_ntfs};
    use std::os::unix::fs::FileExt;
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };

    struct ShortIo {
        inner: FileDevice,
        max: usize,
        cancel: Arc<AtomicBool>,
        fail_sync: Arc<AtomicBool>,
        syncs: usize,
    }
    impl RawIo for ShortIo {
        fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<usize> {
            if self.cancel.load(Ordering::Relaxed) {
                return Err(Error::Io(std::io::Error::other("cancelled read")));
            }
            let count = buf.len().min(self.max);
            self.inner.read_at(offset, &mut buf[..count])
        }
        fn write_at(&mut self, offset: u64, buf: &[u8]) -> Result<usize> {
            if self.cancel.load(Ordering::Relaxed) {
                return Err(Error::Io(std::io::Error::other("cancelled write")));
            }
            let count = buf.len().min(self.max);
            self.inner.write_at(offset, &buf[..count])
        }
        fn size(&self) -> u64 {
            self.inner.size()
        }
        fn logical_sector_size(&self) -> u32 {
            self.inner.logical_sector_size()
        }
        fn sync(&mut self) -> Result<()> {
            if self.fail_sync.load(Ordering::Relaxed) {
                return Err(Error::Io(std::io::Error::other("cancelled sync")));
            }
            self.syncs += 1;
            self.inner.sync()
        }
    }

    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("short.img");
    let file = std::fs::File::create(&path).unwrap();
    file.set_len(32 * 1024 * 1024).unwrap();
    drop(file);
    let cancel = Arc::new(AtomicBool::new(false));
    let fail_sync = Arc::new(AtomicBool::new(false));
    let mut io = ShortIo {
        inner: FileDevice::open(&path).unwrap(),
        max: 4096,
        cancel: Arc::clone(&cancel),
        fail_sync: Arc::clone(&fail_sync),
        syncs: 0,
    };
    format_ntfs(&mut io, &NtfsFormatOptions::new("I2U SHORT", 2048)).unwrap();
    let payload: Vec<u8> = (0..1024 * 1024).map(|value| value as u8).collect();
    {
        let mut volume = NtfsVolume::mount(&mut io).unwrap();
        volume.mkdir("sources").unwrap();
        let mut file = volume.create_file("sources/pattern.bin").unwrap();
        file.write_at(0, &payload).unwrap();
        file.close().unwrap();
        volume.sync().unwrap();
        volume.unmount().unwrap();
    }
    assert!(io.syncs > 0, "writer must flush through RawIo::sync");

    {
        let mut volume = NtfsVolume::mount(&mut io).unwrap();
        let mut file = volume.open_file("sources/pattern.bin").unwrap();
        let mut bytes = vec![0u8; 1024 * 1024];
        assert_eq!(file.read_at(0, &mut bytes).unwrap(), bytes.len());
        assert_eq!(bytes, payload);
        file.close().unwrap();
        volume.unmount().unwrap();
    }

    {
        let mut volume = NtfsVolume::mount(&mut io).unwrap();
        let mut file = volume.create_file("sources/cancelled.bin").unwrap();
        cancel.store(true, Ordering::Relaxed);
        let error = file.write_at(0, b"must not be written").unwrap_err();
        assert!(error.to_string().contains("cancelled write"), "{error}");
        cancel.store(false, Ordering::Relaxed);
        file.write_at(0, b"ok").unwrap();
        file.close().unwrap();

        fail_sync.store(true, Ordering::Relaxed);
        let error = volume.sync().unwrap_err();
        assert!(error.to_string().contains("cancelled sync"), "{error}");
        fail_sync.store(false, Ordering::Relaxed);
        volume.sync().unwrap();
        volume.unmount().unwrap();
    }

    // Reading the cancelled region still observes the file that was closed.
    let file = std::fs::File::open(&path).unwrap();
    let mut boot = [0u8; 512];
    FileExt::read_exact_at(&file, &mut boot, 0).unwrap();
    assert_eq!(&boot[3..11], b"NTFS    ");
}

#[test]
fn ntfs_writer_handles_files_above_4_gib() {
    use image2usb_core::FileDevice;
    use image2usb_platform::media::{NtfsFormatOptions, NtfsVolume, format_ntfs};

    const IMAGE: u64 = 6 * 1024 * 1024 * 1024;
    const BIG: u64 = 4 * 1024 * 1024 * 1024 + 1024 * 1024;
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("large.img");
    let file = std::fs::File::create(&path).unwrap();
    file.set_len(IMAGE).unwrap();
    drop(file);

    let mut io = FileDevice::open(&path).unwrap();
    format_ntfs(&mut io, &NtfsFormatOptions::new("I2U LARGE", 2048)).unwrap();
    {
        let mut volume = NtfsVolume::mount(&mut io).unwrap();
        volume.mkdir("sources").unwrap();
        let mut file = volume.create_file("sources/big.bin").unwrap();
        file.truncate(BIG).unwrap();
        file.write_at(0, b"HEAD").unwrap();
        file.write_at(BIG - 4, b"TAIL").unwrap();
        file.close().unwrap();
        volume.sync().unwrap();
        volume.unmount().unwrap();
    }

    {
        let mut volume = NtfsVolume::mount(&mut io).unwrap();
        let mut file = volume.open_file("sources/big.bin").unwrap();
        let mut bytes = [0u8; 4];
        assert_eq!(file.read_at(0, &mut bytes).unwrap(), 4);
        assert_eq!(&bytes, b"HEAD");
        assert_eq!(file.read_at(BIG - 4, &mut bytes).unwrap(), 4);
        assert_eq!(&bytes, b"TAIL");
        assert_eq!(file.read_at(BIG, &mut bytes).unwrap(), 0);
        file.close().unwrap();
        volume.unmount().unwrap();
    }
    assert_eq!(std::fs::metadata(&path).unwrap().len(), IMAGE);
}

#[test]
fn ntfs_formatter_serializes_concurrent_calls() {
    use image2usb_core::{FileDevice, RawIo};
    use image2usb_platform::media::{NtfsFormatOptions, format_ntfs};
    let tmp = tempfile::tempdir().unwrap();
    let paths = [tmp.path().join("a.img"), tmp.path().join("b.img")];
    for path in &paths {
        let file = std::fs::File::create(path).unwrap();
        file.set_len(16 * 1024 * 1024).unwrap();
    }
    std::thread::scope(|scope| {
        for path in &paths {
            scope.spawn(move || {
                let mut io = FileDevice::open(path).unwrap();
                format_ntfs(&mut io, &NtfsFormatOptions::new("I2U", 2048)).unwrap();
                let mut boot = [0u8; 512];
                io.read_exact_at(0, &mut boot).unwrap();
                assert_eq!(&boot[3..11], b"NTFS    ");
            });
        }
    });
}

unsafe extern "C" {
    fn udfread_get_version(major: *mut i32, minor: *mut i32, micro: *mut i32);
    static ntfs_home: *const c_char;
}

#[test]
fn ntfs_format_creates_primary_and_backup_boot_sectors() {
    use image2usb_core::{RawIo, Result};
    use image2usb_platform::media::{NtfsFormatOptions, format_ntfs};
    use std::{
        fs::File,
        io::{Read, Seek, SeekFrom, Write},
    };

    struct TestIo {
        file: File,
        size: u64,
        sector: u32,
        writes: usize,
        syncs: usize,
    }
    impl RawIo for TestIo {
        fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<usize> {
            self.file.seek(SeekFrom::Start(offset))?;
            Ok(self.file.read(buf)?)
        }
        fn write_at(&mut self, offset: u64, buf: &[u8]) -> Result<usize> {
            self.writes += 1;
            self.file.seek(SeekFrom::Start(offset))?;
            self.file.write_all(buf)?;
            Ok(buf.len())
        }
        fn size(&self) -> u64 {
            self.size
        }
        fn logical_sector_size(&self) -> u32 {
            self.sector
        }
        fn sync(&mut self) -> Result<()> {
            self.syncs += 1;
            self.file.sync_all()?;
            Ok(())
        }
    }
    fn create(path: &std::path::Path, size: u64, sector: u32) -> TestIo {
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(path)
            .unwrap();
        file.set_len(size).unwrap();
        TestIo {
            file,
            size,
            sector,
            writes: 0,
            syncs: 0,
        }
    }

    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("ntfs-512.img");
    const SIZE: u64 = 16 * 1024 * 1024;
    let mut io = create(&path, SIZE, 512);
    format_ntfs(&mut io, &NtfsFormatOptions::new("I2U TEST", 2048)).unwrap();
    assert!(io.writes > 0);
    assert!(io.syncs > 0, "formatter must flush through RawIo::sync");
    let mut boot = [0u8; 512];
    io.read_at(0, &mut boot).unwrap();
    assert_eq!(&boot[3..11], b"NTFS    ");
    assert_eq!(u16::from_le_bytes([boot[11], boot[12]]), 512);
    assert_ne!(boot[13], 0, "sectors per cluster");
    assert_eq!(
        u32::from_le_bytes(boot[28..32].try_into().unwrap()),
        2048,
        "hidden sectors must preserve the partition offset"
    );
    // mkntfs reserves the final sector for the backup boot sector.
    assert_eq!(
        u64::from_le_bytes(boot[40..48].try_into().unwrap()),
        SIZE / 512 - 1
    );
    assert_eq!(&boot[510..512], &[0x55, 0xaa]);

    let mut backup = [0u8; 512];
    io.read_at(SIZE - 512, &mut backup).unwrap();
    assert_eq!(&backup[3..11], b"NTFS    ");
    assert_eq!(u16::from_le_bytes([backup[11], backup[12]]), 512);
    assert_eq!(&backup[510..512], &[0x55, 0xaa]);

    // The same formatter must handle a 4Kn device without changing the offset
    // interpretation. 64 MiB keeps the metadata work small.
    let path = tmp.path().join("ntfs-4096.img");
    const SIZE_4K: u64 = 64 * 1024 * 1024;
    let mut io = create(&path, SIZE_4K, 4096);
    format_ntfs(&mut io, &NtfsFormatOptions::new("I2U 4KN", 2048)).unwrap();
    io.read_at(0, &mut boot).unwrap();
    assert_eq!(&boot[3..11], b"NTFS    ");
    assert_eq!(u16::from_le_bytes([boot[11], boot[12]]), 4096);
    assert_eq!(
        u64::from_le_bytes(boot[40..48].try_into().unwrap()),
        SIZE_4K / 4096 - 1
    );
}

#[test]
fn ntfs_format_rejects_invalid_input_and_propagates_write_errors() {
    use image2usb_core::{Error, RawIo, Result};
    use image2usb_platform::media::{NtfsFormatOptions, format_ntfs};
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    struct FailingIo {
        size: u64,
        writes: Arc<AtomicUsize>,
    }
    impl RawIo for FailingIo {
        fn read_at(&mut self, _offset: u64, buf: &mut [u8]) -> Result<usize> {
            buf.fill(0);
            Ok(buf.len())
        }
        fn write_at(&mut self, _offset: u64, _buf: &[u8]) -> Result<usize> {
            self.writes.fetch_add(1, Ordering::Relaxed);
            Err(Error::Io(std::io::Error::other("injected write failure")))
        }
        fn size(&self) -> u64 {
            self.size
        }
        fn logical_sector_size(&self) -> u32 {
            512
        }
        fn sync(&mut self) -> Result<()> {
            Ok(())
        }
    }

    let writes = Arc::new(AtomicUsize::new(0));
    let mut io = FailingIo {
        size: 16 * 1024 * 1024,
        writes: Arc::clone(&writes),
    };
    let error = format_ntfs(&mut io, &NtfsFormatOptions::new("I2U", 2048)).unwrap_err();
    assert!(
        error.to_string().contains("injected write failure"),
        "{error}"
    );
    assert!(writes.load(Ordering::Relaxed) > 0);

    let mut unaligned = FailingIo {
        size: 16 * 1024 * 1024 + 1,
        writes: Arc::clone(&writes),
    };
    assert!(format_ntfs(&mut unaligned, &NtfsFormatOptions::new("I2U", 2048)).is_err());
    let mut tiny = FailingIo {
        size: 1024 * 1024,
        writes,
    };
    assert!(format_ntfs(&mut tiny, &NtfsFormatOptions::new("I2U", 2048)).is_err());
    assert!(format_ntfs(&mut io, &NtfsFormatOptions::new("", 2048)).is_err());
    assert!(format_ntfs(&mut io, &NtfsFormatOptions::new("bad\nlabel", 2048)).is_err());

    struct SyncFailIo {
        size: u64,
    }
    impl RawIo for SyncFailIo {
        fn read_at(&mut self, _offset: u64, buf: &mut [u8]) -> Result<usize> {
            buf.fill(0);
            Ok(buf.len())
        }
        fn write_at(&mut self, _offset: u64, buf: &[u8]) -> Result<usize> {
            Ok(buf.len())
        }
        fn size(&self) -> u64 {
            self.size
        }
        fn logical_sector_size(&self) -> u32 {
            512
        }
        fn sync(&mut self) -> Result<()> {
            Err(Error::Io(std::io::Error::other("injected sync failure")))
        }
    }
    let mut sync_fail = SyncFailIo {
        size: 16 * 1024 * 1024,
    };
    let error = format_ntfs(&mut sync_fail, &NtfsFormatOptions::new("I2U", 2048)).unwrap_err();
    assert!(
        error.to_string().contains("injected sync failure"),
        "{error}"
    );
}

#[test]
fn pinned_native_libraries_link_and_execute() {
    assert_eq!(image2usb_platform::native().name(), "macos");
    let (mut major, mut minor, mut micro) = (0, 0, 0);
    // SAFETY: three valid, writable integers; upstream has no retained pointers.
    unsafe { udfread_get_version(&mut major, &mut minor, &mut micro) };
    assert_eq!((major, minor, micro), (1, 2, 0));
    // SAFETY: ntfs_home is an upstream static pointer to a NUL-terminated string.
    let home = unsafe { CStr::from_ptr(ntfs_home) };
    assert!(home.to_bytes().windows(7).any(|s| s == b"ntfs-3g"));
}
