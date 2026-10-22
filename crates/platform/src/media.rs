//! Native media access and NTFS volume creation. Never opens a host path or
//! physical device on its own; all I/O goes through caller-supplied `RawIo`.

use std::{
    ffi::{CStr, CString, OsString, c_int, c_void},
    fs::{self, File, OpenOptions},
    io::{self, Read, Seek, SeekFrom},
    os::unix::fs::{FileExt, OpenOptionsExt, PermissionsExt},
    panic::{AssertUnwindSafe, catch_unwind},
    path::{Component, Path, PathBuf},
    process::{Command, ExitStatus, Stdio},
    ptr::NonNull,
    sync::{
        Mutex, MutexGuard,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use image2usb_core::{CancelToken, Error, RawIo, Result};
use sha2::{Digest, Sha256};

#[repr(C)]
struct BlockInput {
    close: Option<unsafe extern "C" fn(*mut BlockInput) -> c_int>,
    read: unsafe extern "C" fn(*mut BlockInput, u32, *mut c_void, u32, c_int) -> c_int,
    size: Option<unsafe extern "C" fn(*mut BlockInput) -> u32>,
}

#[repr(C)]
struct Input {
    api: BlockInput,
    file: File,
    length: u64,
    error: Option<io::Error>,
}

unsafe extern "C" {
    fn udfread_opendir(volume: *mut c_void, path: *const i8) -> *mut c_void;
    fn udfread_readdir(dir: *mut c_void, entry: *mut Dirent) -> *mut Dirent;
    fn udfread_closedir(dir: *mut c_void);
    fn udfread_init() -> *mut c_void;
    fn udfread_open_input(volume: *mut c_void, input: *mut BlockInput) -> c_int;
    fn udfread_close(volume: *mut c_void);
    fn udfread_file_open(volume: *mut c_void, path: *const i8) -> *mut c_void;
    fn udfread_file_close(file: *mut c_void);
    fn udfread_file_size(file: *mut c_void) -> i64;
    fn udfread_file_seek(file: *mut c_void, offset: i64, whence: c_int) -> i64;
    fn udfread_file_read(file: *mut c_void, buffer: *mut c_void, size: usize) -> isize;
    fn i2u_mkntfs_format(
        size: u64,
        sector: u32,
        part_start: u64,
        heads: u32,
        sectors_per_track: u32,
        label: *const i8,
        opaque: *mut c_void,
        read: unsafe extern "C" fn(*mut c_void, u64, *mut c_void, u64) -> i64,
        write: unsafe extern "C" fn(*mut c_void, u64, *const c_void, u64) -> i64,
        sync: unsafe extern "C" fn(*mut c_void) -> c_int,
    ) -> c_int;
    fn i2u_ntfs_mount(
        size: u64,
        sector: u32,
        opaque: *mut c_void,
        read: unsafe extern "C" fn(*mut c_void, u64, *mut c_void, u64) -> i64,
        write: unsafe extern "C" fn(*mut c_void, u64, *const c_void, u64) -> i64,
        sync: unsafe extern "C" fn(*mut c_void) -> c_int,
        volume: *mut *mut c_void,
    ) -> c_int;
    fn i2u_ntfs_mkdir(volume: *mut c_void, path: *const i8) -> c_int;
    fn i2u_ntfs_file_create(
        volume: *mut c_void,
        path: *const i8,
        make_parents: c_int,
    ) -> *mut c_void;
    fn i2u_ntfs_file_open(volume: *mut c_void, path: *const i8) -> *mut c_void;
    fn i2u_ntfs_file_read(file: *mut c_void, offset: u64, buffer: *mut c_void, length: u64) -> i64;
    fn i2u_ntfs_file_write(
        file: *mut c_void,
        offset: u64,
        buffer: *const c_void,
        length: u64,
    ) -> i64;
    fn i2u_ntfs_file_truncate(file: *mut c_void, size: u64) -> c_int;
    fn i2u_ntfs_file_close(file: *mut c_void) -> c_int;
    fn i2u_ntfs_sync(volume: *mut c_void) -> c_int;
    fn i2u_ntfs_umount(volume: *mut c_void) -> c_int;
}

/// Parameters needed by the on-disk NTFS boot sector.
///
/// `partition_start_sector` must be the partition's LBA relative to the whole
/// device, not to the partition. The geometry is written for BIOS boot; UEFI
/// ignores it but Windows setup still expects plausible values.
#[derive(Debug, Clone)]
pub struct NtfsFormatOptions<'a> {
    pub label: &'a str,
    pub partition_start_sector: u32,
    pub heads: u16,
    pub sectors_per_track: u16,
}

impl<'a> NtfsFormatOptions<'a> {
    pub fn new(label: &'a str, partition_start_sector: u32) -> Self {
        Self {
            label,
            partition_start_sector,
            heads: 255,
            sectors_per_track: 63,
        }
    }
}

static NTFS_LOCK: Mutex<()> = Mutex::new(());

struct RawCallbacks<'a> {
    io: &'a mut dyn RawIo,
    error: Option<Error>,
}

fn native_error(callbacks: &mut RawCallbacks<'_>, fallback: &str) -> Error {
    if let Some(error) = callbacks.error.take() {
        return error;
    }
    let error = io::Error::last_os_error();
    if error.raw_os_error() == Some(0) {
        Error::Unsupported(fallback.into())
    } else {
        Error::Io(error)
    }
}

