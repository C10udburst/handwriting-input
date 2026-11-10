//! Optional CJK fallback font (Latin coverage comes with egui's built-in fonts).

use egui::{FontData, FontDefinitions};

use crate::config::Config;

/// System locations tried when `auto_cjk_font` is enabled.
const CJK_CANDIDATES: &[&str] = &[
    "/run/current-system/sw/share/opentype/noto/NotoSansCJK-Regular.ttc",
    "/run/current-system/sw/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
    "/run/current-system/sw/share/fonts/truetype/noto/NotoSansCJK-Regular.ttc",
    "/run/current-system/sw/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc",
    "/run/current-system/sw/share/fonts/truetype/wqy/wqy-zenhei.ttc",
    "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
    "/usr/share/fonts/truetype/noto/NotoSansCJK-Regular.ttc",
];

/// Append the font named `cjk` to every family list so it acts as a fallback
/// for glyphs the primary fonts lack.
pub fn install(ctx: &egui::Context, cfg: &Config) {
    let mut paths: Vec<std::path::PathBuf> = Vec::new();
    if let Some(f) = &cfg.font_file {
        paths.push(std::path::PathBuf::from(f));
    }
    if cfg.auto_cjk_font {
        paths.extend(CJK_CANDIDATES.iter().map(std::path::PathBuf::from));
    }

    let mut defs = FontDefinitions::default();
    for path in paths {
        if let Ok(bytes) = std::fs::read(&path) {
            defs.font_data
                .insert("cjk".to_string(), std::sync::Arc::new(FontData::from_owned(bytes)));
            for family in defs.families.values_mut() {
                if !family.iter().any(|f| f == "cjk") {
                    family.push("cjk".to_string());
                }
            }
            ctx.set_fonts(defs);
            return;
        }
    }
}
