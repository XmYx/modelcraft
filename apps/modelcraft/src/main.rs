//! ModelCraft desktop app.
//!
//! Usage: `modelcraft [--demo model|sculpt] [--workspace …] [--quad] [--screenshot <file.png>]
//! [--bench-viewport <frames>] [file]`; `--help` prints the options.
#![cfg_attr(all(target_os = "windows", not(debug_assertions)), windows_subsystem = "windows")]
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable))]
#![forbid(unsafe_code)]

#[macro_use]
mod i18n;
mod actions;
mod agents;
mod anim_ui;
#[cfg(test)]
mod anim_ui_tests;
mod app;
mod args;
mod clone_ui;
#[cfg(test)]
mod clone_ui_tests;
mod constraint_ui;
#[cfg(test)]
mod constraint_ui_tests;
mod demo;
mod exchange_ui;
mod gpu;
mod keys;
mod material_ui;
mod mesh_tools;
mod panels;
mod prefs;
mod prefs_ui;
mod radial;
mod render_view;
mod rig_ui;
#[cfg(test)]
mod rig_ui_tests;
mod sculpt_ui;
#[cfg(test)]
mod sculpt_ui_tests;
mod shelf;
mod theme;
#[cfg(test)]
mod ui_tests;
mod viewport;

use args::{Cli, Launch};

/// Reverse-DNS app id: Linux `.desktop` file, hicolor icon name and Wayland app id.
const APP_ID: &str = "ai.storyteller.modelcraft";

fn main() -> eframe::Result {
    let cli = match args::parse(std::env::args().skip(1)) {
        Ok(Launch::Run(cli)) => cli,
        Ok(Launch::Print(text)) => {
            print!("{text}");
            return Ok(());
        }
        Err(e) => {
            eprintln!("modelcraft: {e}\n\n{}", args::USAGE);
            std::process::exit(2);
        }
    };
    let Cli { screenshot, bench_frames, .. } = &cli;
    let size = if screenshot.is_some() || bench_frames.is_some() { [1600.0, 1000.0] } else { [1500.0, 920.0] };
    let icon =
        eframe::icon_data::from_png_bytes(include_bytes!("../../../assets/app-icon/hicolor/256x256/apps/ai.storyteller.modelcraft.png")).unwrap_or_default();
    let mut options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("ModelCraft")
            .with_app_id(APP_ID)
            .with_icon(icon)
            .with_inner_size(size)
            .with_min_inner_size([900.0, 600.0]),
        renderer: eframe::Renderer::Wgpu,
        ..Default::default()
    };
    if bench_frames.is_some() {
        // Measure the renderer, not the display's refresh rate.
        options.wgpu_options.surface.present_mode = wgpu::PresentMode::AutoNoVsync;
    }
    eframe::run_native("ModelCraft", options, Box::new(move |cc| Ok(Box::new(app::ModelApp::new(cc, cli)))))
}