pub struct NtfsVolume<'a> {
    handle: Option<NonNull<c_void>>,
    callbacks: Option<Box<RawCallbacks<'a>>>,
    _lock: MutexGuard<'static, ()>,
}

/// An open NTFS file. It borrows the volume, so the volume cannot be unmounted
/// while the handle is alive. Closing flushes the file's metadata.
pub struct NtfsFile<'v, 'a> {
    handle: Option<NonNull<c_void>>,
    callbacks: &'v mut RawCallbacks<'a>,
}

impl<'a> NtfsVolume<'a> {
    /// Mount a freshly formatted NTFS volume for writing through `RawIo`.
    ///
    /// All NTFS operations are serialized for the lifetime of this object
    /// because libntfs-3g uses process-global caches. The supplied `RawIo` must
    /// be limited to the NTFS partition; offsets are partition-relative.
    pub fn mount(io: &'a mut dyn RawIo) -> Result<Self> {
        let size = io.size();
        let sector = io.logical_sector_size();
        validate_ntfs_geometry(size, sector)?;
        let guard = NTFS_LOCK
            .lock()
            .map_err(|_| Error::Unsupported("NTFS lock poisoned".into()))?;
        let mut callbacks = Box::new(RawCallbacks { io, error: None });
        let mut handle = std::ptr::null_mut();
        // SAFETY: the boxed callback context outlives the mounted volume and is
        // stored in NtfsVolume; the native side never retains other pointers.
        let status = unsafe {
            i2u_ntfs_mount(
                size,
                sector,
                (&raw mut *callbacks).cast(),
                format_read,
                format_write,
                format_sync,
                &mut handle,
            )
        };
        if status != 0 {
            return Err(native_error(&mut callbacks, "NTFS mount failed"));
        }
        let handle = NonNull::new(handle)
            .ok_or_else(|| Error::Unsupported("NTFS mount returned null".into()))?;
        Ok(NtfsVolume {
            handle: Some(handle),
            callbacks: Some(callbacks),
            _lock: guard,
        })
    }

    fn callbacks_mut(&mut self) -> Result<&mut RawCallbacks<'a>> {
        self.callbacks
            .as_deref_mut()
            .ok_or_else(|| Error::Unsupported("NTFS volume already released".into()))
    }

    fn handle(&self) -> Result<NonNull<c_void>> {
        self.handle
            .ok_or_else(|| Error::Unsupported("NTFS volume already released".into()))
    }

    pub fn mkdir(&mut self, path: &str) -> Result<()> {
        let path = ntfs_path(path, false)?;
        let handle = self.handle()?;
        // SAFETY: handle is a live mounted volume and path is NUL-terminated.
        let status = unsafe { i2u_ntfs_mkdir(handle.as_ptr(), path.as_ptr()) };
        if status == 0 {
            Ok(())
        } else {
            Err(native_error(self.callbacks_mut()?, "NTFS mkdir failed"))
        }
    }

    pub fn create_file<'v>(&'v mut self, path: &str) -> Result<NtfsFile<'v, 'a>> {
        let path = ntfs_path(path, false)?;
        let handle = self.handle()?;
        // SAFETY: handle is live; the native side copies the file into its own
        // handle, and the callbacks remain alive through the returned borrow.
        let file = unsafe { i2u_ntfs_file_create(handle.as_ptr(), path.as_ptr(), 1) };
        let callbacks = self.callbacks_mut()?;
        let file = NonNull::new(file)
            .ok_or_else(|| native_error(callbacks, "NTFS file creation failed"))?;
        Ok(NtfsFile {
            handle: Some(file),
            callbacks,
        })
    }

    /// Open an existing regular file for reading.
    pub fn open_file<'v>(&'v mut self, path: &str) -> Result<NtfsFile<'v, 'a>> {
        let path = ntfs_path(path, false)?;
        let handle = self.handle()?;
        // SAFETY: handle is live; the native side copies the file into its own
        // handle, and the callbacks remain alive through the returned borrow.
        let file = unsafe { i2u_ntfs_file_open(handle.as_ptr(), path.as_ptr()) };
        let callbacks = self.callbacks_mut()?;
        let file =
            NonNull::new(file).ok_or_else(|| native_error(callbacks, "NTFS file open failed"))?;
        Ok(NtfsFile {
            handle: Some(file),
            callbacks,
        })
    }

    pub fn sync(&mut self) -> Result<()> {
        let handle = self.handle()?;
        // SAFETY: handle is live.
        let status = unsafe { i2u_ntfs_sync(handle.as_ptr()) };
        if status == 0 {
            Ok(())
        } else {
            Err(native_error(self.callbacks_mut()?, "NTFS sync failed"))
        }
    }

    pub fn unmount(mut self) -> Result<()> {
        self.release()
    }

    fn release(&mut self) -> Result<()> {
        let Some(handle) = self.handle.take() else {
            return Ok(());
        };
        // SAFETY: handle was returned by mount and is not used elsewhere.
        let status = unsafe { i2u_ntfs_umount(handle.as_ptr()) };
        if status == 0 {
            Ok(())
        } else {
            self.handle = Some(handle);
            Err(native_error(self.callbacks_mut()?, "NTFS unmount failed"))
        }
    }
}

