//! All application options live in a TOML config file; the only CLI argument is the config path.

use std::fs;
use std::path::Path;

use clap::Parser;
use serde::{Deserialize, Serialize};

#[derive(Parser, Debug)]
#[command(
    name = "handwriting-input",
    version,
    about = "Handwriting input keyboard for Wayland (egui + Google handwriting API)"
)]
pub struct Cli {
    /// Path to the TOML config file (created with defaults when missing)
    #[arg(default_value = default_config_path().as_os_str())]
    pub config: std::path::PathBuf,
}

fn default_config_path() -> std::path::PathBuf {
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        if !xdg.is_empty() {
            return std::path::PathBuf::from(xdg).join("handwriting-input").join("config.toml");
        }
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    std::path::PathBuf::from(home).join(".config").join("handwriting-input").join("config.toml")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SendMode {
    Auto,
    InputMethod,
    Virtual,
    Clipboard,
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ThemeMode {
    Light,
    Dark,
    Auto,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PositionMode {
    Bottom,
    Center,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LayerMode {
    Top,
    Overlay,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MultiLang {
    Auto,
    Parallel,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct Config {
    pub langs: Vec<String>,
    pub server_url: String,
    pub send: SendMode,
    pub num_candidates: usize,
    pub theme: ThemeMode,
    pub position: PositionMode,
    pub layer: LayerMode,
    pub height: u32,
    pub stroke_width: f32,
    pub key_delay_ms: u64,
    pub punctuation: Vec<String>,
    pub font_file: Option<String>,
    pub auto_cjk_font: bool,
    pub multi_lang: MultiLang,
    pub timeout_secs: u64,
    pub use_context: bool,
    pub context_chars: usize,
    pub preedit_preview: bool,
    pub clear_after_send: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            langs: vec!["en".to_string()],
            server_url: "https://www.google.com/inputtools/request?ime=handwriting&app=mobilesearch&cs=1&oe=UTF-8".to_string(),
            send: SendMode::Auto,
            num_candidates: 8,
            theme: ThemeMode::Auto,
            position: PositionMode::Bottom,
            layer: LayerMode::Top,
            height: 300,
            stroke_width: 6.0,
            key_delay_ms: 0,
            punctuation: vec![
                ",".to_string(),
                ".".to_string(),
                "?".to_string(),
                "!".to_string(),
                "'".to_string(),
            ],
            font_file: None,
            auto_cjk_font: true,
            multi_lang: MultiLang::Auto,
            timeout_secs: 6,
            use_context: true,
            context_chars: 40,
            preedit_preview: false,
            clear_after_send: true,
        }
    }
}

impl Config {
    pub fn load(path: &Path) -> Result<Config, String> {
        match fs::read_to_string(path) {
            Ok(s) => {
                let mut cfg: Config =
                    toml::from_str(&s).map_err(|e| format!("{}: {e}", path.display()))?;
                cfg.normalize();
                Ok(cfg)
            }
            Err(_) => {
                let cfg = Config::default();
                if let Some(dir) = path.parent() {
                    let _ = fs::create_dir_all(dir);
                }
                if let Ok(t) = toml::to_string_pretty(&cfg) {
                    let _ = fs::write(path, format!("# handwriting-input defaults\n{t}"));
                }
                Ok(cfg)
            }
        }
    }

    fn normalize(&mut self) {
        self.langs.retain(|l| !l.trim().is_empty());
        if self.langs.is_empty() {
            self.langs.push("en".to_string());
        }
        self.num_candidates = self.num_candidates.clamp(1, 64);
        self.height = self.height.clamp(120, 2000);
    }
}