//! macOS backend: Disk Arbitration via `diskutil`, raw access through
//! `/dev/rdiskN`.

use std::fs::{File, OpenOptions};
use std::os::unix::fs::FileExt;
use std::os::unix::io::AsRawFd;
use std::process::{Command, Output};

use image2usb_core::util::format_bytes;
use image2usb_core::{Bus, DeviceId, DeviceInfo, Error, Platform, RawIo, Result, SafetyError};
use plist::Value;

/// `DKIOCGETBLOCKSIZE` from `<sys/disk.h>`.
const DKIOCGETBLOCKSIZE: libc::c_ulong = 0x4004_6418;
/// `DKIOCGETBLOCKCOUNT`.
const DKIOCGETBLOCKCOUNT: libc::c_ulong = 0x4008_6419;
/// `DKIOCGETPHYSICALBLOCKSIZE`.
const DKIOCGETPHYSICALBLOCKSIZE: libc::c_ulong = 0x4004_6429;
/// `F_FULLFSYNC`, which flushes the drive's own write cache.
const F_FULLFSYNC: libc::c_int = 51;
/// `DKIOCSYNCHRONIZECACHE` from `<sys/disk.h>`.
const DKIOCSYNCHRONIZECACHE: libc::c_ulong = 0x2000_6416;

pub struct MacPlatform;

impl Platform for MacPlatform {
    fn name(&self) -> &'static str {
        "macos"
    }

    fn list_devices(&self) -> Result<Vec<DeviceInfo>> {
        let value = run_plist("diskutil", &["list", "-plist"])?;
        let dict = value
            .as_dictionary()
            .ok_or_else(|| Error::Other("unexpected diskutil list output".to_owned()))?;
        let disks = dict
            .get("AllDisksAndPartitions")
            .and_then(Value::as_array)
            .ok_or_else(|| Error::Other("diskutil list returned no devices".to_owned()))?;

        let root_parent = root_whole_disk();
        let mut out = Vec::new();

        for disk in disks {
            let Some(d) = disk.as_dictionary() else {
                continue;
            };
            let Some(id) = d.get("DeviceIdentifier").and_then(Value::as_string) else {
                continue;
            };
            let mount_points = collect_mount_points(disk);
            let info = match run_plist("diskutil", &["info", "-plist", id]) {
                Ok(value) => value,
                Err(err) => {
                    tracing::warn!(%err, disk = id, "skipping device without info");
                    continue;
                }
            };
            let Some(i) = info.as_dictionary() else {
                continue;
            };

            let size = i
                .get("TotalSize")
                .and_then(Value::as_unsigned_integer)
                .or_else(|| d.get("Size").and_then(Value::as_unsigned_integer))
                .unwrap_or(0);
            let internal = i
                .get("Internal")
                .and_then(Value::as_boolean)
                .unwrap_or(true);
            let removable = i
                .get("RemovableMedia")
                .and_then(Value::as_boolean)
                .unwrap_or(false);
            let read_only = i
                .get("MediaReadOnly")
                .and_then(Value::as_boolean)
                .unwrap_or(false);
            let bus_protocol = i
                .get("BusProtocol")
                .and_then(Value::as_string)
                .unwrap_or_default();
            let virtual_disk = i
                .get("VirtualOrPhysical")
                .and_then(Value::as_string)
                .is_some_and(|v| v == "Virtual");
            let model = i
                .get("MediaName")
                .and_then(Value::as_string)
                .map(str::to_owned)
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "Unknown device".to_owned());

            let bus = classify_bus(bus_protocol, internal, virtual_disk);
            let is_system =
                mount_points.iter().any(|m| m == "/") || root_parent.as_deref() == Some(id);
            let node = i
                .get("DeviceNode")
                .and_then(Value::as_string)
                .map(str::to_owned)
                .unwrap_or_else(|| format!("/dev/{id}"));

            out.push(DeviceInfo {
                id: DeviceId(node),
                display_name: format!("{model} ({})", format_bytes(size)),
                model,
                serial: i
                    .get("SerialNumber")
                    .and_then(Value::as_string)
                    .map(str::to_owned),
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

        let node = current.id.as_str().to_owned();
        if let Err(first) = run("diskutil", &["unmountDisk", &node]) {
            tracing::warn!(%first, node, "unmountDisk failed, retrying with force");
            run("diskutil", &["unmountDisk", "force", &node])?;
        }

        let raw_node = raw_node(&node)?;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&raw_node)
            .map_err(|err| open_error(&raw_node, err))?;
        let fd = file.as_raw_fd();
        let (size, logical, physical) = query_geometry(fd).unwrap_or((current.size, 512, 512));

        Ok(Box::new(MacRawIo {
            file,
            size,
            logical,
            physical,
        }))
    }

    fn eject(&self, target: &DeviceInfo) -> Result<()> {
        run("diskutil", &["eject", target.id.as_str()])?;
        Ok(())
    }
}

struct MacRawIo {
    file: File,
    size: u64,
    logical: u32,
    physical: u32,
}