impl Drop for NtfsVolume<'_> {
    fn drop(&mut self) {
        if self.release().is_err() {
            // The native volume could not be released. Leak the callback box so
            // any retained native pointer cannot reach freed Rust memory.
            if let Some(callbacks) = self.callbacks.take() {
                std::mem::forget(callbacks);
            }
        }
    }
}

impl NtfsFile<'_, '_> {
    pub fn read_at(&mut self, offset: u64, data: &mut [u8]) -> Result<usize> {
        let handle = self
            .handle
            .ok_or_else(|| Error::Unsupported("NTFS file already closed".into()))?;
        if offset > i64::MAX as u64 || data.len() as u64 > i64::MAX as u64 - offset {
            return Err(Error::Unsupported("NTFS file offset too large".into()));
        }
        // SAFETY: handle is live and data is valid for its length.
        let read = unsafe {
            i2u_ntfs_file_read(
                handle.as_ptr(),
                offset,
                data.as_mut_ptr().cast(),
                data.len() as u64,
            )
        };
        if read < 0 {
            Err(native_error(self.callbacks, "NTFS file read failed"))
        } else {
            Ok(read as usize)
        }
    }

    pub fn write_at(&mut self, offset: u64, data: &[u8]) -> Result<()> {
        let handle = self
            .handle
            .ok_or_else(|| Error::Unsupported("NTFS file already closed".into()))?;
        if offset > i64::MAX as u64 || data.len() as u64 > i64::MAX as u64 - offset {
            return Err(Error::Unsupported("NTFS file offset too large".into()));
        }
        // SAFETY: handle is live and data is valid for its length.
        let written = unsafe {
            i2u_ntfs_file_write(
                handle.as_ptr(),
                offset,
                data.as_ptr().cast(),
                data.len() as u64,
            )
        };
        if written == data.len() as i64 {
            Ok(())
        } else {
            Err(native_error(self.callbacks, "NTFS file write failed"))
        }
    }

    pub fn truncate(&mut self, size: u64) -> Result<()> {
        let handle = self
            .handle
            .ok_or_else(|| Error::Unsupported("NTFS file already closed".into()))?;
        if size > i64::MAX as u64 {
            return Err(Error::Unsupported("NTFS file size too large".into()));
        }
        // SAFETY: handle is live.
        let status = unsafe { i2u_ntfs_file_truncate(handle.as_ptr(), size) };
        if status == 0 {
            Ok(())
        } else {
            Err(native_error(self.callbacks, "NTFS truncate failed"))
        }
    }

    pub fn close(mut self) -> Result<()> {
        self.close_inner()
    }

    fn close_inner(&mut self) -> Result<()> {
        let Some(handle) = self.handle.take() else {
            return Ok(());
        };
        // SAFETY: handle is live and freed by this call.
        let status = unsafe { i2u_ntfs_file_close(handle.as_ptr()) };
        if status == 0 {
            Ok(())
        } else {
            Err(native_error(self.callbacks, "NTFS file close failed"))
        }
    }
}

impl Drop for NtfsFile<'_, '_> {
    fn drop(&mut self) {
        let _ = self.close_inner();
    }
}

/// A private temporary workspace with owner-only permissions.
///
/// The directory is created with a unique name, mode 0700 on Unix, and removed
/// only by its own `Drop`. Files are created exclusively and never overwrite a
/// pre-existing path.
pub struct PrivateWorkspace {
    dir: tempfile::TempDir,
}

impl PrivateWorkspace {
    pub fn new(parent: &Path) -> Result<Self> {
        let dir = tempfile::Builder::new()
            .prefix("image2usb-")
            .tempdir_in(parent)?;
        fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700))?;
        Ok(Self { dir })
    }

    pub fn path(&self) -> &Path {
        self.dir.path()
    }

    /// Create a new file inside the workspace. `name` must be one normal path
    /// component; separators, `..`, absolute paths and NUL are rejected.
    pub fn create_new_file(&self, name: &str) -> Result<File> {
        if name.contains('\0') {
            return Err(Error::Unsupported(
                "workspace file name must not contain NUL".into(),
            ));
        }
        let mut components = Path::new(name).components();
        let Some(Component::Normal(_)) = components.next() else {
            return Err(Error::Unsupported(
                "workspace file name must be one normal component".into(),
            ));
        };
        if components.next().is_some() {
            return Err(Error::Unsupported(
                "workspace file name must be one normal component".into(),
            ));
        }
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(self.dir.path().join(name))?;
        Ok(file)
    }
}

/// A native helper resolved from an absolute private prefix.
///
/// Resolution canonicalizes both paths, rejects symlink escapes outside the
/// prefix, checks executability and verifies the SHA-256 supplied by the
/// release manifest. The helper is always invoked by absolute path with an
/// argument vector; no shell or `PATH` lookup is used.
#[derive(Clone, Debug)]
pub struct Helper {
    path: PathBuf,
    sha256: [u8; 32],
}

/// Result files for one completed helper invocation.
#[derive(Debug)]
pub struct HelperOutput {
    pub stdout: PathBuf,
    pub stderr: PathBuf,
    pub status: ExitStatus,
}

impl HelperOutput {
    /// Read at most `limit` bytes from the end of the helper's stderr.
    pub fn stderr_text(&self, limit: usize) -> io::Result<String> {
        read_tail(&self.stderr, limit)
    }
}

