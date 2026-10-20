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
    CancelToken, DeviceInfo, Phase, ProgressEvent, ProgressSink, ResolvedMode, Result, VerifyMode,
    WriteOptions, check_target, verify_image, write_image,
};
use image2usb_extract::extract_image;
use image2usb_iso::{Distro, IsoInfo, analyze_path};
use image2usb_platform::native;

fn main() -> eframe::Result {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(std::env::var("RUST_LOG").unwrap_or_else(|_| "warn".to_owned()))
        .try_init();

    let options = eframe::NativeOptions {
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

    iso_path: String,
    analysis: Option<IsoInfo>,
    analysis_error: Option<String>,

    mode: ModeChoice,
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
            iso_path: String::new(),
            analysis: None,
            analysis_error: None,
            mode: ModeChoice::Auto,
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
        let path = PathBuf::from(self.iso_path.trim());
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
            let result = analyze_path(&path).map_err(|err| err.to_string());
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

    fn can_write(&self) -> bool {
        !self.busy
            && self.confirmed
            && self.analysis.is_some()
            && self.selected_device().is_some_and(|d| d.is_safe_target())
            && self.resolved_mode().is_some()
            && !self.extract_windows()
    }

    fn extract_windows(&self) -> bool {
        self.resolved_mode() == Some(ResolvedMode::Extract)
            && self
                .analysis
                .as_ref()
                .is_some_and(|info| info.distro == Distro::Windows)
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
        // Extraction needs room for FAT metadata and the full tree, so require
        // at least the image size and never less than 64 MiB.
        let required = if mode == ResolvedMode::Extract {
            info.size.max(64 * 1024 * 1024)
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

        let iso = PathBuf::from(self.iso_path.trim());
        let cancel = CancelToken::new();
        self.cancel = Some(cancel.clone());
        self.busy = true;
        self.confirmed = false;
        self.progress = Some(ProgressEvent::message(Phase::Opening, "opening device"));
        self.status = "Opening device...".to_owned();
        self.log(format!("Writing {} to {}", iso.display(), device.id));

        let tx = self.tx.clone();
        let verify = self.verify;
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
                Msg::Devices(Ok(devices)) => {
                    self.devices = devices;
                    self.devices_error = None;
                    if self.selected.as_ref().is_some_and(|id| {
                        !self.devices.iter().any(|d| d.id.as_str() == id.as_str())
                    }) {
                        self.selected = None;
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

        let mut clicked = None;
        egui::ScrollArea::vertical()
            .max_height(420.0)
            .show(ui, |ui| {
                for device in &self.devices {
                    let safe = device.is_safe_target();
                    let selected = self.selected.as_deref() == Some(device.id.as_str());
                    let label = format!(
                        "{}  •  {}  •  {}{}",
                        device.model,
                        format_bytes(device.size),
                        device.bus,
                        if safe { "" } else { "  •  BLOCKED" }
                    );
                    let response = ui.add_enabled(
                        safe && !self.busy,
                        egui::Button::selectable(selected, label),
                    );
                    if response.clicked() {
                        clicked = Some(device.id.as_str().to_owned());
                    }
                    ui.horizontal(|ui| {
                        ui.small(device.id.as_str());
                        if !device.mount_points.is_empty() {
                            ui.small(format!("mounted: {}", device.mount_points.join(", ")));
                        }
                    });
                    if !safe {
                        ui.small(
                            egui::RichText::new(if device.is_system {
                                "system disk — never writable"
                            } else {
                                "internal disk — blocked"
                            })
                            .color(egui::Color32::LIGHT_RED),
                        );
                    }
                    ui.add_space(6.0);
                }
            });
        if let Some(id) = clicked {
            self.selected = Some(id);
            self.confirmed = false;
        }
    }

    fn draw_image(&mut self, ui: &mut egui::Ui) {
        ui.heading("Image");
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            let response = ui.add(
                egui::TextEdit::singleline(&mut self.iso_path)
                    .hint_text("/path/to/image.iso")
                    .desired_width(420.0),
            );
            if response.changed() {
                self.analysis = None;
                self.analysis_error = None;
                self.confirmed = false;
            }
            if ui.button("Browse…").clicked()
                && let Some(path) = rfd::FileDialog::new()
                    .add_filter("Disk images", &["iso", "img"])
                    .pick_file()
            {
                self.iso_path = path.display().to_string();
                self.analyze(ui.ctx().clone());
            }
            if ui.button("Analyze").clicked() {
                self.analyze(ui.ctx().clone());
            }
        });

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
            if self.extract_windows() {
                ui.colored_label(
                    egui::Color32::from_rgb(255, 120, 120),
                    "Windows extraction is not implemented yet (needs wimlib/split-WIM). Use DD.",
                );
            } else {
                ui.colored_label(
                    egui::Color32::YELLOW,
                    "Extraction creates an MBR + FAT32 volume from the ISO tree. UEFI boot is \
                     supported; legacy BIOS bootloaders are not installed yet.",
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
