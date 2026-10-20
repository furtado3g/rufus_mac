//! Linux backend: `lsblk` for enumeration, `O_EXCL` raw devices and
//! `umount2` for releasing volumes.

use std::ffi::CString;
use std::fs::{File, OpenOptions};
use std::os::unix::fs::{FileExt, OpenOptionsExt};
use std::os::unix::io::AsRawFd;
use std::process::Command;

use image2usb_core::util::format_bytes;
use image2usb_core::{Bus, DeviceId, DeviceInfo, Error, Platform, RawIo, Result, SafetyError};
use serde_json::Value;

const BLKGETSIZE64: libc::c_ulong = 0x8008_1272;
const BLKSSZGET: libc::c_ulong = 0x0000_1268;
const BLKPBSZGET: libc::c_ulong = 0x0000_127B;
const BLKRRPART: libc::c_ulong = 0x0000_125F;
const BLKFLSBUF: libc::c_ulong = 0x0000_1261;

pub struct LinuxPlatform;

impl Platform for LinuxPlatform {
    fn name(&self) -> &'static str {
        "linux"
    }

    fn list_devices(&self) -> Result<Vec<DeviceInfo>> {
        let json = lsblk_json()?;
        let devices = json
            .get("blockdevices")
            .and_then(Value::as_array)
            .ok_or_else(|| Error::Other("lsblk returned no block devices".to_owned()))?;

        let mut out = Vec::new();
        for device in devices {
            let Some(kind) = device.get("type").and_then(Value::as_str) else {
                continue;
            };
            if kind != "disk" {
                continue;
            }
            let Some(path) = device.get("path").and_then(Value::as_str) else {
                continue;
            };
            let size = device.get("size").and_then(json_u64).unwrap_or(0);
            let model = device
                .get("model")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .unwrap_or("Unknown device")
                .to_owned();
            let serial = device
                .get("serial")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned);
            let transport = device
                .get("tran")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_lowercase();
            let removable = device.get("rm").and_then(json_bool).unwrap_or(false);
            let read_only = device.get("ro").and_then(json_bool).unwrap_or(false);

            let mut mount_points = Vec::new();
            let mut is_system = false;
            collect_mounts(device, &mut mount_points, &mut is_system);

            let bus = classify_bus(&transport, removable);

            out.push(DeviceInfo {
                id: DeviceId(path.to_owned()),
                display_name: format!("{model} ({})", format_bytes(size)),
                model,
                serial,
                size,
                bus,
                removable,
                mount_points,
                is_system,
                read_only,
            });
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

        unmount_all(&current.mount_points)?;

        let path = current.id.as_str().to_owned();
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(libc::O_CLOEXEC | libc::O_EXCL)
            .open(&path)
            .map_err(|err| open_error(&path, err))?;
        let fd = file.as_raw_fd();
        let size = query_size(fd).unwrap_or(current.size);
        let logical = query_u32(fd, BLKSSZGET).unwrap_or(512).max(512);
        let physical = query_u32(fd, BLKPBSZGET).unwrap_or(logical).max(logical);

        Ok(Box::new(LinuxRawIo {
            file,
            size,
            logical,
            physical,
        }))
    }

    fn eject(&self, _target: &DeviceInfo) -> Result<()> {
        // Linux has no portable "eject" verb; unmounting is enough and the
        // user can physically remove the device.
        Ok(())
    }
}

struct LinuxRawIo {
    file: File,
    size: u64,
    logical: u32,
    physical: u32,
}

impl RawIo for LinuxRawIo {
    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<usize> {
        Ok(self.file.read_at(buf, offset)?)
    }

    fn write_at(&mut self, offset: u64, buf: &[u8]) -> Result<usize> {
        Ok(self.file.write_at(buf, offset)?)
    }

    fn size(&self) -> u64 {
        self.size
    }

    fn logical_sector_size(&self) -> u32 {
        self.logical
    }

    fn physical_sector_size(&self) -> u32 {
        self.physical
    }

    fn sync(&mut self) -> Result<()> {
        self.file.sync_all()?;
        let fd = self.file.as_raw_fd();
        // SAFETY: `fd` is valid for the lifetime of the call.
        let rc = unsafe { libc::ioctl(fd, BLKFLSBUF) };
        if rc != 0 {
            tracing::debug!(
                error = %std::io::Error::last_os_error(),
                "BLKFLSBUF failed after fsync"
            );
        }
        Ok(())
    }

    fn invalidate(&mut self) -> Result<()> {
        let fd = self.file.as_raw_fd();
        // SAFETY: `fd` is valid for the lifetime of the call.
        let rc = unsafe { libc::ioctl(fd, BLKRRPART) };
        if rc != 0 {
            let err = std::io::Error::last_os_error();
            if err.raw_os_error() == Some(libc::EBUSY) {
                tracing::warn!(%err, "kernel kept the old partition table; re-plug the device to refresh it");
            } else {
                return Err(Error::Io(err));
            }
        }
        Ok(())
    }
}