impl Helper {
    pub fn resolve(prefix: &Path, relative: &Path, expected_sha256: &str) -> Result<Self> {
        if !prefix.is_absolute() {
            return Err(Error::Unsupported(
                "helper prefix must be an absolute path".into(),
            ));
        }
        if relative.as_os_str().is_empty()
            || relative.is_absolute()
            || relative
                .components()
                .any(|component| !matches!(component, Component::Normal(_)))
        {
            return Err(Error::Unsupported(
                "helper path must be a relative path without traversal".into(),
            ));
        }
        let prefix = prefix.canonicalize()?;
        let path = prefix.join(relative).canonicalize()?;
        if !path.starts_with(&prefix) {
            return Err(Error::Unsupported(
                "helper path escapes the private prefix".into(),
            ));
        }
        let metadata = fs::metadata(&path)?;
        if !metadata.is_file() {
            return Err(Error::Unsupported("helper is not a regular file".into()));
        }
        if metadata.permissions().mode() & 0o111 == 0 {
            return Err(Error::Permission(format!(
                "helper is not executable: {}",
                path.display()
            )));
        }
        let expected = hex::decode(expected_sha256)
            .map_err(|_| Error::Other("invalid helper SHA-256 encoding".into()))?;
        if expected.len() != 32 {
            return Err(Error::Other(
                "invalid helper SHA-256 length; expected 32 bytes".into(),
            ));
        }
        let actual = sha256_file(&path)?;
        if actual.as_slice() != expected.as_slice() {
            return Err(Error::Other(format!(
                "helper SHA-256 mismatch for {}",
                path.display()
            )));
        }
        Ok(Self {
            path,
            sha256: actual,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Run the helper with separate arguments, no shell, a cleared environment
    /// and output files inside `workspace`.
    ///
    /// Cancellation kills the child and waits for it before returning, so the
    /// caller may safely drop the workspace immediately after any result.
    pub fn run(
        &self,
        workspace: &PrivateWorkspace,
        args: &[OsString],
        cancel: &CancelToken,
    ) -> Result<HelperOutput> {
        cancel.check()?;
        // Re-check the digest immediately before spawning: resolution and use
        // are separated in time and the private prefix could be modified.
        if sha256_file(&self.path)? != self.sha256 {
            return Err(Error::Other(format!(
                "helper SHA-256 changed before execution: {}",
                self.path.display()
            )));
        }
        static SEQUENCE: AtomicU64 = AtomicU64::new(0);
        let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let stem = format!("helper-{}-{sequence}", std::process::id());
        let stdout_path = workspace.path().join(format!("{stem}.stdout"));
        let stderr_path = workspace.path().join(format!("{stem}.stderr"));
        let stdout = workspace.create_new_file(&format!("{stem}.stdout"))?;
        let stderr = workspace.create_new_file(&format!("{stem}.stderr"))?;
        let mut command = Command::new(&self.path);
        command
            .args(args)
            .current_dir(workspace.path())
            .env_clear()
            .env("LC_ALL", "C")
            .env("LANG", "C")
            .stdin(Stdio::null())
            .stdout(Stdio::from(stdout))
            .stderr(Stdio::from(stderr));
        let mut child = command.spawn()?;
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) => {}
                Err(error) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(Error::Io(error));
                }
            }
            if cancel.is_cancelled() {
                let _ = child.kill();
                let _ = child.wait();
                return Err(Error::Cancelled);
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        if !status.success() {
            let stderr = read_tail(&stderr_path, 8192).unwrap_or_default();
            return Err(Error::Command {
                program: self.path.display().to_string(),
                code: status.code().unwrap_or(-1),
                stderr: stderr.trim().to_owned(),
            });
        }
        Ok(HelperOutput {
            stdout: stdout_path,
            stderr: stderr_path,
            status,
        })
    }
}

fn sha256_file(path: &Path) -> io::Result<[u8; 32]> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    Ok(hasher.finalize().into())
}

