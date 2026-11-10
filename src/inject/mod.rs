//! Text and key injection into the focused application.
//!
//! Three layers, chosen at commit time by [`crate::app::App::inject_text`]:
//! 1. `zwp_input_method_v2` — whole strings committed at the caret, live preedit,
//!    surrounding-text aware backspace, and the source of `pre_context`.
//! 2. `zwp_virtual_keyboard_v1` — keycodes through our generated XKB keymap.
//! 3. `zwlr_data_control_v1` — clipboard copy only (last resort).

pub mod keymap;

use std::os::fd::AsFd;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use wayland_client::QueueHandle;
use wayland_protocols_misc::zwp_input_method_v2::client::zwp_input_method_v2::ZwpInputMethodV2;
use wayland_protocols_wlr::data_control::v1::client::zwlr_data_control_device_v1::ZwlrDataControlDeviceV1;
use wayland_protocols_wlr::data_control::v1::client::zwlr_data_control_manager_v1::ZwlrDataControlManagerV1;

use crate::app::App;
use keymap::{keymap_memfd, KeymapBuilder};

pub use keymap::{KEYSYM_BACKSPACE, KEYSYM_RETURN};

/// Monotonic-ish timestamp in ms for `virtual_keyboard.key` events.
fn now_millis() -> u32 {
    static BASE: AtomicU32 = AtomicU32::new(0);
    let ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_millis())
        .unwrap_or(0);
    BASE.fetch_add(1, Ordering::Relaxed) ^ ms
}

// ---------------------------------------------------------------------------
// 1. input method
// ---------------------------------------------------------------------------

/// Double-buffered state delivered by `zwp_input_method_v2` events.
#[derive(Default)]
pub struct ImState {
    pub active: bool,
    pub serial: u32,
    pub unavailable: bool,
    /// Text around the caret, if the client supports it.
    pub surrounding: String,
    pub cursor: usize,
    pub anchor: usize,
    pub supports_surrounding: bool,
    /// Raw `zwp_text_input_v3.content_hint` / `.content_purpose` values.
    pub content_hint: u32,
    pub content_purpose: u32,
    /// Last preedit string sent, to avoid redundant round-trips.
    pub last_preedit_sent: Option<String>,
}

pub struct InputMethodCtl {
    pub im: ZwpInputMethodV2,
    pub state: ImState,
}

impl InputMethodCtl {
    pub fn new(im: ZwpInputMethodV2) -> Self {
        Self {
            im,
            state: ImState::default(),
        }
    }

    /// Commit a whole string at the caret (words, punctuation, candidate picks).
    pub fn commit_text(&self, text: &str) {
        self.im.set_preedit_string(String::new(), 0, 0);
        self.im.commit_string(text.to_string());
        self.im.commit(self.state.serial);
    }

    pub fn set_preedit(&self, text: &str) {
        self.im.set_preedit_string(text.to_string(), -1, -1);
        self.im.commit(self.state.serial);
    }

    pub fn clear_preedit(&self) {
        self.im.set_preedit_string(String::new(), 0, 0);
        self.im.commit(self.state.serial);
    }

    /// Delete text before the caret. Needs surrounding-text support; returns
    /// false when the client gave us nothing to work with.
    pub fn delete_before_caret(&self, bytes: Option<usize>) -> bool {
        if !self.state.supports_surrounding || self.state.cursor == 0 {
            return false;
        }
        let n = bytes.unwrap_or_else(|| prev_char_len(&self.state.surrounding, self.state.cursor));
        if n == 0 {
            return false;
        }
        self.im.delete_surrounding_text(n as u32, 0);
        self.im.set_preedit_string(String::new(), 0, 0);
        self.im.commit(self.state.serial);
        true
    }

    /// Up to `max_chars` of text just before the caret: the `pre_context` that
    /// makes Google's recogniser much better at connected handwriting.
    pub fn context_before(&self, max_chars: usize) -> Option<String> {
        if !self.state.supports_surrounding || self.state.cursor == 0 {
            return None;
        }
        let cut = self.state.cursor.min(self.state.surrounding.len());
        let mut out: Vec<char> = self.state.surrounding[..cut].chars().rev().take(max_chars).collect();
        out.reverse();
        let s: String = out.into_iter().collect();
        if s.is_empty() {
            None
        } else {
            Some(s)
        }
    }
}