fn lsblk_json() -> Result<Value> {
    let columns = "NAME,PATH,TYPE,SIZE,MODEL,SERIAL,TRAN,RM,RO,MOUNTPOINTS";
    let run = |cols: &str| {
        Command::new("lsblk")
            .args(["--json", "-b", "-o", cols])
            .output()
    };
    let output =
        run(columns).or_else(|_| run("NAME,PATH,TYPE,SIZE,MODEL,SERIAL,TRAN,RM,RO,MOUNTPOINT"))?;
    if !output.status.success() {
        return Err(Error::Command {
            program: "lsblk".to_owned(),
            code: output.status.code().unwrap_or(-1),
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        });
    }
    serde_json::from_slice(&output.stdout)
        .map_err(|err| Error::Other(format!("failed to parse lsblk output: {err}")))
}

fn collect_mounts(node: &Value, out: &mut Vec<String>, is_system: &mut bool) {
    if let Some(mounts) = node.get("mountpoints").and_then(Value::as_array) {
        for mount in mounts.iter().filter_map(Value::as_str) {
            if !mount.is_empty() {
                if mount == "/" {
                    *is_system = true;
                }
                out.push(mount.to_owned());
            }
        }
    }
    if let Some(mount) = node.get("mountpoint").and_then(Value::as_str)
        && !mount.is_empty()
    {
        if mount == "/" {
            *is_system = true;
        }
        out.push(mount.to_owned());
    }
    if let Some(children) = node.get("children").and_then(Value::as_array) {
        for child in children {
            collect_mounts(child, out, is_system);
        }
    }
}

fn classify_bus(transport: &str, removable: bool) -> Bus {
    match transport {
        "usb" => Bus::Usb,
        "mmc" => Bus::SdCard,
        "virtio" | "vbd" => Bus::Virtual,
        "nbd" => Bus::Network,
        "" => {
            if removable {
                Bus::Unknown
            } else {
                Bus::Internal
            }
        }
        _ => {
            if removable {
                Bus::Unknown
            } else {
                Bus::Internal
            }
        }
    }
}

fn unmount_all(mount_points: &[String]) -> Result<()> {
    for mount in mount_points {
        let Ok(path) = CString::new(mount.as_str()) else {
            continue;
        };
        // SAFETY: `path` is a valid NUL-terminated string.
        let rc = unsafe { libc::umount2(path.as_ptr(), libc::MNT_DETACH) };
        if rc != 0 {
            let err = std::io::Error::last_os_error();
            match err.raw_os_error() {
                Some(libc::EINVAL) | Some(libc::ENOENT) => {}
                Some(libc::EBUSY) => {
                    return Err(Error::Other(format!(
                        "cannot unmount {mount}: {err}. Close any file manager using the device."
                    )));
                }
                _ => return Err(Error::Io(err)),
            }
        }
    }
    Ok(())
}

fn query_size(fd: libc::c_int) -> Option<u64> {
    let mut size: u64 = 0;
    // SAFETY: the ioctl writes a u64 into a valid local.
    let rc = unsafe { libc::ioctl(fd, BLKGETSIZE64, &mut size) };
    (rc == 0 && size > 0).then_some(size)
}

fn query_u32(fd: libc::c_int, request: libc::c_ulong) -> Option<u32> {
    let mut value: u32 = 0;
    // SAFETY: the ioctl writes a u32 into a valid local.
    let rc = unsafe { libc::ioctl(fd, request, &mut value) };
    (rc == 0 && value > 0).then_some(value)
}

fn open_error(path: &str, err: std::io::Error) -> Error {
    if err.kind() == std::io::ErrorKind::PermissionDenied {
        Error::Permission(format!(
            "cannot open {path}: {err}. Run image2usb as root (sudo)."
        ))
    } else if err.raw_os_error() == Some(libc::EBUSY) {
        Error::Other(format!(
            "{path} is busy (still mounted or in use). Unmount its partitions and try again."
        ))
    } else {
        Error::Io(err)
    }
}

fn json_u64(value: &Value) -> Option<u64> {
    match value {
        Value::Number(n) => n.as_u64(),
        Value::String(s) => s.parse().ok(),
        _ => None,
    }
}

fn json_bool(value: &Value) -> Option<bool> {
    match value {
        Value::Bool(b) => Some(*b),
        Value::Number(n) => n.as_u64().map(|v| v != 0),
        Value::String(s) => Some(s == "1" || s.eq_ignore_ascii_case("true")),
        _ => None,
    }
}