fn read_tail(path: &Path, limit: usize) -> io::Result<String> {
    if limit == 0 {
        return Ok(String::new());
    }
    let mut file = File::open(path)?;
    let length = file.metadata()?.len();
    if length > limit as u64 {
        file.seek(SeekFrom::End(-(limit as i64)))?;
    }
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

unsafe extern "C" fn format_read(
    ctx: *mut c_void,
    offset: u64,
    buffer: *mut c_void,
    len: u64,
) -> i64 {
    // SAFETY: C passes the live RawCallbacks pointer for the duration of the call.
    let ctx = unsafe { &mut *ctx.cast::<RawCallbacks>() };
    let result = catch_unwind(AssertUnwindSafe(|| {
        let len =
            usize::try_from(len).map_err(|_| Error::Unsupported("NTFS read too large".into()))?;
        if len != 0 && buffer.is_null() {
            return Err(Error::Unsupported("null NTFS read buffer".into()));
        }
        // SAFETY: upstream supplies a writable buffer of len bytes.
        let buffer = unsafe { std::slice::from_raw_parts_mut(buffer.cast::<u8>(), len) };
        ctx.io.read_exact_at(offset, buffer)
    }));
    match result {
        Ok(Ok(())) => i64::try_from(len).unwrap_or(-1),
        Ok(Err(error)) => {
            ctx.error = Some(error);
            -1
        }
        Err(_) => {
            ctx.error = Some(Error::Unsupported("panic in NTFS read".into()));
            -1
        }
    }
}

unsafe extern "C" fn format_write(
    ctx: *mut c_void,
    offset: u64,
    buffer: *const c_void,
    len: u64,
) -> i64 {
    // SAFETY: C passes the live RawCallbacks pointer for the duration of the call.
    let ctx = unsafe { &mut *ctx.cast::<RawCallbacks>() };
    let result = catch_unwind(AssertUnwindSafe(|| {
        let len =
            usize::try_from(len).map_err(|_| Error::Unsupported("NTFS write too large".into()))?;
        if len != 0 && buffer.is_null() {
            return Err(Error::Unsupported("null NTFS write buffer".into()));
        }
        // SAFETY: upstream supplies a readable buffer of len bytes.
        let buffer = unsafe { std::slice::from_raw_parts(buffer.cast::<u8>(), len) };
        ctx.io.write_all_at(offset, buffer)
    }));
    match result {
        Ok(Ok(())) => i64::try_from(len).unwrap_or(-1),
        Ok(Err(error)) => {
            ctx.error = Some(error);
            -1
        }
        Err(_) => {
            ctx.error = Some(Error::Unsupported("panic in NTFS write".into()));
            -1
        }
    }
}

unsafe extern "C" fn format_sync(ctx: *mut c_void) -> c_int {
    // SAFETY: C passes the live RawCallbacks pointer for the duration of the call.
    let ctx = unsafe { &mut *ctx.cast::<RawCallbacks>() };
    match catch_unwind(AssertUnwindSafe(|| ctx.io.sync())) {
        Ok(Ok(())) => 0,
        Ok(Err(error)) => {
            ctx.error = Some(error);
            -1
        }
        Err(_) => {
            ctx.error = Some(Error::Unsupported("panic in NTFS sync".into()));
            -1
        }
    }
}

/// Validate the geometry shared by formatting, mounting and the native shim.
fn validate_ntfs_geometry(size: u64, sector: u32) -> Result<()> {
    if !(512..=4096).contains(&sector)
        || !sector.is_power_of_two()
        || !size.is_multiple_of(u64::from(sector))
        || !(2 * 1024 * 1024..=i64::MAX as u64).contains(&size)
    {
        return Err(Error::Unsupported(
            "invalid NTFS size or logical sector".into(),
        ));
    }
    Ok(())
}

/// Convert a relative, shell-safe path into the NTFS namespace rules.
fn ntfs_path(path: &str, root: bool) -> Result<CString> {
    if !root {
        for name in path.split('/') {
            if name.encode_utf16().count() > 255
                || name.ends_with([' ', '.'])
                || name
                    .bytes()
                    .any(|byte| byte < 0x20 || byte == 0x7f || b"<>:\"|?*".contains(&byte))
            {
                return Err(Error::Unsupported("invalid NTFS path component".into()));
            }
        }
    }
    valid_path(path, root).map_err(Error::Io)
}

/// Format an NTFS volume through the adapted mkntfs formatter.
///
/// The formatter uses only `RawIo` callbacks and never receives a host path,
/// shell or writable file descriptor. Calls are serialized because mkntfs uses
/// global state.
pub fn format_ntfs(io: &mut dyn RawIo, options: &NtfsFormatOptions<'_>) -> Result<()> {
    let label = options.label;
    if label.is_empty()
        || label.len() > 32
        || !label.bytes().all(|b| b.is_ascii_graphic() || b == b' ')
    {
        return Err(Error::Unsupported(
            "NTFS label must be 1-32 printable ASCII bytes".into(),
        ));
    }
    if options.heads == 0 || options.sectors_per_track == 0 {
        return Err(Error::Unsupported("NTFS geometry must be non-zero".into()));
    }
    let label = CString::new(label).map_err(|_| Error::Unsupported("NUL in NTFS label".into()))?;
    let size = io.size();
    let sector = io.logical_sector_size();
    validate_ntfs_geometry(size, sector)?;
    let _guard = NTFS_LOCK
        .lock()
        .map_err(|_| Error::Unsupported("NTFS lock poisoned".into()))?;
    let mut ctx = RawCallbacks { io, error: None };
    // SAFETY: callback context outlives the synchronous native call. The native
    // side copies no pointers and is serialized by FORMAT_LOCK.
    let status = unsafe {
        i2u_mkntfs_format(
            size,
            sector,
            u64::from(options.partition_start_sector),
            u32::from(options.heads),
            u32::from(options.sectors_per_track),
            label.as_ptr(),
            (&raw mut ctx).cast(),
            format_read,
            format_write,
            format_sync,
        )
    };
    match (status, ctx.error.take()) {
        (0, None) => Ok(()),
        (_, Some(error)) => Err(error),
        (_, None) => Err(Error::Unsupported("NTFS formatter failed".into())),
    }
}

#[repr(C)]
struct Dirent {
    kind: u32,
    name: *const i8,
}

/// A validated immediate child; no host filesystem paths or native pointers.
#[derive(Debug)]
pub struct UdfEntry {
    pub name: String,
    pub is_dir: bool,
}

fn valid_path(path: &str, root: bool) -> io::Result<CString> {
    if (path.is_empty() && !root)
        || path.starts_with('/')
        || path.contains(['\\', '\0'])
        || (!path.is_empty()
            && path
                .split('/')
                .any(|s| s.is_empty() || s == "." || s == ".."))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid relative path",
        ));
    }
    CString::new(path).map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "NUL in path"))
}

