//! `image2usb` command line interface.

#![forbid(unsafe_code)]

use std::fs::{File, OpenOptions};
use std::io::{self, IsTerminal, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::{AtomicU64, Ordering};

use clap::{Parser, Subcommand, ValueEnum};
use image2usb_core::{
    CancelToken, DeviceInfo, Error, FileDevice, Phase, ProgressEvent, ProgressSink, RawIo,
    ResolvedMode, Result, SafetyError, VerifyMode, WriteOptions, check_target, verify_image,
    write_image,
};
use image2usb_extract::extract_image;
use image2usb_iso::{IsoInfo, analyze_path};
use image2usb_platform::native;

#[derive(Parser)]
#[command(
    name = "image2usb",
    version,
    about = "Create bootable USB drives from ISO images",
    long_about = "image2usb writes ISO images to removable media on macOS, Linux and Windows.\n\
                  Writing requires administrator/root privileges."
)]
struct Cli {
    /// Enable debug logging (same as RUST_LOG=debug).
    #[arg(short, long, global = true)]
    verbose: bool,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// List removable devices.
    List {
        /// Include internal and system disks (marked as blocked).
        #[arg(long)]
        all: bool,
        /// Machine readable output.
        #[arg(long)]
        json: bool,
    },
    /// Inspect an ISO image and report its boot capabilities.
    Analyze {
        /// Path to the ISO image.
        iso: PathBuf,
        /// Machine readable output.
        #[arg(long)]
        json: bool,
    },
    /// Write an ISO image to a device.
    Write {
        /// Path to the ISO image.
        iso: PathBuf,
        /// Target device (see `image2usb list`).
        #[arg(long, conflicts_with = "output")]
        device: Option<String>,
        /// Write to a regular file instead of a device (testing / image creation).
        /// Extraction requires this file to exist and be at least as large as the
        /// ISO (and at least 64 MiB).
        #[arg(long, conflicts_with = "device")]
        output: Option<PathBuf>,
        /// Skip the interactive confirmation.
        #[arg(long)]
        yes: bool,
        /// Print the plan without touching anything.
        #[arg(long)]
        dry_run: bool,
        /// Write strategy.
        #[arg(long, value_enum, default_value_t = ModeArg::Auto)]
        mode: ModeArg,
        /// FAT32 volume label for extraction mode (default: ISO volume ID).
        #[arg(long)]
        label: Option<String>,
        /// Read the device back and compare it with the image. In extraction
        /// mode every copied file is read back and hashed.
        #[arg(long)]
        verify: bool,
    },
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum ModeArg {
    Auto,
    Dd,
    Extract,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    init_logging(cli.verbose);

    match run(cli.command) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("error: {err}");
            if let Some(hint) = hint_for(&err) {
                eprintln!("hint: {hint}");
            }
            ExitCode::from(exit_code(&err))
        }
    }
}

fn run(command: Command) -> Result<()> {
    match command {
        Command::List { all, json } => list_devices(all, json),
        Command::Analyze { iso, json } => analyze(&iso, json),
        Command::Write {
            iso,
            device,
            output,
            yes,
            dry_run,
            mode,
            label,
            verify,
        } => write(&iso, device, output, yes, dry_run, mode, label, verify),
    }
}

fn list_devices(all: bool, json: bool) -> Result<()> {
    let platform = native();
    let mut devices = platform.list_devices()?;
    if !all {
        devices.retain(|d| d.is_safe_target());
    }
    devices.sort_by(|a, b| a.id.as_str().cmp(b.id.as_str()));

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&devices)
                .map_err(|err| Error::Other(format!("failed to serialize devices: {err}")))?
        );
        return Ok(());
    }

    if devices.is_empty() {
        println!("No removable devices found.");
        println!("Insert a USB drive, or run `image2usb list --all` to inspect every disk.");
        return Ok(());
    }

    println!(
        "{:<22} {:<34} {:>10} {:<8} {:<6} MOUNTS",
        "DEVICE", "MODEL", "SIZE", "BUS", "BLOCKED"
    );
    for d in &devices {
        println!(
            "{:<22} {:<34} {:>10} {:<8} {:<6} {}",
            d.id.as_str(),
            truncate(&d.model, 34),
            image2usb_core::util::format_bytes(d.size),
            d.bus.to_string(),
            if d.is_safe_target() { "no" } else { "yes" },
            d.mount_points.join(", ")
        );
    }
    Ok(())
}

