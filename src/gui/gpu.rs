//! The GPU page: every NVIDIA card in this PC as read from the driver at startup, whether it is
//! supported, and where the FanConnect II controller was found.

use eframe::egui::{self, CornerRadius, Margin, RichText};
use gpu_fanctl::fanconnect::{ADDRESS, PORT, SUPPORTED_CARDS, format_pci_ids};
use gpu_fanctl::gpu::GpuInfo;

use crate::live::{Live, Source};
use crate::theme::Palette;
use crate::widgets::{badge, card, card_title, info_grid, info_grid_rich};

fn gpu_entry(ui: &mut egui::Ui, index: usize, gpu: &GpuInfo, driver: &str) {
    let p = Palette::current(ui.ctx());
    egui::Frame::new()
        .fill(p.plot_bg)
        .stroke(egui::Stroke::new(1.0, p.card_stroke))
        .corner_radius(CornerRadius::same(12))
        .inner_margin(Margin::same(14))
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(RichText::new(&gpu.name).size(17.0).strong());
                match gpu.supported {
                    Some(_) => badge(ui, "Supported", p.ok),
                    None => badge(ui, "Not supported", p.weak),
                }
            });
            let mut rows = vec![
                ("PCI IDs", format_pci_ids(gpu.device_id, gpu.subsystem_id)),
                ("PCI address", gpu.pci_address.clone()),
            ];
            if let Some(card) = gpu.supported {
                rows.insert(0, ("Model", card.name.to_string()));
            }
            if let Some(vbios) = &gpu.vbios {
                rows.push(("VBIOS", vbios.clone()));
            }
            if let Some(bytes) = gpu.memory_bytes {
                rows.push(("Memory", format!("{:.0} GB", bytes as f64 / 1024f64.powi(3))));
            }
            rows.push(("Driver", driver.to_string()));
            info_grid(ui, &format!("gpu{index}"), &rows);
        });
}

pub fn page(ui: &mut egui::Ui, live: &Live) {
    let p = Palette::current(ui.ctx());

    card(ui, |ui| {
        card_title(ui, "Graphics cards", "Read from the NVIDIA driver when the app starts");
        match &live.system {
            None => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Reading graphics cards…");
                });
            }
            Some(Err(e)) => {
                ui.label(RichText::new(format!("Could not read the graphics cards: {e}")).color(p.danger));
            }
            Some(Ok(system)) if system.gpus.is_empty() => {
                ui.label(RichText::new("No NVIDIA graphics card found.").color(p.danger));
            }
            Some(Ok(system)) => {
                let driver = system.driver_version.clone().unwrap_or_else(|| "unknown".into());
                for (index, gpu) in system.gpus.iter().enumerate() {
                    gpu_entry(ui, index, gpu, &driver);
                }
                if system.gpus.iter().all(|g| g.supported.is_none()) {
                    let supported: Vec<&str> = SUPPORTED_CARDS.iter().map(|c| c.name).collect();
                    ui.label(
                        RichText::new(format!(
                            "None of these cards is supported. Supported: {}.",
                            supported.join(", ")
                        ))
                        .color(p.warn),
                    );
                }
            }
        }
    });

    card(ui, |ui| {
        card_title(ui, "Fan controller", "The FanConnect II headers on the supported card");
        let found = match (&live.controller, live.source) {
            (Some(location), _) => RichText::new(format!("Found ({location})")).color(p.ok),
            // A service from an older version doesn't publish the location.
            (None, Some(Source::Daemon)) => RichText::new("Found by the running service").color(p.ok),
            (None, _) => RichText::new(live.error.clone().unwrap_or_else(|| "Not found".into())).color(p.danger),
        };
        let source = match live.source {
            Some(Source::Daemon) => "The running service",
            Some(Source::Direct) => "Read directly from the card",
            None => "Not available",
        };
        // One table for all rows, so the values line up.
        info_grid_rich(
            ui,
            "controller",
            vec![
                ("Status", found),
                ("Chip", RichText::new(format!("FanConnect II, I2C address 0x{ADDRESS:02X} on GPU I2C port {PORT}"))),
                ("Outputs", RichText::new("2 × 4-pin PWM with tachometer, one shared duty")),
                ("Data source", RichText::new(source)),
            ],
        );
    });
}