fn contain_io<T>(
    error: &mut Option<io::Error>,
    panic_message: &str,
    operation: impl FnOnce() -> io::Result<T> + std::panic::UnwindSafe,
) -> Option<T> {
    match catch_unwind(operation) {
        Ok(Ok(value)) => Some(value),
        Ok(Err(failure)) => {
            *error = Some(failure);
            None
        }
        Err(_) => {
            *error = Some(io::Error::other(panic_message));
            None
        }
    }
}

fn read_blocks(file: &File, length: u64, lba: u32, buffer: &mut [u8]) -> io::Result<()> {
    let offset = u64::from(lba) * 2048;
    let end = offset
        .checked_add(buffer.len() as u64)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "UDF block range overflow"))?;
    if end > length {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "UDF block outside ISO",
        ));
    }
    file.read_exact_at(buffer, offset)
}

unsafe extern "C" fn read(
    input: *mut BlockInput,
    lba: u32,
    buffer: *mut c_void,
    blocks: u32,
    _flags: c_int,
) -> c_int {
    // SAFETY: upstream receives this stable boxed Input, whose first field is api.
    // Calls are synchronous; the owning Udf is neither Send nor Sync.
    let input = unsafe { &mut *input.cast::<Input>() };
    let file = &input.file;
    let length = input.length;
    let result = contain_io(&mut input.error, "panic in UDF read callback", || {
        let bytes = usize::try_from(blocks)
            .ok()
            .and_then(|n| n.checked_mul(2048))
            .filter(|&n| n <= isize::MAX as usize)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "UDF read too large"))?;
        if blocks > c_int::MAX as u32 || (bytes != 0 && buffer.is_null()) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid UDF read",
            ));
        }
        if bytes == 0 {
            return Ok(());
        }
        // SAFETY: upstream supplies a writable buffer for the requested blocks.
        let buffer = unsafe { std::slice::from_raw_parts_mut(buffer.cast::<u8>(), bytes) };
        read_blocks(file, length, lba, buffer)
    });
    match result {
        Some(()) => blocks as c_int,
        None => 0,
    }
}

/// Owns a read-only ISO handle and native volume. Native pointers never escape.
pub struct Udf {
    volume: NonNull<c_void>,
    input: Box<Input>,
}

impl Udf {
    /// List immediate children. An empty path means the volume root.
    pub fn read_dir(&mut self, path: &str) -> io::Result<Vec<UdfEntry>> {
        let path = valid_path(path, true)?;
        self.input.error = None;
        // SAFETY: live volume and valid C string; path is not retained.
        let dir = NonNull::new(unsafe { udfread_opendir(self.volume.as_ptr(), path.as_ptr()) })
            .ok_or_else(|| {
                self.input.error.take().unwrap_or_else(|| {
                    io::Error::new(io::ErrorKind::NotFound, "UDF directory not found")
                })
            })?;
        let result = (|| {
            let mut entries = Vec::new();
            loop {
                let mut entry = Dirent {
                    kind: 0,
                    name: std::ptr::null(),
                };
                // SAFETY: live directory and writable ABI-compatible entry.
                if unsafe { udfread_readdir(dir.as_ptr(), &mut entry) }.is_null() {
                    break;
                }
                if entry.name.is_null() || !matches!(entry.kind, 1 | 2) {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "unsupported UDF directory entry",
                    ));
                }
                // SAFETY: upstream guarantees a NUL-terminated name valid until next call.
                // Strict UTF-8 rejects MUTF-8 NUL/surrogate encodings rather than aliasing paths.
                let name = unsafe { CStr::from_ptr(entry.name) }
                    .to_str()
                    .map_err(|_| {
                        io::Error::new(io::ErrorKind::InvalidData, "unsupported UDF name encoding")
                    })?;
                if name == "." || name == ".." {
                    continue;
                }
                valid_path(name, false)?;
                if name.contains('/') || entries.len() >= 250_000 {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "invalid UDF child name or entry limit",
                    ));
                }
                entries.push(UdfEntry {
                    name: name.to_owned(),
                    is_dir: entry.kind == 1,
                });
            }
            Ok(entries)
        })();
        // SAFETY: directory closed exactly once, before the volume.
        unsafe { udfread_closedir(dir.as_ptr()) };
        match self.input.error.take() {
            Some(error) => Err(error),
            None => result,
        }
    }

    pub fn open(file: File) -> io::Result<Self> {
        let length = file.metadata()?.len();
        if length / 2048 > u64::from(u32::MAX) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "ISO exceeds UDF block address range",
            ));
        }
        let mut input = Box::new(Input {
            api: BlockInput {
                close: None,
                read,
                size: None,
            },
            file,
            length,
            error: None,
        });
        // SAFETY: allocation has no arguments; null is handled below.
        let volume = NonNull::new(unsafe { udfread_init() })
            .ok_or_else(|| io::Error::other("UDF allocation failed"))?;
        // SAFETY: both allocations stay alive through close, including failure.
        if unsafe { udfread_open_input(volume.as_ptr(), &mut input.api) } < 0 {
            // SAFETY: close an initialized volume exactly once.
            unsafe { udfread_close(volume.as_ptr()) };
            return Err(input.error.take().unwrap_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidData, "invalid UDF image")
            }));
        }
        Ok(Self { volume, input })
    }

    /// Read a bounded range without loading the complete file into memory.
    pub fn read_at(&mut self, path: &str, offset: u64, buffer: &mut [u8]) -> io::Result<usize> {
        if path.is_empty()
            || path.starts_with('/')
            || path.contains(['\\', '\0'])
            || path
                .split('/')
                .any(|part| part.is_empty() || part == "." || part == "..")
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid UDF relative path",
            ));
        }
        let offset = i64::try_from(offset)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "UDF offset exceeds i64"))?;
        let path = CString::new(path)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "NUL in UDF path"))?;
        self.input.error = None;
        // SAFETY: valid volume and NUL-terminated path, no retained path pointer.
        let handle = unsafe { udfread_file_open(self.volume.as_ptr(), path.as_ptr()) };
        let handle = NonNull::new(handle).ok_or_else(|| {
            self.input
                .error
                .take()
                .unwrap_or_else(|| io::Error::new(io::ErrorKind::NotFound, "UDF file not found"))
        })?;
        // SAFETY: handle is live; buffer has the supplied writable length. The
        // closure contains no panicking operations; handle is closed below.
        let result = unsafe {
            let size = udfread_file_size(handle.as_ptr());
            if size < 0 || offset > size || udfread_file_seek(handle.as_ptr(), offset, 0) != offset
            {
                Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "invalid UDF file range",
                ))
            } else {
                let count =
                    udfread_file_read(handle.as_ptr(), buffer.as_mut_ptr().cast(), buffer.len());
                usize::try_from(count).map_err(|_| io::Error::other("UDF read failed"))
            }
        };
        // SAFETY: live handle, closed exactly once before volume can be dropped.
        unsafe { udfread_file_close(handle.as_ptr()) };
        match self.input.error.take() {
            Some(error) => Err(error),
            None => result,
        }
    }
}

