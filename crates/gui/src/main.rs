//! image2usb graphical interface (egui/eframe).

#![forbid(unsafe_code)]
// Release Windows builds are GUI apps and must not open a console window.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use std::fs::File;
use std::io::{Seek, SeekFrom};
use std::path::PathBuf;
use std::thread;

use crossbeam_channel::{Receiver, Sender};
use eframe::egui;
use image2usb_core::util::format_bytes;
use image2usb_core::{
    CancelToken, DeviceInfo, FileSystem as FilesystemKind, FileSystemChoice, PartitionScheme,
    Phase, ProgressEvent, ProgressSink, ResolvedMode, Result, VerifyMode, WriteOptions,
    check_target, verify_image, write_image,
};
use image2usb_extract::{WINDOWS_NTFS_SUPPORTED, analyze_iso, extract_image, resolve_filesystem};
use image2usb_iso::{Distro, IsoInfo};
use image2usb_platform::native;

fn main() -> eframe::Result {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(std::env::var("RUST_LOG").unwrap_or_else(|_| "warn".to_owned()))
        .try_init();

    let options = eframe::NativeOptions {
        #[cfg(target_os = "macos")]
        renderer: eframe::Renderer::Glow,
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1040.0, 720.0])
            .with_min_inner_size([820.0, 560.0])
            .with_title("image2usb"),
        ..Default::default()
    };
    eframe::run_native(
        "image2usb",
        options,
        Box::new(|cc| Ok(Box::new(App::new(cc)))),
    )
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ModeChoice {
    Auto,
    Dd,
    Extract,
}

enum Msg {
    Devices(std::result::Result<Vec<DeviceInfo>, String>),
    Analysis(Box<std::result::Result<IsoInfo, String>>),
    Progress(ProgressEvent),
    Log(String),
    Done(std::result::Result<String, String>),
}

struct GuiSink {
    tx: Sender<Msg>,
}

impl ProgressSink for GuiSink {
    fn report(&self, event: ProgressEvent) {
        let _ = self.tx.try_send(Msg::Progress(event));
    }
}

struct App {
    tx: Sender<Msg>,
    rx: Receiver<Msg>,

    devices: Vec<DeviceInfo>,
    devices_error: Option<String>,
    selected: Option<String>,

    iso_path: PathBuf,
    analysis: Option<IsoInfo>,
    analysis_error: Option<String>,

    mode: ModeChoice,
    scheme: PartitionScheme,
    filesystem: FileSystemChoice,
    verify: bool,
    confirmed: bool,