/// UTF-8 length of the character ending just before byte offset `cursor`.
fn prev_char_len(s: &str, cursor: usize) -> usize {
    let cut = cursor.min(s.len());
    let mut idx = cut;
    while idx > 0 {
        if s.as_bytes()[idx - 1] & 0xC0 == 0x80 {
            idx -= 1;
        } else {
            break;
        }
    }
    cut - idx
}

// ---------------------------------------------------------------------------
// 2. virtual keyboard
// ---------------------------------------------------------------------------

pub struct VirtualKeyboard {
    vk: wayland_protocols_misc::zwp_virtual_keyboard_v1::client::zwp_virtual_keyboard_v1::ZwpVirtualKeyboardV1,
    keymap: KeymapBuilder,
    /// The memfd backing the current keymap must stay open while the compositor uses it.
    _keymap_file: Option<std::fs::File>,
}

impl VirtualKeyboard {
    pub fn new(vk: wayland_protocols_misc::zwp_virtual_keyboard_v1::client::zwp_virtual_keyboard_v1::ZwpVirtualKeyboardV1) -> Self {
        let mut this = Self {
            vk,
            keymap: KeymapBuilder::new(),
            _keymap_file: None,
        };
        this.push_keymap();
        this.vk.modifiers(0, 0, 0, 0);
        this
    }

    /// Send the generated keymap (XKB_V1 format) to the compositor.
    fn push_keymap(&mut self) {
        let text = self.keymap.keymap_text().to_owned();
        match keymap_memfd(&text) {
            Ok(file) => {
                self.vk.keymap(1, file.as_fd(), text.len() as u32);
                self._keymap_file = Some(file);
            }
            Err(e) => eprintln!("keymap memfd failed: {e}"),
        }
    }

    fn tap(&self, keycode: u16, delay: Duration) {
        self.vk.key(now_millis(), keycode as u32, 1);
        self.vk.key(now_millis(), keycode as u32, 0);
        if !delay.is_zero() {
            std::thread::sleep(delay);
        }
    }

    /// Type `text` one character at a time through the generated keymap.
    pub fn send_text(&mut self, text: &str, delay: Duration) -> Result<(), String> {
        if !self.keymap.ensure_text(text) {
            return Err("ran out of keymap slots for this text".into());
        }
        if self.keymap.is_dirty() {
            self.push_keymap();
        }
        for c in text.chars() {
            let action = self
                .keymap
                .action_for_char(c)
                .ok_or_else(|| format!("no keycode assigned for {c:?}"))?;
            self.tap(action.keycode, delay);
        }
        Ok(())
    }

    /// Press a special key (Enter, Backspace, Tab, Escape).
    pub fn send_keysym(&mut self, keysym: u32, delay: Duration) -> Result<(), String> {
        if let Some(action) = self.keymap.action_for_keysym(keysym) {
            self.tap(action.keycode, delay);
            return Ok(());
        }
        if let Some(c) = char::from_u32(keysym) {
            return self.send_text(&c.to_string(), delay);
        }
        Err(format!("cannot inject keysym {keysym:#x}"))
    }
}

// ---------------------------------------------------------------------------
// 3. clipboard (last resort)
// ---------------------------------------------------------------------------

/// Text handed to the data-control source when the compositor asks for it.
#[derive(Clone, Default)]
pub struct ClipboardPayload(pub Arc<Mutex<Option<String>>>);

pub struct ClipboardCtl {
    pub mgr: ZwlrDataControlManagerV1,
    pub device: ZwlrDataControlDeviceV1,
    pub payload: ClipboardPayload,
}

impl ClipboardCtl {
    pub fn new(mgr: ZwlrDataControlManagerV1, device: ZwlrDataControlDeviceV1) -> Self {
        Self {
            mgr,
            device,
            payload: ClipboardPayload::default(),
        }
    }

    /// Claim the clipboard with `text`. A fresh source per copy keeps older
    /// paste targets from seeing stale content.
    pub fn copy(&self, text: &str, qh: &QueueHandle<App>) {
        *self.payload.0.lock().unwrap() = Some(text.to_string());
        let source = self.mgr.create_data_source(qh, self.payload.clone());
        self.device.set_selection(Some(&source));
    }
}
