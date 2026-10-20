//! Windows backend.
//!
//! Enumeration and raw I/O use `CreateFileW` + `DeviceIoControl` directly,
//! without SetupAPI or WMI. Volumes on the target disk are locked and
//! dismounted before the physical drive is opened with
//! `FILE_FLAG_NO_BUFFERING | FILE_FLAG_WRITE_THROUGH`.
//!
//! Note: this module cannot be compiled on macOS/Linux. It is kept dependency
//! free (raw `kernel32` FFI) so the surface is small and auditable.

use std::ffi::{OsStr, c_void};
use std::os::windows::ffi::OsStrExt;

use image2usb_core::util::format_bytes;
use image2usb_core::{Bus, DeviceId, DeviceInfo, Error, Platform, RawIo, Result, SafetyError};

type Handle = *mut c_void;

const INVALID_HANDLE_VALUE: Handle = -1isize as Handle;

const GENERIC_READ: u32 = 0x8000_0000;
const GENERIC_WRITE: u32 = 0x4000_0000;
const FILE_SHARE_READ: u32 = 0x0000_0001;
const FILE_SHARE_WRITE: u32 = 0x0000_0002;
const OPEN_EXISTING: u32 = 3;
const FILE_BEGIN: u32 = 0;
const FILE_FLAG_NO_BUFFERING: u32 = 0x2000_0000;
const FILE_FLAG_WRITE_THROUGH: u32 = 0x8000_0000;

const FSCTL_LOCK_VOLUME: u32 = 0x0009_0018;
const FSCTL_DISMOUNT_VOLUME: u32 = 0x0009_0020;
const IOCTL_DISK_GET_LENGTH_INFO: u32 = 0x0007_405C;
const IOCTL_DISK_GET_DRIVE_GEOMETRY_EX: u32 = 0x0007_00A0;
const IOCTL_DISK_UPDATE_PROPERTIES: u32 = 0x0007_0140;
const IOCTL_STORAGE_QUERY_PROPERTY: u32 = 0x002D_1400;
const IOCTL_STORAGE_EJECT_MEDIA: u32 = 0x002D_4808;
const IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS: u32 = 0x0056_0000;

const BUS_TYPE_USB: u32 = 7;
const BUS_TYPE_SD: u32 = 12;
const BUS_TYPE_MMC: u32 = 13;
const BUS_TYPE_VIRTUAL: u32 = 14;
const BUS_TYPE_FILE_BACKED_VIRTUAL: u32 = 15;

const ERROR_ACCESS_DENIED: u32 = 5;
const ERROR_SHARING_VIOLATION: u32 = 32;

#[repr(C)]
struct StoragePropertyQuery {
    property_id: u32,
    query_type: u32,
    additional: [u8; 1],
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn CreateFileW(
        file_name: *const u16,
        desired_access: u32,
        share_mode: u32,
        security_attributes: *const c_void,
        creation_disposition: u32,
        flags_and_attributes: u32,
        template_file: Handle,
    ) -> Handle;

    fn DeviceIoControl(
        device: Handle,
        control_code: u32,
        in_buffer: *const c_void,
        in_size: u32,
        out_buffer: *mut c_void,
        out_size: u32,
        bytes_returned: *mut u32,
        overlapped: *mut c_void,
    ) -> i32;

    fn ReadFile(
        file: Handle,
        buffer: *mut u8,
        bytes_to_read: u32,
        bytes_read: *mut u32,
        overlapped: *mut c_void,
    ) -> i32;

    fn WriteFile(
        file: Handle,
        buffer: *const u8,
        bytes_to_write: u32,
        bytes_written: *mut u32,
        overlapped: *mut c_void,
    ) -> i32;

    fn SetFilePointerEx(
        file: Handle,
        distance: i64,
        new_position: *mut i64,
        move_method: u32,
    ) -> i32;