    busy: bool,
    progress: Option<ProgressEvent>,
    status: String,
    log: Vec<String>,
    cancel: Option<CancelToken>,
    is_admin: bool,
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        cc.egui_ctx.set_visuals(egui::Visuals::dark());
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut app = Self {
            tx,
            rx,
            devices: Vec::new(),
            devices_error: None,
            selected: None,
            iso_path: PathBuf::new(),
            analysis: None,
            analysis_error: None,
            mode: ModeChoice::Auto,
            scheme: PartitionScheme::Mbr,
            filesystem: FileSystemChoice::Auto,
            verify: true,
            confirmed: false,
            busy: false,
            progress: None,
            status: "Ready".to_owned(),
            log: Vec::new(),
            cancel: None,
            is_admin: is_elevated(),
        };
        app.refresh_devices(cc.egui_ctx.clone());
        app
    }

    fn log(&mut self, message: impl Into<String>) {
        let message = message.into();
        self.log.push(message);
        if self.log.len() > 500 {
            self.log.remove(0);
        }
    }

    fn refresh_devices(&mut self, ctx: egui::Context) {
        let tx = self.tx.clone();
        thread::spawn(move || {
            let result = native().list_devices().map_err(|err| err.to_string());
            let _ = tx.send(Msg::Devices(result));
            ctx.request_repaint();
        });
    }

    fn analyze(&mut self, ctx: egui::Context) {
        let path = self.iso_path.clone();
        if path.as_os_str().is_empty() {
            self.analysis_error = Some("Choose an ISO image first.".to_owned());
            return;
        }
        self.analysis = None;
        self.analysis_error = None;
        self.confirmed = false;
        self.status = "Analyzing image...".to_owned();
        let tx = self.tx.clone();
        thread::spawn(move || {
            let result = analyze_iso(&path).map_err(|err| err.to_string());
            let _ = tx.send(Msg::Analysis(Box::new(result)));
            ctx.request_repaint();
        });
    }

    fn selected_device(&self) -> Option<&DeviceInfo> {
        let id = self.selected.as_deref()?;
        self.devices.iter().find(|d| d.id.as_str() == id)
    }

    fn resolved_mode(&self) -> Option<ResolvedMode> {
        let info = self.analysis.as_ref()?;
        Some(match self.mode {
            ModeChoice::Auto => {
                if info.dd_safe() {
                    ResolvedMode::Dd
                } else {
                    ResolvedMode::Extract
                }
            }
            ModeChoice::Dd => ResolvedMode::Dd,
            ModeChoice::Extract => ResolvedMode::Extract,
        })
    }

    fn filesystem_plan(&self) -> Option<std::result::Result<(FilesystemKind, String), String>> {
        let info = self.analysis.as_ref()?;
        let mode = self.resolved_mode()?;
        Some(resolve_filesystem(self.filesystem, mode, info).map_err(|error| error.to_string()))
    }

    fn can_write(&self) -> bool {
        !self.busy
            && self.confirmed
            && self.analysis.is_some()
            && self.selected_device().is_some_and(|d| d.is_safe_target())
            && self.resolved_mode().is_some()
            && self.windows_incompatibility().is_none()
            && match self.filesystem_plan() {
                Some(Ok((FilesystemKind::Fat32, _))) => true,
                Some(Ok((FilesystemKind::Ntfs, _))) => WINDOWS_NTFS_SUPPORTED,
                _ => false,
            }
    }

    fn windows_incompatibility(&self) -> Option<String> {
        if self.resolved_mode() != Some(ResolvedMode::Extract) {
            return None;
        }
        let info = self.analysis.as_ref()?;
        if info.distro != Distro::Windows {
            return None;
        }
        let windows = info.windows.as_ref()?;
        if !windows.bootmgr || !windows.bcd {
            return Some(
                "Windows boot chain is incomplete: BOOTMGR and boot/BCD are required.".to_owned(),
            );
        }
        if windows.efi_fallback.is_none() {
            return Some(
                "Windows media has no EFI/BOOT/BOOT<arch>.EFI removable-media fallback.".to_owned(),
            );
        }
        if windows.pe_architecture == image2usb_iso::WindowsArchitecture::Unknown {
            return Some(
                "Windows EFI fallback has no recognized PE machine; extraction is refused."
                    .to_owned(),
            );
        }
        if windows.install_parts.is_empty() {
            return Some("Windows media has no installation image.".to_owned());
        }
        None
    }

    fn start_write(&mut self, ctx: egui::Context) {
        if self.busy {
            return;
        }
        let Some(device) = self.selected_device().cloned() else {
            self.log("Select a device first.");
            return;
        };
        let Some(info) = self.analysis.clone() else {
            self.log("Analyze an image first.");
            return;
        };
        let mode = self.resolved_mode().unwrap_or(ResolvedMode::Dd);
        if let Some(reason) = self.windows_incompatibility() {
            self.log(format!("Windows compatibility check failed: {reason}"));
            return;
        }
        let (filesystem, filesystem_reason) = match self.filesystem_plan() {
            Some(Ok(plan)) => plan,
            Some(Err(error)) => {
                self.log(format!("Filesystem selection failed: {error}"));
                return;
            }
            None => {
                self.log("Analyze an image first.");
                return;
            }
        };
        if mode == ResolvedMode::Extract
            && filesystem == FilesystemKind::Ntfs
            && !WINDOWS_NTFS_SUPPORTED
        {
            self.log(
                "NTFS/UEFI:NTFS extraction is not available in this build; use a build with the \
                 windows-media feature on macOS. The operation stopped before opening the device.",
            );
            return;
        }
        // Extraction needs room for FAT metadata and the full tree, so require
        // the image size plus a margin and never less than 64 MiB.
        let required = if mode == ResolvedMode::Extract {
            info.size
                .saturating_add(16 * 1024 * 1024)
                .max(64 * 1024 * 1024)
        } else {
            info.size
        };
        if let Err(err) = check_target(&device, required) {
            self.log(format!("Safety check failed: {err}"));
            return;
        }
        if !self.is_admin {
            self.log(
                "Not running with administrator privileges: opening the raw device will likely fail. \
                 Restart image2usb with sudo (macOS/Linux) or as Administrator (Windows).",
            );
        }

        let iso = self.iso_path.clone();
        let cancel = CancelToken::new();
        self.cancel = Some(cancel.clone());
        self.busy = true;
        self.confirmed = false;
        self.progress = Some(ProgressEvent::message(Phase::Opening, "opening device"));
        self.status = "Opening device...".to_owned();
        self.log(format!(
            "Writing {} to {}: {} + {} ({})",
            iso.display(),
            device.id,
            mode,
            filesystem,
            filesystem_reason
        ));

        let tx = self.tx.clone();
        let verify = self.verify;
        let scheme = self.scheme;
        let ctx_worker = ctx.clone();
        thread::spawn(move || {
            let platform = native();
            let sink = GuiSink { tx: tx.clone() };
            let _ = tx.send(Msg::Log(format!("Opening {}", device.id)));
            let result = (|| -> Result<String> {
                let mut raw = platform.open_exclusive(&device)?;
                let message = match mode {
                    ResolvedMode::Dd => {
                        let mut image = File::open(&iso)?;
                        let stats = write_image(&mut image, info.size, &mut *raw, &cancel, &sink)?;
                        let mut message = format!(
                            "Wrote {} in {:.1}s. SHA-256 {}",
                            format_bytes(stats.bytes_written),
                            stats.elapsed.as_secs_f64(),
                            stats.sha256
                        );
                        if verify {
                            image.seek(SeekFrom::Start(0))?;
                            let verify_stats =
                                verify_image(&mut image, info.size, &mut *raw, &cancel, &sink)?;
                            message.push_str(&format!(
                                " Verified {}.",
                                format_bytes(verify_stats.bytes_compared)
                            ));
                        }
                        message
                    }
                    ResolvedMode::Extract => {
                        let options = WriteOptions {
                            scheme,
                            filesystem: filesystem.into(),
                            verify: if verify {
                                VerifyMode::Sha256
                            } else {
                                VerifyMode::None
                            },
                            label: String::new(),
                            ..WriteOptions::default()
                        };
                        let stats =
                            extract_image(&iso, &info, &mut *raw, &options, &cancel, &sink)?;
                        let mut message = format!(
                            "Extracted {} files ({} directories, {}) in {:.1}s.",
                            stats.files,
                            stats.directories,
                            format_bytes(stats.bytes),
                            stats.elapsed.as_secs_f64()
                        );
                        for warning in &stats.warnings {
                            message.push_str(&format!(" Warning: {warning}"));
                        }
                        message
                    }
                };
                raw.invalidate()?;
                drop(raw);
                platform.eject(&device)?;
                Ok(message)
            })()
            .map_err(|err| err.to_string());

            let _ = tx.send(Msg::Done(result));
            ctx_worker.request_repaint();
        });
    }

    fn handle_messages(&mut self, ctx: &egui::Context) {
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                Msg::Devices(Ok(mut devices)) => {
                    devices.retain(DeviceInfo::is_safe_target);
                    self.devices = devices;
                    self.devices_error = None;
                    if self.selected.as_ref().is_some_and(|id| {
                        !self.devices.iter().any(|d| d.id.as_str() == id.as_str())
                    }) {
                        self.selected = None;
                        self.confirmed = false;
                    }
                }
                Msg::Devices(Err(err)) => self.devices_error = Some(err),
                Msg::Analysis(result) => match *result {
                    Ok(info) => {
                        self.status = format!(
                            "{} image, {} — {}",
                            info.distro.label(),
                            format_bytes(info.size),
                            if info.dd_safe() {
                                "DD recommended"
                            } else {
                                "extraction required"
                            }
                        );
                        self.analysis = Some(info);
                        self.analysis_error = None;
                    }
                    Err(err) => {
                        self.analysis_error = Some(err);
                        self.status = "Analysis failed".to_owned();
                    }
                },
                Msg::Progress(event) => {
                    self.status = event.summary();
                    self.progress = Some(event);
                }
                Msg::Log(message) => self.log(message),
                Msg::Done(result) => {
                    self.busy = false;
                    self.cancel = None;
                    match result {
                        Ok(message) => {
                            self.status = "Done".to_owned();
                            self.log(message);
                        }
                        Err(err) => {
                            self.status = "Failed".to_owned();
                            self.log(format!("Error: {err}"));
                        }
                    }
                    self.refresh_devices(ctx.clone());
                }
            }
        }
    }

    fn draw_devices(&mut self, ui: &mut egui::Ui) {
        ui.heading("Devices");
        ui.add_space(4.0);
        if ui.button("Refresh").clicked() {
            self.refresh_devices(ui.ctx().clone());
        }
        if let Some(err) = &self.devices_error {
            ui.colored_label(egui::Color32::LIGHT_RED, err);
        }
        ui.separator();
        if self.devices.is_empty() && self.devices_error.is_none() {
            ui.label("No eligible devices. Connect a writable USB drive or SD card, then refresh.");
        }

        let mut clicked = None;
        egui::ScrollArea::vertical()
            .max_height(420.0)
            .show(ui, |ui| {
                for device in &self.devices {
                    let selected = self.selected.as_deref() == Some(device.id.as_str());
                    let label = format!(
                        "{}  •  {}  •  {}",
                        device.model,
                        format_bytes(device.size),
                        device.bus,
                    );
                    let response =
                        ui.add_enabled(!self.busy, egui::Button::selectable(selected, label));
                    if response.clicked() {
                        clicked = Some(device.id.as_str().to_owned());
                    }
                    ui.horizontal(|ui| {
                        ui.small(device.id.as_str());
                        if !device.mount_points.is_empty() {
                            ui.small(format!("mounted: {}", device.mount_points.join(", ")));
                        }
                    });
                    ui.add_space(6.0);
                }
            });
        if let Some(id) = clicked {
            self.selected = Some(id);
            self.confirmed = false;
        }
    }

    fn draw_image(&mut self, ui: &mut egui::Ui) {
        let previous_options = (self.mode, self.scheme, self.filesystem, self.verify);
        ui.heading("Image");
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            if ui
                .add_enabled(!self.busy, egui::Button::new("Choose ISO…"))
                .clicked()
                && let Some(path) = rfd::FileDialog::new()
                    .add_filter("Disk images", &["iso", "img"])
                    .pick_file()
            {
                self.iso_path = path;
                self.analyze(ui.ctx().clone());
            }
            ui.menu_button("Download ISO…", |ui| {
                ui.label("Official download pages (opens your browser):");
                ui.hyperlink_to("Ubuntu", "https://ubuntu.com/download/desktop");
                ui.hyperlink_to("Fedora", "https://fedoraproject.org/workstation/download");
                ui.hyperlink_to("Debian", "https://www.debian.org/distrib/");
                ui.hyperlink_to("Linux Mint", "https://linuxmint.com/download.php");
                ui.hyperlink_to("Arch Linux", "https://archlinux.org/download/");
                ui.separator();
                ui.label("After downloading, use Choose ISO… to select the file.");
                ui.label("Choose the ISO architecture for the computer that will boot the USB.");
            });
        });
        if let Some(name) = self.iso_path.file_name() {
            ui.label(name.to_string_lossy())
                .on_hover_text(self.iso_path.display().to_string());
        } else {
            ui.label("Choose an image from your computer or download a Linux ISO.");
        }

        if let Some(err) = &self.analysis_error {
            ui.add_space(4.0);
            ui.colored_label(egui::Color32::LIGHT_RED, err);
        }

        if let Some(info) = &self.analysis {
            ui.add_space(8.0);
            egui::Grid::new("analysis")
                .num_columns(2)
                .spacing([18.0, 4.0])
                .show(ui, |ui| {
                    ui.label("Distribution");
                    ui.label(info.distro.label());
                    ui.end_row();
                    ui.label("Size");
                    ui.label(format_bytes(info.size));
                    ui.end_row();
                    ui.label("Filesystems");
                    ui.label(format!(
                        "ISO9660{}{}{}",
                        if info.has_joliet { " + Joliet" } else { "" },
                        if info.has_rock_ridge {
                            " + Rock Ridge"
                        } else {
                            ""
                        },
                        if info.has_udf { " + UDF" } else { "" }
                    ));
                    ui.end_row();
                    ui.label("Legacy BIOS");
                    ui.label(if info.dd_bios {
                        "bootable via DD"
                    } else {
                        "needs extraction"
                    });
                    ui.end_row();
                    ui.label("UEFI");
                    ui.label(if info.dd_uefi {
                        "bootable via DD"
                    } else {
                        "needs extraction"
                    });
                    ui.end_row();
                });
            for warning in &info.warnings {
                ui.add_space(4.0);
                ui.colored_label(egui::Color32::YELLOW, warning);
            }
        }

        ui.add_space(12.0);
        ui.heading("Options");
        ui.horizontal(|ui| {
            ui.label("Mode");
            egui::ComboBox::from_id_salt("mode")
                .selected_text(match self.mode {
                    ModeChoice::Auto => "Auto",
                    ModeChoice::Dd => "DD (1:1)",
                    ModeChoice::Extract => "Extraction",
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.mode, ModeChoice::Auto, "Auto");
                    ui.selectable_value(&mut self.mode, ModeChoice::Dd, "DD (1:1)");
                    ui.selectable_value(&mut self.mode, ModeChoice::Extract, "Extraction");
                });
            ui.checkbox(&mut self.verify, "Verify after write");
        });

        if self.resolved_mode() == Some(ResolvedMode::Extract) {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.label("Partition table");
                egui::ComboBox::from_id_salt("scheme")
                    .selected_text(self.scheme.to_string().to_uppercase())
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.scheme, PartitionScheme::Mbr, "MBR");
                        ui.selectable_value(&mut self.scheme, PartitionScheme::Gpt, "GPT + ESP");
                    });
            });
            ui.horizontal(|ui| {
                ui.label("Filesystem");
                egui::ComboBox::from_id_salt("filesystem")
                    .selected_text(match self.filesystem {
                        FileSystemChoice::Auto => "Auto",
                        FileSystemChoice::Fat32 => "FAT32",
                        FileSystemChoice::Ntfs => "NTFS",
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.filesystem, FileSystemChoice::Auto, "Auto");
                        ui.selectable_value(&mut self.filesystem, FileSystemChoice::Fat32, "FAT32");
                        ui.selectable_value(&mut self.filesystem, FileSystemChoice::Ntfs, "NTFS");
                    });
            });
            match self.filesystem_plan() {
                Some(Ok((FilesystemKind::Fat32, reason))) => {
                    ui.colored_label(
                        egui::Color32::YELLOW,
                        format!("Filesystem: FAT32 — {reason}"),
                    );
                }
                Some(Ok((FilesystemKind::Ntfs, reason))) => {
                    if WINDOWS_NTFS_SUPPORTED {
                        ui.colored_label(
                            egui::Color32::YELLOW,
                            format!(
                                "Filesystem: NTFS — {reason}. A small FAT partition with the \
                                 UEFI:NTFS chainloader will be added; Secure Boot acceptance is \
                                 not validated in this build."
                            ),
                        );
                    } else {
                        ui.colored_label(
                            egui::Color32::LIGHT_RED,
                            format!(
                                "Filesystem: NTFS — {reason}. NTFS/UEFI:NTFS extraction is not \
                                 available in this build."
                            ),
                        );
                    }
                }
                Some(Err(error)) => {
                    ui.colored_label(egui::Color32::LIGHT_RED, error);
                }
                None => {}
            }
            if let Some(reason) = self.windows_incompatibility() {
                ui.colored_label(egui::Color32::LIGHT_RED, reason);
            } else if self
                .analysis
                .as_ref()
                .is_some_and(|info| info.distro == Distro::Windows)
            {
                let ntfs_note = if WINDOWS_NTFS_SUPPORTED {
                    " Large media can use NTFS plus the UEFI:NTFS chainloader."
                } else {
                    ""
                };
                ui.colored_label(
                    egui::Color32::YELLOW,
                    format!(
                        "Windows extraction validates BOOTMGR, boot/BCD and the EFI fallback, then \
                         copies from a private snapshot. Windows media is UEFI-only in this path; \
                         legacy BIOS boot is not installed.{ntfs_note}"
                    ),
                );
            } else {
                ui.colored_label(
                    egui::Color32::YELLOW,
                    "Extraction creates a FAT32 volume from the ISO tree (MBR by default, or \
                     GPT + ESP). UEFI boot comes from EFI/BOOT; legacy BIOS uses the bundled \
                     GRUB core when the ISO has boot/grub/grub.cfg or boot/grub2/grub.cfg, \
                     otherwise use DD mode for BIOS.",
                );
            }
        }

        if let Some(device) = self.selected_device() {
            ui.add_space(8.0);
            ui.label(format!("Target: {} ({})", device.id, device.model));
            ui.checkbox(
                &mut self.confirmed,
                "I understand that all data on this device will be erased.",
            );
        }

        ui.add_space(8.0);
        ui.horizontal(|ui| {
            let enabled = self.can_write();
            if ui
                .add_enabled(enabled, egui::Button::new("Write"))
                .clicked()
            {
                self.start_write(ui.ctx().clone());
            }
            if self.busy {
                if ui.button("Cancel").clicked()
                    && let Some(cancel) = &self.cancel
                {
                    cancel.cancel();
                }
                ui.spinner();
            }
        });
        if previous_options != (self.mode, self.scheme, self.filesystem, self.verify) {
            self.confirmed = false;
        }
    }
}