fn analyze(path: &Path, json: bool) -> Result<()> {
    let info = analyze_iso(path)?;
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&info)
                .map_err(|err| Error::Other(format!("failed to serialize analysis: {err}")))?
        );
        return Ok(());
    }

    println!("Image:        {}", info.path.display());
    println!(
        "Size:         {}",
        image2usb_core::util::format_bytes(info.size)
    );
    println!(
        "Volume ID:    {}",
        info.volume_id.as_deref().unwrap_or("(none)")
    );
    println!("Distribution: {}", info.distro.label());
    println!(
        "Filesystems:  ISO9660{}",
        [
            info.has_joliet.then_some(" + Joliet"),
            info.has_rock_ridge.then_some(" + Rock Ridge"),
            info.has_udf.then_some(" + UDF"),
        ]
        .into_iter()
        .flatten()
        .collect::<String>()
    );

    if let Some(el) = &info.el_torito {
        println!(
            "El Torito:    BIOS {}, UEFI {}, {} entr{}",
            yes_no(el.bios),
            yes_no(el.uefi),
            el.entries.len(),
            if el.entries.len() == 1 { "y" } else { "ies" }
        );
    } else {
        println!("El Torito:    not present");
    }

    if let Some(mbr) = &info.mbr {
        println!(
            "MBR:          signature {}, boot code {}, {} partition(s), hybrid {}",
            yes_no(mbr.signature_valid),
            yes_no(mbr.boot_code_nonzero),
            mbr.partitions.len(),
            yes_no(mbr.isohybrid_bootable())
        );
    } else {
        println!("MBR:          not present");
    }

    if let Some(gpt) = &info.gpt {
        println!(
            "GPT:          {} entries, ESP {}",
            gpt.entry_count,
            yes_no(gpt.has_esp)
        );
    }

    println!(
        "DD mode:      BIOS {}, UEFI {}",
        yes_no(info.dd_bios),
        yes_no(info.dd_uefi)
    );

    let mut boot = Vec::new();
    if let Some(f) = &info.boot_files.isolinux {
        boot.push(format!("isolinux: {f}"));
    }
    if let Some(f) = &info.boot_files.grub_cfg {
        boot.push(format!("grub: {f}"));
    }
    if info.boot_files.bootmgr {
        boot.push("Windows BOOTMGR".to_owned());
    }
    if !info.boot_files.efi_boot.is_empty() {
        boot.push(format!("EFI: {}", info.boot_files.efi_boot.join(", ")));
    }
    if boot.is_empty() {
        boot.push("none detected".to_owned());
    }
    println!("Boot files:   {}", boot.join("; "));

    if let Some(windows) = &info.windows
        && let Some(size) = windows.install_wim_size
    {
        println!(
            "Windows:      install.wim {}",
            image2usb_core::util::format_bytes(size)
        );
    }

    if !info.warnings.is_empty() {
        println!();
        for warning in &info.warnings {
            println!("warning: {warning}");
        }
    }
    println!();
    println!(
        "Recommended:  {}",
        if info.dd_safe() {
            "DD (write the image 1:1)"
        } else {
            "extraction (MBR + FAT32, UEFI boot files)"
        }
    );
    Ok(())
}