impl Drop for Udf {
    fn drop(&mut self) {
        // SAFETY: volume owns no input; our Box remains alive through close.
        unsafe { udfread_close(self.volume.as_ptr()) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn block_reads_are_bounded_and_use_64_bit_offsets() {
        let file = tempfile::tempfile().unwrap();
        file.set_len(5 * 1024 * 1024 * 1024).unwrap();
        let offset = 4 * 1024 * 1024 * 1024;
        file.write_all_at(b"large", offset).unwrap();
        let mut input = Input {
            api: BlockInput {
                close: None,
                read,
                size: None,
            },
            length: file.metadata().unwrap().len(),
            file,
            error: None,
        };
        let mut buffer = [0; 2048];
        read_blocks(
            &input.file,
            input.length,
            (offset / 2048) as u32,
            &mut buffer,
        )
        .unwrap();
        assert_eq!(&buffer[..5], b"large");
        assert!(read_blocks(&input.file, input.length, u32::MAX, &mut buffer).is_err());
        // SAFETY: valid Input and buffer, matching the upstream callback contract.
        let result = unsafe { read(&mut input.api, u32::MAX, buffer.as_mut_ptr().cast(), 1, 0) };
        assert_eq!(result, 0);
        assert_eq!(
            input.error.take().unwrap().kind(),
            io::ErrorKind::UnexpectedEof
        );
        input.file.set_len(0).unwrap();
        assert!(read_blocks(&input.file, input.length, 0, &mut buffer).is_err());
        input.file.write_all(b"bad").unwrap();
        assert!(Udf::open(input.file).is_err());
    }

    #[test]
    fn udf_callback_panic_and_error_are_contained() {
        let mut error = None;
        let result: Option<()> = contain_io(&mut error, "panic in UDF read callback", || {
            panic!("injected callback panic")
        });
        assert!(result.is_none());
        assert_eq!(
            error.take().unwrap().to_string(),
            "panic in UDF read callback"
        );

        let result: Option<()> = contain_io(&mut error, "panic", || {
            Err(io::Error::new(io::ErrorKind::UnexpectedEof, "short read"))
        });
        assert!(result.is_none());
        assert_eq!(error.take().unwrap().to_string(), "short read");
    }

    #[test]
    fn untrusted_paths_are_rejected() {
        for path in [
            "",
            "/absolute",
            "../parent",
            "a/./b",
            "a//b",
            "a\\b",
            "a\0b",
        ] {
            assert!(valid_path(path, false).is_err(), "{path:?}");
        }
        assert!(valid_path("", true).is_ok());
        assert!(valid_path("sources/install.wim", false).is_ok());
    }

    #[test]
    fn helper_paths_hashes_and_prefix_containment_are_checked() {
        let true_path = Path::new("/usr/bin/true");
        let expected = hex::encode(sha256_file(true_path).unwrap());
        let helper = Helper::resolve(Path::new("/usr/bin"), Path::new("true"), &expected).unwrap();
        assert_eq!(helper.path(), true_path.canonicalize().unwrap());
        assert!(Helper::resolve(Path::new("usr/bin"), Path::new("true"), &expected).is_err());
        assert!(
            Helper::resolve(Path::new("/usr/bin"), Path::new("../bin/true"), &expected).is_err()
        );
        assert!(Helper::resolve(Path::new("/usr/bin"), Path::new("/bin/true"), &expected).is_err());
        assert!(Helper::resolve(Path::new("/usr/bin"), Path::new("true"), "xyz").is_err());
        assert!(
            Helper::resolve(Path::new("/usr/bin"), Path::new("true"), &"00".repeat(32)).is_err()
        );

        let tmp = tempfile::tempdir().unwrap();
        let outside = tmp.path().join("outside");
        fs::create_dir(&outside).unwrap();
        std::os::unix::fs::symlink(true_path, outside.join("true")).unwrap();
        assert!(Helper::resolve(tmp.path(), Path::new("outside/true"), &expected).is_err());
    }

    #[test]
    fn private_workspace_is_exclusive_and_private() {
        let tmp = tempfile::tempdir().unwrap();
        let outside = tmp.path().join("preexisting");
        fs::write(&outside, b"keep").unwrap();
        let workspace = PrivateWorkspace::new(tmp.path()).unwrap();
        let workspace_path = workspace.path().to_path_buf();
        assert_eq!(
            workspace_path.metadata().unwrap().permissions().mode() & 0o777,
            0o700
        );
        let file = workspace.create_new_file("secret").unwrap();
        assert_eq!(
            workspace_path
                .join("secret")
                .metadata()
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        drop(file);
        assert!(workspace.create_new_file("secret").is_err());
        assert!(workspace.create_new_file("../escape").is_err());
        assert!(workspace.create_new_file("a/b").is_err());
        drop(workspace);
        assert!(!workspace_path.exists());
        assert!(outside.exists());
    }

    #[test]
    fn helper_arguments_do_not_use_a_shell_or_path_lookup() {
        let expected = hex::encode(sha256_file(Path::new("/bin/echo")).unwrap());
        let helper = Helper::resolve(Path::new("/bin"), Path::new("echo"), &expected).unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let workspace = PrivateWorkspace::new(tmp.path()).unwrap();
        let marker = "image2usb-shell-injection";
        let argument = format!("safe; touch {marker}; $HOME");
        let output = helper
            .run(
                &workspace,
                &[OsString::from(argument.clone())],
                &CancelToken::new(),
            )
            .unwrap();
        let stdout = fs::read_to_string(&output.stdout).unwrap();
        assert_eq!(stdout.trim_end(), argument);
        assert!(output.stdout.starts_with(workspace.path()));
        assert!(!workspace.path().join(marker).exists());
        assert!(!tmp.path().join(marker).exists());
    }

    #[test]
    fn helper_cancellation_kills_and_waits_before_returning() {
        let expected = hex::encode(sha256_file(Path::new("/bin/sleep")).unwrap());
        let helper = Helper::resolve(Path::new("/bin"), Path::new("sleep"), &expected).unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let workspace = PrivateWorkspace::new(tmp.path()).unwrap();
        let cancel = CancelToken::new();
        let started = std::time::Instant::now();
        std::thread::scope(|scope| {
            let child_cancel = cancel.clone();
            let handle =
                scope.spawn(move || helper.run(&workspace, &[OsString::from("5")], &child_cancel));
            std::thread::sleep(Duration::from_millis(100));
            cancel.cancel();
            let error = handle.join().unwrap().unwrap_err();
            assert!(matches!(error, Error::Cancelled));
        });
        assert!(started.elapsed() < Duration::from_secs(3));
    }

    #[test]
    fn helper_failure_reports_exit_code_and_stderr() {
        let expected = hex::encode(sha256_file(Path::new("/bin/cat")).unwrap());
        let helper = Helper::resolve(Path::new("/bin"), Path::new("cat"), &expected).unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let workspace = PrivateWorkspace::new(tmp.path()).unwrap();
        let error = helper
            .run(
                &workspace,
                &[OsString::from("/definitely-not-an-image2usb-file")],
                &CancelToken::new(),
            )
            .unwrap_err();
        match error {
            Error::Command { code, stderr, .. } => {
                assert_eq!(code, 1);
                assert!(stderr.contains("No such file"), "{stderr}");
            }
            other => panic!("unexpected error: {other}"),
        }
    }

    #[test]
    fn formatter_callbacks_contain_panics_and_errors() {
        struct Io;
        impl RawIo for Io {
            fn read_at(&mut self, _offset: u64, _buf: &mut [u8]) -> Result<usize> {
                panic!("read panic")
            }
            fn write_at(&mut self, _offset: u64, _buf: &[u8]) -> Result<usize> {
                panic!("write panic")
            }
            fn size(&self) -> u64 {
                16 * 1024 * 1024
            }
            fn logical_sector_size(&self) -> u32 {
                512
            }
            fn sync(&mut self) -> Result<()> {
                Err(Error::Io(io::Error::other("sync failure")))
            }
        }

        let mut io = Io;
        let mut ctx = RawCallbacks {
            io: &mut io,
            error: None,
        };
        // SAFETY: valid context and a one-byte buffer owned by the test.
        let result = unsafe { format_write((&raw mut ctx).cast(), 0, b"x".as_ptr().cast(), 1) };
        assert_eq!(result, -1);
        assert!(
            matches!(ctx.error.take(), Some(Error::Unsupported(message)) if message.contains("panic"))
        );

        // SAFETY: valid context; sync failure must be reported through the C ABI.
        let result = unsafe { format_sync((&raw mut ctx).cast()) };
        assert_eq!(result, -1);
        assert!(
            matches!(ctx.error.take(), Some(Error::Io(error)) if error.to_string() == "sync failure")
        );
    }
}