    fn FlushFileBuffers(file: Handle) -> i32;
    fn CloseHandle(object: Handle) -> i32;
    fn GetLastError() -> u32;
    fn GetLogicalDrives() -> u32;
    fn GetWindowsDirectoryW(buffer: *mut u16, size: u32) -> u32;
}

pub struct WindowsPlatform;

impl Platform for WindowsPlatform {
    fn name(&self) -> &'static str {
        "windows"
    }

    fn list_devices(&self) -> Result<Vec<DeviceInfo>> {
        let system_letter = windows_drive_letter();
        let mut out = Vec::new();

        for number in 0..32u32 {
            let path = format!("\\\\.\\PhysicalDrive{number}");
            let handle = open(&path, 0, 0);
            if handle == INVALID_HANDLE_VALUE {
                continue;
            }
            let info = query_device(handle, number, &path, system_letter);
            // SAFETY: `handle` was created by CreateFileW and is owned here.
            unsafe {
                CloseHandle(handle);
            }
            if let Some(info) = info {
                out.push(info);
            }
        }
        Ok(out)
    }

    fn open_exclusive(&self, target: &DeviceInfo) -> Result<Box<dyn RawIo>> {
        let current = self
            .list_devices()?
            .into_iter()
            .find(|d| d.id == target.id)
            .ok_or_else(|| Error::DeviceNotFound(target.id.to_string()))?;
        if !target.identity().matches(&current) {
            return Err(Error::DeviceChanged {
                expected: format!("{} ({} bytes)", target.model, target.size),
                found: format!("{} ({} bytes)", current.model, current.size),
            });
        }
        if current.is_system {
            return Err(Error::UnsafeTarget(SafetyError::SystemDisk));
        }

        let mut volumes = Vec::new();
        for mount in &current.mount_points {
            let volume_path = format!("\\\\.\\{}", mount.trim_end_matches('\\'));
            let volume = open(&volume_path, GENERIC_READ | GENERIC_WRITE, 0);
            if volume == INVALID_HANDLE_VALUE {
                close_all(&volumes);
                return Err(map_windows_error(&volume_path, last_error()));
            }
            if !device_io_control(volume, FSCTL_LOCK_VOLUME, null_in(), 0, null_out(), 0) {
                let err = last_error();
                close_handle(volume);
                close_all(&volumes);
                return Err(map_windows_error(&volume_path, err));
            }
            // Dismounting is best effort: the lock already prevents writes.
            if !device_io_control(volume, FSCTL_DISMOUNT_VOLUME, null_in(), 0, null_out(), 0) {
                tracing::warn!(volume = volume_path, "FSCTL_DISMOUNT_VOLUME failed");
            }
            volumes.push(volume);
        }

        let handle = open(
            current.id.as_str(),
            GENERIC_READ | GENERIC_WRITE,
            FILE_FLAG_NO_BUFFERING | FILE_FLAG_WRITE_THROUGH,
        );
        if handle == INVALID_HANDLE_VALUE {
            let err = last_error();
            close_all(&volumes);
            return Err(map_windows_error(current.id.as_str(), err));
        }

        let size = query_length(handle).unwrap_or(current.size);
        let sector = query_sector_size(handle).unwrap_or(512).max(512);

        Ok(Box::new(WindowsRawIo {
            handle,
            volumes,
            size,
            sector,
        }))
    }

    fn eject(&self, target: &DeviceInfo) -> Result<()> {
        let handle = open(target.id.as_str(), 0, 0);
        if handle == INVALID_HANDLE_VALUE {
            return Ok(());
        }
        let ok = device_io_control(
            handle,
            IOCTL_STORAGE_EJECT_MEDIA,
            null_in(),
            0,
            null_out(),
            0,
        );
        // SAFETY: handle owned by this function.
        unsafe {
            CloseHandle(handle);
        }
        if !ok {
            tracing::debug!("IOCTL_STORAGE_EJECT_MEDIA failed; media can be removed manually");
        }
        Ok(())
    }
}