fn write(
    iso: &Path,
    device: Option<String>,
    output: Option<PathBuf>,
    yes: bool,
    dry_run: bool,
    mode: ModeArg,
    label: Option<String>,
    verify: bool,
) -> Result<()> {
    let info = analyze_iso(iso)?;
    let image_len = info.size;
    let resolved = resolve_mode(mode, &info);
    let verify_mode = if verify {
        VerifyMode::Sha256
    } else {
        VerifyMode::None
    };
    let options = WriteOptions {
        label: label.unwrap_or_default(),
        verify: verify_mode,
        ..WriteOptions::default()
    };

    println!("Image:  {}", iso.display());
    println!("Size:   {}", image2usb_core::util::format_bytes(image_len));
    println!("Distro: {}", info.distro.label());
    println!(
        "Mode:   {} ({})",
        resolved,
        match resolved {
            ResolvedMode::Dd => "byte-for-byte copy",
            ResolvedMode::Extract => "MBR + FAT32, file extraction",
        }
    );
    if resolved == ResolvedMode::Extract {
        println!(
            "Target: {} + {}, label {}",
            options.scheme,
            options.filesystem,
            label_for(&options, &info)
        );
    }
    for warning in &info.warnings {
        println!("warning: {warning}");
    }

    // Extraction needs room for the FAT structures and the full tree; require
    // at least the image size so a too-small device is rejected before the MBR
    // is touched.
    let required = if resolved == ResolvedMode::Extract {
        image_len.max(64 * 1024 * 1024)
    } else {
        image_len
    };

    if dry_run {
        println!();
        println!("Dry run: no data will be written.");
        if resolved == ResolvedMode::Extract {
            let (files, dirs, bytes) = image2usb_extract::preview(iso)?;
            println!(
                "Would extract {} files ({} directories, {}) from the ISO tree.",
                files,
                dirs,
                image2usb_core::util::format_bytes(bytes)
            );
        }
        if let Some(device) = &device {
            let target = find_device(device)?;
            print_target(&target);
            check_target(&target, required)?;
            println!("Safety checks: passed");
        } else if let Some(output) = &output {
            println!("Would write to file: {}", output.display());
        } else {
            println!("No --device/--output given; nothing else to do.");
        }
        return Ok(());
    }

    if let Some(output) = output {
        // Writing to an existing file destroys its contents, so require the
        // same explicit opt-in as writing to a device. Creating a new file is
        // not destructive and does not need `--yes`.
        if output.exists() && !yes {
            return Err(Error::Other(format!(
                "refusing to overwrite existing output {} without --yes",
                output.display()
            )));
        }
        let mut target = match resolved {
            ResolvedMode::Dd => open_output(&output, image_len)?,
            ResolvedMode::Extract => open_output_existing(&output, required)?,
        };
        let cancel = install_cancel_handler()?;
        let sink = CliSink::new();
        run_on_target(
            resolved,
            iso,
            &info,
            image_len,
            &mut target,
            &options,
            &sink,
            &cancel,
        )?;
        println!("Done. Output image: {}", output.display());
        return Ok(());
    }

    let Some(device_id) = device else {
        return Err(Error::Other(
            "missing --device (see `image2usb list`) or --output".to_owned(),
        ));
    };
    let target = find_device(&device_id)?;
    print_target(&target);
    check_target(&target, required)?;

    if !yes {
        confirm(&target)?;
    }

    let platform = native();
    let cancel = install_cancel_handler()?;
    let sink = CliSink::new();
    let write_result = {
        let mut raw = platform.open_exclusive(&target)?;
        let result = run_on_target(
            resolved, iso, &info, image_len, &mut *raw, &options, &sink, &cancel,
        );
        if result.is_ok() {
            raw.invalidate()?;
        }
        result
    };

    match write_result {
        Ok(()) => {
            platform.eject(&target)?;
            println!("Done. Device can be removed.");
            Ok(())
        }
        Err(err) => {
            eprintln!();
            eprintln!(
                "The device may contain partial data. Re-run the write to overwrite it completely."
            );
            Err(err)
        }
    }
}

fn label_for<'a>(options: &'a WriteOptions, info: &'a IsoInfo) -> &'a str {
    if options.label.trim().is_empty() {
        info.volume_id.as_deref().unwrap_or("IMAGE2USB")
    } else {
        &options.label
    }
}

#[allow(clippy::too_many_arguments)]
fn run_on_target(
    mode: ResolvedMode,
    iso: &Path,
    info: &IsoInfo,
    image_len: u64,
    target: &mut dyn RawIo,
    options: &WriteOptions,
    sink: &dyn ProgressSink,
    cancel: &CancelToken,
) -> Result<()> {
    match mode {
        ResolvedMode::Dd => execute_write(iso, image_len, target, sink, cancel, options.verify),
        ResolvedMode::Extract => execute_extract(iso, info, target, options, sink, cancel),
    }
}

