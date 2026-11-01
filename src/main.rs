#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod config;
mod dots;
mod engine;
mod update;

use eframe::egui;
use std::{process::Command, thread, time::Duration};

fn main() -> eframe::Result {
    let args: Vec<String> = std::env::args().collect();
    let hidden = args.iter().any(|a| a == "--tray");
    let after_update = args.iter().any(|a| a == "--after-update");

    // The old instance needs a moment to exit after an update.
    let tries = if after_update { 40 } else { 1 };
    let mut instance = single_instance::SingleInstance::new("SWORTA-singleton").unwrap();
    for _ in 1..tries {
        if instance.is_single() {
            break;
        }
        drop(instance);
        thread::sleep(Duration::from_millis(250));
        instance = single_instance::SingleInstance::new("SWORTA-singleton").unwrap();
    }
    if !instance.is_single() {
        return Ok(());
    }
    update::cleanup_old();

    let viewport = egui::ViewportBuilder::default()
        .with_title("SWORTA")
        .with_inner_size([440.0, 680.0])
        .with_resizable(false)
        .with_maximize_button(false)
        .with_visible(!hidden)
        .with_icon(egui::IconData {
            rgba: app::icon_rgba(64),
            width: 64,
            height: 64,
        });

    let result = eframe::run_native(
        "SWORTA",
        eframe::NativeOptions { viewport, ..Default::default() },
        Box::new(|cc| Ok(Box::new(app::App::new(cc, hidden)))),
    );

    let restart = *update::RESTART.lock().unwrap();
    if let Some(silent) = restart {
        drop(instance);
        if let Ok(exe) = std::env::current_exe() {
            let mut cmd = Command::new(exe);
            cmd.arg("--after-update");
            if silent {
                cmd.arg("--tray");
            }
            let _ = cmd.spawn();
        }
    }
    result
}