impl RawIo for MacRawIo {
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
        let fd = self.file.as_raw_fd();
        // `fsync` on a raw disk can fail with ENOTTY on macOS even though every
        // write reached the drive. Try the disk-specific flush ioctls first and
        // only report an error when no flush path succeeded.
        // SAFETY: `fd` is a valid descriptor owned by `self.file`; these calls
        // do not retain it and take no pointer arguments.
        if unsafe { libc::fcntl(fd, F_FULLFSYNC) } == 0 {
            return Ok(());
        }
        let fullfsync = std::io::Error::last_os_error();
        if unsafe { libc::ioctl(fd, DKIOCSYNCHRONIZECACHE) } == 0 {
            return Ok(());
        }
        let ioctl_err = std::io::Error::last_os_error();
        let err = match self.file.sync_all() {
            Ok(()) => {
                tracing::warn!(
                    %fullfsync,
                    %ioctl_err,
                    "raw-disk flush unsupported; fsync succeeded"
                );
                return Ok(());
            }
            Err(err) => err,
        };
        // Raw devices on macOS report ENOTTY for fsync/flush requests they do
        // not implement. The writes themselves completed; closing the device
        // flushes the kernel cache, so this is not a failed write.
        if matches!(
            err.raw_os_error(),
            Some(libc::ENOTTY | libc::EINVAL | libc::ENOTSUP)
        ) {
            tracing::warn!(
                %fullfsync,
                %ioctl_err,
                %err,
                "raw-disk flush unsupported; writes completed without an explicit cache flush"
            );
            return Ok(());
        }
        tracing::warn!(%fullfsync, %ioctl_err, %err, "all raw-disk flush paths failed");
        Err(Error::Io(err))
    }

    fn invalidate(&mut self) -> Result<()> {
        // Raw device writes are visible immediately; `diskutil` re-reads the
        // partition table on the next access.
        Ok(())
    }
}

fn run(program: &str, args: &[&str]) -> Result<Output> {
    let output = Command::new(program)
        .args(args)
        .output()
        .map_err(|err| Error::Other(format!("failed to execute `{program}`: {err}")))?;
    if !output.status.success() {
        return Err(Error::Command {
            program: program.to_owned(),
            code: output.status.code().unwrap_or(-1),
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        });
    }
    Ok(output)
}

fn run_plist(program: &str, args: &[&str]) -> Result<Value> {
    let output = run(program, args)?;
    Value::from_reader(std::io::Cursor::new(output.stdout))
        .map_err(|err| Error::Other(format!("failed to parse `{program}` plist: {err}")))
}

fn collect_mount_points(value: &Value) -> Vec<String> {
    fn walk(value: &Value, out: &mut Vec<String>) {
        let Some(dict) = value.as_dictionary() else {
            return;
        };
        if let Some(mp) = dict.get("MountPoint").and_then(Value::as_string) {
            out.push(mp.to_owned());
        }
        for key in ["Partitions", "APFSVolumes"] {
            if let Some(entries) = dict.get(key).and_then(Value::as_array) {
                for entry in entries {
                    walk(entry, out);
                }
            }
        }
    }
    let mut out = Vec::new();
    walk(value, &mut out);
    out.sort();
    out.dedup();
    out
}

fn root_whole_disk() -> Option<String> {
    let value = run_plist("diskutil", &["info", "-plist", "/"]).ok()?;
    let dict = value.as_dictionary()?;
    dict.get("ParentWholeDisk")
        .and_then(Value::as_string)
        .map(str::to_owned)
}

fn classify_bus(bus_protocol: &str, internal: bool, virtual_disk: bool) -> Bus {
    let protocol = bus_protocol.to_lowercase();
    if virtual_disk {
        Bus::Virtual
    } else if protocol.contains("usb") {
        Bus::Usb
    } else if protocol.contains("secure digital") || protocol == "sd" {
        Bus::SdCard
    } else if internal {
        Bus::Internal
    } else {
        Bus::Unknown
    }
}

fn raw_node(node: &str) -> Result<String> {
    if let Some(rest) = node.strip_prefix("/dev/disk") {
        Ok(format!("/dev/rdisk{rest}"))
    } else if node.starts_with("/dev/rdisk") {
        Ok(node.to_owned())
    } else {
        Err(Error::Unsupported(format!(
            "unexpected macOS device node: {node}"
        )))
    }
}

fn open_error(path: &str, err: std::io::Error) -> Error {
    if err.kind() == std::io::ErrorKind::PermissionDenied {
        Error::Permission(format!(
            "cannot open {path}: {err}. Run image2usb with administrator privileges (sudo)."
        ))
    } else {
        Error::Io(err)
    }
}

fn query_geometry(fd: libc::c_int) -> Option<(u64, u32, u32)> {
    let mut block_size: u32 = 0;
    let mut block_count: u64 = 0;
    // SAFETY: both ioctls write a fixed-size integer into a valid local.
    unsafe {
        if libc::ioctl(fd, DKIOCGETBLOCKSIZE, &mut block_size) != 0 {
            return None;
        }
        if libc::ioctl(fd, DKIOCGETBLOCKCOUNT, &mut block_count) != 0 {
            return None;
        }
    }
    let mut physical: u32 = 0;
    // SAFETY: same as above.
    let physical = if unsafe { libc::ioctl(fd, DKIOCGETPHYSICALBLOCKSIZE, &mut physical) } == 0
        && physical > 0
    {
        physical
    } else {
        block_size
    };
    Some((
        block_count.saturating_mul(block_size as u64),
        block_size,
        physical,
    ))
}