fn execute_extract(
    iso: &Path,
    info: &IsoInfo,
    target: &mut dyn RawIo,
    options: &WriteOptions,
    sink: &dyn ProgressSink,
    cancel: &CancelToken,
) -> Result<()> {
    let stats = extract_image(iso, info, target, options, cancel, sink)?;
    println!();
    println!(
        "Extracted {} files ({} directories, {}) in {:.1}s.",
        stats.files,
        stats.directories,
        image2usb_core::util::format_bytes(stats.bytes),
        stats.elapsed.as_secs_f64()
    );
    for warning in &stats.warnings {
        println!("warning: {warning}");
    }
    Ok(())
}

fn execute_write(
    iso: &Path,
    image_len: u64,
    device: &mut dyn RawIo,
    sink: &dyn ProgressSink,
    cancel: &CancelToken,
    verify_mode: VerifyMode,
) -> Result<()> {
    let mut image = File::open(iso)?;

    sink.report(ProgressEvent::message(Phase::Writing, "writing image"));
    let stats = write_image(&mut image, image_len, device, cancel, sink)?;
    println!();
    println!(
        "Wrote {} in {:.1}s ({}/s). SHA-256: {}",
        image2usb_core::util::format_bytes(stats.bytes_written),
        stats.elapsed.as_secs_f64(),
        image2usb_core::util::format_bytes(
            (stats.bytes_written as f64 / stats.elapsed.as_secs_f64().max(0.001)) as u64
        ),
        stats.sha256
    );

    if verify_mode == VerifyMode::Sha256 {
        image.seek(SeekFrom::Start(0))?;
        sink.report(ProgressEvent::message(Phase::Verifying, "reading back"));
        let verify = verify_image(&mut image, image_len, device, cancel, sink)?;
        println!(
            "Verified {} in {:.1}s.",
            image2usb_core::util::format_bytes(verify.bytes_compared),
            verify.elapsed.as_secs_f64()
        );
    }
    Ok(())
}

fn resolve_mode(mode: ModeArg, info: &IsoInfo) -> ResolvedMode {
    match mode {
        ModeArg::Auto => {
            if info.dd_safe() {
                ResolvedMode::Dd
            } else {
                ResolvedMode::Extract
            }
        }
        ModeArg::Dd => ResolvedMode::Dd,
        ModeArg::Extract => ResolvedMode::Extract,
    }
}

fn find_device(id: &str) -> Result<DeviceInfo> {
    let platform = native();
    platform
        .list_devices()?
        .into_iter()
        .find(|d| d.id.as_str() == id)
        .ok_or_else(|| Error::DeviceNotFound(id.to_owned()))
}

fn print_target(target: &DeviceInfo) {
    println!();
    println!("Device: {}", target.id);
    println!("Model:  {}", target.model);
    if let Some(serial) = &target.serial {
        println!("Serial: {serial}");
    }
    println!(
        "Size:   {}",
        image2usb_core::util::format_bytes(target.size)
    );
    println!("Bus:    {}", target.bus);
    if !target.mount_points.is_empty() {
        println!("Mounts: {}", target.mount_points.join(", "));
    }
}

fn confirm(target: &DeviceInfo) -> Result<()> {
    let expected = target.serial.as_deref().unwrap_or("YES");
    eprintln!();
    eprintln!(
        "ALL DATA ON {} ({}) WILL BE DESTROYED.",
        target.id, target.model
    );
    eprint!("Type `{expected}` to continue: ");
    io::stderr().flush()?;
    let mut line = String::new();
    io::stdin().read_line(&mut line)?;
    if line.trim().eq_ignore_ascii_case(expected) {
        Ok(())
    } else {
        Err(Error::Other(
            "confirmation rejected; nothing was written".to_owned(),
        ))
    }
}

fn open_output(path: &Path, len: u64) -> Result<FileDevice> {
    if !path.exists() {
        let file = File::create(path)?;
        file.set_len(len)?;
    } else {
        let current = std::fs::metadata(path)?.len();
        if current < len {
            OpenOptions::new().write(true).open(path)?.set_len(len)?;
        }
    }
    FileDevice::open(path)
}