struct WindowsRawIo {
    handle: Handle,
    volumes: Vec<Handle>,
    size: u64,
    sector: u32,
}

// SAFETY: the raw handles are only used by the thread that owns the struct,
// which is enforced by `&mut self` on every I/O method.
unsafe impl Send for WindowsRawIo {}

impl Drop for WindowsRawIo {
    fn drop(&mut self) {
        close_all(&self.volumes);
        close_handle(self.handle);
    }
}

impl RawIo for WindowsRawIo {
    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<usize> {
        if !seek(self.handle, offset) {
            return Err(Error::Io(last_error()));
        }
        let mut read = 0u32;
        // SAFETY: buffer is valid for `buf.len()` bytes and not overlapped.
        let ok = unsafe {
            ReadFile(
                self.handle,
                buf.as_mut_ptr(),
                buf.len() as u32,
                &mut read,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 {
            return Err(Error::Io(last_error()));
        }
        Ok(read as usize)
    }

    fn write_at(&mut self, offset: u64, buf: &[u8]) -> Result<usize> {
        if !seek(self.handle, offset) {
            return Err(Error::Io(last_error()));
        }
        let mut written = 0u32;
        // SAFETY: buffer is valid for `buf.len()` bytes and not overlapped.
        let ok = unsafe {
            WriteFile(
                self.handle,
                buf.as_ptr(),
                buf.len() as u32,
                &mut written,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 {
            return Err(Error::Io(last_error()));
        }
        Ok(written as usize)
    }

    fn size(&self) -> u64 {
        self.size
    }

    fn logical_sector_size(&self) -> u32 {
        self.sector
    }

    fn physical_sector_size(&self) -> u32 {
        self.sector
    }

    fn sync(&mut self) -> Result<()> {
        // SAFETY: handle is valid until drop.
        if unsafe { FlushFileBuffers(self.handle) } == 0 {
            return Err(Error::Io(last_error()));
        }
        Ok(())
    }

    fn invalidate(&mut self) -> Result<()> {
        device_io_control(
            self.handle,
            IOCTL_DISK_UPDATE_PROPERTIES,
            null_in(),
            0,
            null_out(),
            0,
        );
        Ok(())
    }
}

fn query_device(
    handle: Handle,
    number: u32,
    path: &str,
    system_letter: Option<char>,
) -> Option<DeviceInfo> {
    let size = query_length(handle)?;
    if size == 0 {
        return None;
    }
    let (bus_type, removable, vendor, product, serial) =
        query_descriptor(handle).unwrap_or((0, false, String::new(), String::new(), None));
    let mount_points = mount_points_for_disk(number);
    let is_system = system_letter.is_some_and(|letter| {
        mount_points.iter().any(|m| {
            m.chars()
                .next()
                .is_some_and(|c| c.eq_ignore_ascii_case(&letter))
        })
    });

    let model = match (vendor.is_empty(), product.is_empty()) {
        (false, false) => format!("{vendor} {product}"),
        (true, false) => product,
        _ => "Unknown device".to_owned(),
    };
    let bus = classify_bus(bus_type, removable);

    Some(DeviceInfo {
        id: DeviceId(path.to_owned()),
        display_name: format!("{model} ({})", format_bytes(size)),
        model,
        serial,
        size,
        bus,
        removable,
        mount_points,
        is_system,
        read_only: false,
    })
}

fn query_descriptor(handle: Handle) -> Option<(u32, bool, String, String, Option<String>)> {
    let query = StoragePropertyQuery {
        property_id: 0,
        query_type: 0,
        additional: [0],
    };
    let mut buf = [0u8; 1024];
    let mut returned = 0u32;
    // SAFETY: query and buf are valid for the call duration.
    let ok = unsafe {
        DeviceIoControl(
            handle,
            IOCTL_STORAGE_QUERY_PROPERTY,
            &query as *const _ as *const c_void,
            std::mem::size_of::<StoragePropertyQuery>() as u32,
            buf.as_mut_ptr() as *mut c_void,
            buf.len() as u32,
            &mut returned,
            std::ptr::null_mut(),
        )
    };
    if ok == 0 || returned < 36 {
        return None;
    }
    let removable = buf[10] != 0;
    let bus_type = u32::from_ne_bytes(buf[28..32].try_into().ok()?);
    let vendor = read_cstr(&buf, u32::from_ne_bytes(buf[12..16].try_into().ok()?));
    let product = read_cstr(&buf, u32::from_ne_bytes(buf[16..20].try_into().ok()?));
    let serial = read_cstr(&buf, u32::from_ne_bytes(buf[24..28].try_into().ok()?));
    let serial = (!serial.is_empty()).then_some(serial);
    Some((bus_type, removable, vendor, product, serial))
}

fn read_cstr(buf: &[u8], offset: u32) -> String {
    let start = offset as usize;
    if start == 0 || start >= buf.len() {
        return String::new();
    }
    let end = buf[start..]
        .iter()
        .position(|&b| b == 0)
        .map(|p| start + p)
        .unwrap_or(buf.len());
    String::from_utf8_lossy(&buf[start..end]).trim().to_owned()
}

fn query_length(handle: Handle) -> Option<u64> {
    let mut buf = [0u8; 8];
    let mut returned = 0u32;
    // SAFETY: buf is valid for 8 bytes.
    let ok = unsafe {
        DeviceIoControl(
            handle,
            IOCTL_DISK_GET_LENGTH_INFO,
            null_in(),
            0,
            buf.as_mut_ptr() as *mut c_void,
            buf.len() as u32,
            &mut returned,
            std::ptr::null_mut(),
        )
    };
    if ok == 0 || returned < 8 {
        return None;
    }
    let value = i64::from_ne_bytes(buf);
    (value > 0).then_some(value as u64)
}

fn query_sector_size(handle: Handle) -> Option<u32> {
    let mut buf = [0u8; 256];
    let mut returned = 0u32;
    // SAFETY: buf is valid for 256 bytes.
    let ok = unsafe {
        DeviceIoControl(
            handle,
            IOCTL_DISK_GET_DRIVE_GEOMETRY_EX,
            null_in(),
            0,
            buf.as_mut_ptr() as *mut c_void,
            buf.len() as u32,
            &mut returned,
            std::ptr::null_mut(),
        )
    };
    if ok == 0 || returned < 24 {
        return None;
    }
    let bytes_per_sector = u32::from_ne_bytes(buf[20..24].try_into().ok()?);
    (bytes_per_sector > 0).then_some(bytes_per_sector)
}

fn mount_points_for_disk(number: u32) -> Vec<String> {
    let mut out = Vec::new();
    // SAFETY: GetLogicalDrives has no preconditions.
    let mask = unsafe { GetLogicalDrives() };
    for index in 0..26u32 {
        if mask & (1 << index) == 0 {
            continue;
        }
        let letter = (b'A' + index as u8) as char;
        let volume_path = format!("\\\\.\\{letter}:");
        let handle = open(&volume_path, 0, 0);
        if handle == INVALID_HANDLE_VALUE {
            continue;
        }
        let mut buf = [0u8; 4096];
        let mut returned = 0u32;
        // SAFETY: buf is valid for 4096 bytes.
        let ok = unsafe {
            DeviceIoControl(
                handle,
                IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS,
                null_in(),
                0,
                buf.as_mut_ptr() as *mut c_void,
                buf.len() as u32,
                &mut returned,
                std::ptr::null_mut(),
            )
        };
        if ok != 0 && returned >= 8 {
            let count = u32::from_ne_bytes(buf[0..4].try_into().unwrap_or([0; 4]));
            for i in 0..count.min(16) as usize {
                let base = 8 + i * 24;
                if base + 4 > buf.len() {
                    break;
                }
                let disk = u32::from_ne_bytes(buf[base..base + 4].try_into().unwrap_or([0; 4]));
                if disk == number {
                    out.push(format!("{letter}:\\"));
                    break;
                }
            }
        }
        // SAFETY: handle owned by this function.
        unsafe {
            CloseHandle(handle);
        }
    }
    out.sort();
    out.dedup();
    out
}

fn windows_drive_letter() -> Option<char> {
    let mut buf = [0u16; 260];
    // SAFETY: buffer is valid for 260 UTF-16 code units.
    let len = unsafe { GetWindowsDirectoryW(buf.as_mut_ptr(), buf.len() as u32) };
    if len == 0 {
        return None;
    }
    let path = String::from_utf16_lossy(&buf[..len as usize]);
    path.chars().next()
}

fn classify_bus(bus_type: u32, removable: bool) -> Bus {
    match bus_type {
        BUS_TYPE_USB => Bus::Usb,
        BUS_TYPE_SD | BUS_TYPE_MMC => Bus::SdCard,
        BUS_TYPE_VIRTUAL | BUS_TYPE_FILE_BACKED_VIRTUAL => Bus::Virtual,
        0 if removable => Bus::Unknown,
        0 => Bus::Internal,
        _ => {
            if removable {
                Bus::Unknown
            } else {
                Bus::Internal
            }
        }
    }
}

fn open(path: &str, access: u32, flags: u32) -> Handle {
    let wide: Vec<u16> = OsStr::new(path)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    // SAFETY: `wide` is NUL-terminated and valid for the call. No security
    // attributes and no template handle are used.
    unsafe {
        CreateFileW(
            wide.as_ptr(),
            access,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            std::ptr::null(),
            OPEN_EXISTING,
            flags,
            std::ptr::null_mut(),
        )
    }
}

fn device_io_control(
    handle: Handle,
    code: u32,
    in_buffer: *const c_void,
    in_size: u32,
    out_buffer: *mut c_void,
    out_size: u32,
) -> bool {
    let mut returned = 0u32;
    // SAFETY: buffers are supplied by the caller with the given sizes.
    let ok = unsafe {
        DeviceIoControl(
            handle,
            code,
            in_buffer,
            in_size,
            out_buffer,
            out_size,
            &mut returned,
            std::ptr::null_mut(),
        )
    };
    ok != 0
}

fn seek(handle: Handle, offset: u64) -> bool {
    // SAFETY: handle is valid; `offset` fits in i64 for any real device.
    let ok = unsafe { SetFilePointerEx(handle, offset as i64, std::ptr::null_mut(), FILE_BEGIN) };
    ok != 0
}

fn close_handle(handle: Handle) {
    if handle != INVALID_HANDLE_VALUE {
        // SAFETY: caller transfers ownership of the handle.
        unsafe {
            CloseHandle(handle);
        }
    }
}

fn close_all(handles: &[Handle]) {
    for &handle in handles {
        close_handle(handle);
    }
}

fn last_error() -> std::io::Error {
    // SAFETY: GetLastError has no preconditions.
    std::io::Error::from_raw_os_error(unsafe { GetLastError() } as i32)
}

fn map_windows_error(path: &str, err: std::io::Error) -> Error {
    match err.raw_os_error().map(|code| code as u32) {
        Some(ERROR_ACCESS_DENIED) => Error::Permission(format!(
            "cannot open {path}: {err}. Run image2usb as administrator."
        )),
        Some(ERROR_SHARING_VIOLATION) => Error::Other(format!(
            "{path} is in use. Close Explorer windows, antivirus scans and any program using the device."
        )),
        _ => Error::Io(err),
    }
}

fn null_in() -> *const c_void {
    std::ptr::null()
}

fn null_out() -> *mut c_void {
    std::ptr::null_mut()
}