impl eframe::App for App {
    // eframe 0.37 hands the app a root `Ui` instead of a `Context`.
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.handle_messages(&ctx);

        egui::Panel::top("header").show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.heading("image2usb");
                ui.label(
                    egui::RichText::new("bootable USB writer for macOS, Linux and Windows").weak(),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if !self.is_admin {
                        ui.colored_label(egui::Color32::YELLOW, "not elevated — writing will fail");
                    }
                });
            });
        });

        egui::Panel::bottom("status").show(ui, |ui| {
            ui.add_space(4.0);
            let fraction = self
                .progress
                .as_ref()
                .map(ProgressEvent::fraction)
                .unwrap_or(0.0);
            ui.add_sized(
                [ui.available_width(), 18.0],
                egui::ProgressBar::new(fraction).show_percentage(),
            );
            ui.horizontal(|ui| {
                ui.label(&self.status);
                if let Some(event) = &self.progress
                    && let Some(eta) = event.eta
                {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(format!("ETA {}s", eta.as_secs()));
                    });
                }
            });
            egui::ScrollArea::vertical()
                .stick_to_bottom(true)
                .max_height(120.0)
                .show(ui, |ui| {
                    for line in &self.log {
                        ui.small(line);
                    }
                });
        });

        egui::Panel::left("devices")
            .resizable(true)
            .default_size(380.0)
            .show(ui, |ui| {
                self.draw_devices(ui);
            });

        egui::CentralPanel::default().show(ui, |ui| {
            self.draw_image(ui);
        });

        if self.busy {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
    }
}

fn is_elevated() -> bool {
    #[cfg(unix)]
    {
        std::process::Command::new("id")
            .arg("-u")
            .output()
            .ok()
            .and_then(|out| String::from_utf8(out.stdout).ok())
            .is_some_and(|out| out.trim() == "0")
    }
    #[cfg(windows)]
    {
        // The Windows portable build embeds a requireAdministrator manifest;
        // reaching this point means the process is already elevated.
        true
    }
}