/// Extraction cannot infer the target size from the ISO, so `--output` must be
/// a pre-sized file. Tests use `mkfile 256m`/`truncate -s 256M`.
fn open_output_existing(path: &Path, min_len: u64) -> Result<FileDevice> {
    let meta = std::fs::metadata(path).map_err(|_| {
        Error::Other(format!(
            "--output for extraction must be an existing file of at least {} \
             (create it first, e.g. `mkfile 256m {}`)",
            image2usb_core::util::format_bytes(min_len),
            path.display()
        ))
    })?;
    if meta.len() < min_len {
        return Err(Error::Other(format!(
            "output file {} is too small ({} < {} required)",
            path.display(),
            image2usb_core::util::format_bytes(meta.len()),
            image2usb_core::util::format_bytes(min_len)
        )));
    }
    FileDevice::open(path)
}

fn analyze_iso(path: &Path) -> Result<IsoInfo> {
    analyze_path(path).map_err(|err| Error::InvalidImage(err.to_string()))
}

fn install_cancel_handler() -> Result<CancelToken> {
    let cancel = CancelToken::new();
    let token = cancel.clone();
    ctrlc::set_handler(move || {
        eprintln!();
        eprintln!("cancelling after the current chunk...");
        token.cancel();
    })
    .map_err(|err| Error::Other(format!("failed to install Ctrl+C handler: {err}")))?;
    Ok(cancel)
}

struct CliSink {
    tty: bool,
    last_percent: AtomicU64,
}

impl CliSink {
    fn new() -> Self {
        Self {
            tty: io::stderr().is_terminal(),
            last_percent: AtomicU64::new(u64::MAX),
        }
    }
}

impl ProgressSink for CliSink {
    fn report(&self, event: ProgressEvent) {
        let is_data_phase = matches!(event.phase, Phase::Writing | Phase::Verifying);
        if !is_data_phase || event.total_bytes == 0 {
            eprintln!("{}", event.summary());
            return;
        }

        let percent = (event.fraction() * 100.0) as u64;
        let finished = event.done_bytes >= event.total_bytes;
        if self.tty {
            eprint!("\r\x1b[2K{}", event.summary());
            let _ = io::stderr().flush();
            if finished {
                eprintln!();
            }
        } else {
            let previous = self.last_percent.load(Ordering::Relaxed);
            if finished || previous == u64::MAX || percent >= previous + 5 {
                eprintln!("{}", event.summary());
                self.last_percent.store(percent, Ordering::Relaxed);
            }
        }
    }
}

fn yes_no(value: bool) -> &'static str {
    if value { "yes" } else { "no" }
}

fn truncate(value: &str, max: usize) -> String {
    if value.chars().count() <= max {
        value.to_owned()
    } else {
        let mut out: String = value.chars().take(max.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}

fn init_logging(verbose: bool) {
    let filter = if verbose {
        "debug".to_owned()
    } else {
        std::env::var("RUST_LOG").unwrap_or_else(|_| "warn".to_owned())
    };
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(io::stderr)
        .try_init();
}

fn exit_code(err: &Error) -> u8 {
    match err {
        Error::Permission(_) => 3,
        Error::DeviceNotFound(_) | Error::DeviceChanged { .. } | Error::UnsafeTarget(_) => 4,
        Error::InvalidImage(_) | Error::Unsupported(_) | Error::EmptyImage => 5,
        Error::Io(_) | Error::ImageTooLarge { .. } => 6,
        Error::VerifyMismatch { .. } => 7,
        Error::ExtractedFileMismatch { .. } => 7,
        Error::Cancelled => 8,
        _ => 1,
    }
}

fn hint_for(err: &Error) -> Option<&'static str> {
    match err {
        Error::Permission(_) => Some("run with sudo (macOS/Linux) or as Administrator (Windows)"),
        Error::UnsafeTarget(SafetyError::SystemDisk) => {
            Some("image2usb refuses to write to the disk that hosts the running OS")
        }
        Error::UnsafeTarget(_) => Some("only USB/SD removable media can be used as targets"),
        Error::DeviceNotFound(_) => Some("run `image2usb list` and copy the device id exactly"),
        Error::InvalidImage(_) => Some("make sure the file is an ISO 9660 image"),
        Error::Unsupported(message) if message.contains("Windows") => {
            Some("use --mode dd for Windows media (UEFI boot only) or wait for split-WIM support")
        }
        _ => None,
    }
}
