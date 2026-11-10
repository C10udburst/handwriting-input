//! All application options live in a TOML config file; the only CLI argument is the config path.

use std::fs;
use std::path::{Path, PathBuf};

use clap::Parser;
use serde::{Deserialize, Serialize};

fn default_config_path() -> PathBuf {
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        if !xdg.is_empty() {
            return PathBuf::from(xdg).join("handwriting-input").join("config.toml");
        }
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home)
        .join(".config")
        .join("handwriting-input")
        .join("config.toml")
}

#[derive(Parser, Debug)]
#[command(
    name = "handwriting-input",
    version,
    about = "Handwriting input keyboard for Wayland (egui + Google handwriting API)"
)]
pub struct Cli {
    /// Path to the TOML config file (created with defaults when missing)
    #[arg(default_value_os_t = default_config_path())]
    pub config: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SendMode {
    /// input method when a text field is focused, else virtual keyboard, else clipboard
    Auto,
    /// only zwp_input_method_v2 (commit_string / preedit / surrounding delete)
    InputMethod,
    /// only zwp_virtual_keyboard_v1 (keysym typing)
    Virtual,
    /// never type: copy to clipboard instead
    Clipboard,
    /// do nothing on commit
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
    /// one request with a language array; falls back to per-language requests
    Auto,
    /// always one request per language
    Parallel,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(
    default,
    rename_all = "kebab-case",
    // A typo in the config should be an error, not a silently ignored option.
    deny_unknown_fields
)]
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
    /// Read `path`; when missing, write the defaults there and use them.
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
        self.context_chars = self.context_chars.clamp(0, 512);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partial_files_inherit_the_defaults() {
        let cfg: Config = toml::from_str("langs = [\"pl\", \"en\"]\nheight = 420\n").unwrap();
        assert_eq!(cfg.langs, vec!["pl", "en"]);
        assert_eq!(cfg.height, 420);
        assert_eq!(cfg.num_candidates, Config::default().num_candidates);
        assert_eq!(cfg.send, SendMode::Auto);
    }

    #[test]
    fn options_use_kebab_case_names() {
        let cfg: Config = toml::from_str(
            "send = \"input-method\"\ntheme = \"light\"\nposition = \"center\"\nlayer = \"overlay\"\nmulti-lang = \"parallel\"\n",
        )
        .unwrap();
        assert_eq!(cfg.send, SendMode::InputMethod);
        assert_eq!(cfg.theme, ThemeMode::Light);
        assert_eq!(cfg.position, PositionMode::Center);
        assert_eq!(cfg.layer, LayerMode::Overlay);
        assert_eq!(cfg.multi_lang, MultiLang::Parallel);
    }

    #[test]
    fn unknown_options_are_reported_instead_of_ignored() {
        let err = toml::from_str::<Config>("not-an-option = 1").unwrap_err().to_string();
        assert!(err.contains("not-an-option"), "got: {err}");
    }

    #[test]
    fn normalize_repairs_useless_values() {
        let mut cfg = Config::default();
        cfg.langs = vec!["".to_string(), "  ".to_string()];
        cfg.num_candidates = 0;
        cfg.height = 1;
        cfg.normalize();
        assert_eq!(cfg.langs, vec!["en".to_string()]);
        assert_eq!(cfg.num_candidates, 1);
        assert_eq!(cfg.height, 120);
    }

    #[test]
    fn a_missing_file_is_created_with_defaults_and_round_trips() {
        let dir = std::env::temp_dir().join(format!("handwriting-config-{}", std::process::id()));
        let path = dir.join("config.toml");
        let _ = fs::remove_file(&path);

        let first = Config::load(&path).expect("defaults");
        assert!(path.is_file(), "the default file should be written");

        let mut changed = first.clone();
        changed.height = 444;
        fs::write(&path, toml::to_string_pretty(&changed).unwrap()).unwrap();
        assert_eq!(Config::load(&path).unwrap().height, 444);

        let _ = fs::remove_dir_all(&dir);
    }
}
