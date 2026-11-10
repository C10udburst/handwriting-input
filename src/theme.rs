//! egui visuals per config theme.

use crate::config::{Config, ThemeMode};

pub fn is_dark(cfg: &Config) -> bool {
    match cfg.theme {
        ThemeMode::Dark => true,
        ThemeMode::Light => false,
        // No portal theme query yet; dark matches the overlay-OSK aesthetic.
        ThemeMode::Auto => true,
    }
}

pub fn apply(ctx: &egui::Context, cfg: &Config) {
    ctx.set_visuals(if is_dark(cfg) {
        egui::Visuals::dark()
    } else {
        egui::Visuals::light()
    });
}

/// wgpu clear color matching the egui panel background.
pub fn clear_color(cfg: &Config) -> wgpu::Color {
    if is_dark(cfg) {
        wgpu::Color {
            r: 0.11,
            g: 0.11,
            b: 0.15,
            a: 1.0,
        }
    } else {
        wgpu::Color {
            r: 0.94,
            g: 0.94,
            b: 0.96,
            a: 1.0,
        }
    }
}
