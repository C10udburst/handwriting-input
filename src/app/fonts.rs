//! Font setup.
//!
//! Latin coverage comes from egui's bundled fonts (`default_fonts`). CJK
//! candidates need a real CJK font, which we look for in the fontconfig font
//! directories (this is how it works on both NixOS and FHS systems).

use std::path::{Path, PathBuf};

use egui::FontData;

use crate::config::Config;

/// File-name fragments that suggest a font covers CJK.
const CJK_HINTS: &[&str] = &[
    "notosanscjk",
    "notoserifcjk",
    "notosansmono",
    "sourcehan",
    "wqy",
    "droidsansfallback",
    "arphic",
    "uming",
    "ukai",
    "cjk",
];

/// Last-resort absolute paths (FHS layouts).
const CJK_FALLBACK_PATHS: &[&str] = &[
    "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
    "/usr/share/fonts/truetype/noto/NotoSansCJK-Regular.ttc",
    "/usr/share/fonts/truetype/wqy/wqy-zenhei.ttc",
    "/usr/share/fonts/opentype/source-han-sans/SourceHanSans-Regular.otf",
];

/// Append a `cjk` fallback font to every family, if one can be found.
pub fn install(ctx: &egui::Context, cfg: &Config) {
    let Some(path) = find_cjk_font(cfg) else {
        eprintln!(
            "note: no CJK font found; Chinese/Japanese/Korean candidates may render as boxes \
             (set font_file in the config to fix)"
        );
        return;
    };
    let Ok(bytes) = std::fs::read(&path) else {
        eprintln!("warning: cannot read font {}", path.display());
        return;
    };
    eprintln!("using CJK fallback font {}", path.display());

    let mut defs = egui::FontDefinitions::default();
    defs.font_data.insert(
        "cjk".to_string(),
        std::sync::Arc::new(FontData::from_owned(bytes)),
    );
    for family in defs.families.values_mut() {
        if !family.iter().any(|name| name == "cjk") {
            family.push("cjk".to_string());
        }
    }
    ctx.set_fonts(defs);
}

fn find_cjk_font(cfg: &Config) -> Option<PathBuf> {
    if let Some(file) = &cfg.font_file {
        let path = PathBuf::from(file);
        if path.is_file() {
            return Some(path);
        }
        eprintln!("warning: configured font_file {} not found", path.display());
    }
    if !cfg.auto_cjk_font {
        return None;
    }
    // Directories fontconfig knows about (covers NixOS store paths).
    for dir in fontconfig_dirs() {
        if let Some(found) = scan_for_cjk(&dir) {
            return Some(found);
        }
    }
    CJK_FALLBACK_PATHS
        .iter()
        .map(Path::new)
        .find(|p| p.is_file())
        .map(Path::to_path_buf)
}

/// `<dir>` entries of the system fontconfig configuration.
fn fontconfig_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    for conf in [
        PathBuf::from("/etc/fonts/fonts.conf"),
        PathBuf::from("/run/current-system/sw/etc/fonts/fonts.conf"),
    ] {
        let Ok(text) = std::fs::read_to_string(&conf) else {
            continue;
        };
        let mut rest = text.as_str();
        while let Some(start) = rest.find("<dir>") {
            let after = &rest[start + 5..];
            let Some(end) = after.find("</dir>") else {
                break;
            };
            let raw = after[..end].trim();
            // fontconfig's own home token, plus the plain form we can use.
            let expanded = raw.replace("~", &std::env::var("HOME").unwrap_or_default());
            let path = PathBuf::from(expanded);
            if path.is_dir() {
                dirs.push(path);
            }
            rest = &after[end + 6..];
        }
    }
    dirs
}

/// Bounded depth-first search of a font directory (Nix store layouts are
/// `share/fonts/<family>/<file>`, so two levels are not enough).
fn scan_for_cjk(dir: &Path) -> Option<PathBuf> {
    const MAX_DEPTH: usize = 5;
    let mut stack = vec![(dir.to_path_buf(), 0usize)];
    let mut visited = 0usize;
    while let Some((path, depth)) = stack.pop() {
        visited += 1;
        if visited > 2000 {
            return None;
        }
        let Ok(entries) = std::fs::read_dir(&path) else {
            continue;
        };
        for entry in entries.flatten() {
            let child = entry.path();
            if child.is_file() {
                if looks_cjk(&child) {
                    return Some(child);
                }
            } else if depth < MAX_DEPTH {
                stack.push((child, depth + 1));
            }
        }
    }
    None
}

fn looks_cjk(path: &Path) -> bool {
    match path.extension().and_then(|e| e.to_str()) {
        Some("ttf") | Some("otf") | Some("ttc") | Some("otc") => {}
        _ => return false,
    }
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    CJK_HINTS.iter().any(|hint| name.contains(hint))
}
