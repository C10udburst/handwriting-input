//! Text/key injection into the focused application.
//!
//! Layers (selection order decided in `inject_text`):
//! 1. `zwp_input_method_v2` — whole-string commits at the caret (primary).
//! 2. `zwp_virtual_keyboard_v1` — keys routed through our XKB keymap (fallback).
//! 3. Clipboard via ext-data-control — copy only.

use std::io::Write;
use std::sync::Mutex;

use wayland_client::protocol::wl_surface::WlSurface;
use wayland_client::{Connection, QueueHandle};
use wayland_protocols_misc::zwp_input_method_v2::client::zwp_input_method_v2::{
    ZwpInputMethodV2,
};
use wayland_protocols_wlr::data_control::v1::client::zwlr_data_control_device_v1::{
    ZwlrDataControlDeviceV1, ZwlrDataControlDeviceV1Event,
};
use wayland_protocols_wlr::data_control::v1::client::zwlr_data_control_source_v1::{
    ZwlrDataControlSourceV1, ZwlrDataControlSourceV1Event,
};

use crate::inject::keymap::{action_for_char, assign, ensure_text, action_for_keysym, codes, chars, dirty, render, keymap_text, keymap_memfd, code_max, next_dynamic, dynamic_order, extended_static};

/// Mirrors the double-buffered state delivered by `zwp_input_method_v2` events.
#[derive(Default, Clone)]
pub struct ImState {
    pub active: bool,
    pub serial: u32,
    pub surrounding: String,
    pub cursor: usize,
    pub anchor: usize,
    pub supports_surrounding: bool,
    pub saw_surrounding_or_done: bool,
    pub content_purpose: u32,
    pub content_hint: u32,
    pub unavailable: bool,
    pub last_preedit_sent: Option<String>,
}

pub struct InputMethodCtl {
    pub im: ZwpInputMethodV2,
    pub state: ImState,
}

impl InputMethodCtl {
    pub fn commit_text(&self, text: &str) {
        self.im.commit_string(text.to_string());
        self.im.set_preedit_string(String::new(), 0, 0);
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

    /// Delete `bytes` characters before the caret (using surrounding text info).
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

    pub fn context_before(&self, max_chars: usize) -> Option<String> {
        if !self.state.supports_surrounding || self.state.cursor == 0 {
            return None;
        }
        let cut = self.state.cursor.min(self.state.surrounding.len());
        let before = &self.state.surrounding[..cut];
        let mut out: Vec<char> = before.chars().rev().take(max_chars).collect();
        out.reverse();
        let s: String = out.into_iter().collect();
        if s.is_empty() {
            None
        } else {
            Some(s)
        }
    }
}

fn prev_char_len(s: &str, cursor: usize) -> usize {
    let cut = cursor.min(s.len());
    let mut idx = cut;
    while idx > 0 {
        let prev = s.as_bytes()[idx - 1];
        if prev & 0xC0 == 0x80 {
            idx -= 1;
            continue;
        }
        break;
    }
    cut - idx
}

// ---------------------------------------------------------------------------
// virtual keyboard (fallback + keys)
// ---------------------------------------------------------------------------

pub struct VirtualKeyboard {
    vk: ZwpVirtualKeyboardV1,
    pub keymap: KeymapBuilder,
    _keymap_file: Option<std::fs::File>,
}

impl VirtualKeyboard {
    pub fn new(vk: ZwpVirtualKeyboardV1) -> Self {
        let mut kb = Self {
            vk,
            keymap: KeymapBuilder::new(),
            _keymap_file: None,
        };
        kb.push_keymap();
        kb.vk.modifiers(0, 0, 0, 0);
        kb
    }

    fn push_keymap(&mut self) {
        let text = self.keymap.keymap_text().to_owned();
        match keymap_memfd(&text) {
            Ok(file) => {
                let size = text.len() as u32;
                self.vk.keymap(1, &file, size); // KEYMAP_FORMAT_XKB_V1 = 1
                self._keymap_file = Some(file);
            }
            Err(e) => eprintln!("keymap memfd failed: {e}"),
        }
    }

    fn press_release(&self, act: KeyAction, delay: std::time::Duration) {
        if act.shift {
            // In our keymap the shift level is declared via types, not a hardware key.
            self.vk.modifiers(1 << 0, 0, 0, 0);
        }
        self.vk.key(now_millis(), act.keycode as u32, 1); // PRESSED
        self.vk.key(now_millis(), act.keycode as u32, 0); // RELEASED
        if act.shift {
            self.vk.modifiers(0, 0, 0, 0);
        }
        if !delay.is_zero() {
            std::thread::sleep(delay);
        }
    }

    pub fn send_text(&mut self, text: &str, delay: std::time::Duration) -> Result<(), String> {
        if !ensure_text(&mut self.keymap, text) {
            return Err("keymap space exhausted".into());
        }
        if dirty(&self.keymap) {
            self.push_keymap();
        }
        for c in text.chars() {
            let act = action_for_char(&self.keymap, c)
                .ok_or_else(|| format!("no keymap slot for {c:?}"))?;
            self.press_release(act, delay);
        }
        Ok(())
    }

    pub fn send_keysym(&mut self, keysym: u32, delay: std::time::Duration) -> Result<(), String> {
        if let Some(act) = action_for_keysym(&self.keymap, keysym) {
            self.press_release(act, delay);
            return Ok(());
        }
        if let Some(c) = char::from_u32(keysym) {
            return self.send_text(&c.to_string(), delay);
        }
        Err(format!("cannot inject keysym {keysym:#x}"))
    }
}

// ---------------------------------------------------------------------------
// clipboard (last resort)
// ---------------------------------------------------------------------------

/// Payload shared with ext-data-control source dispatch.
#[derive(Clone, Default)]
pub struct ClipboardPayload(pub Arc<Mutex<Option<String>>>);

pub struct ClipboardCtl {
    pub mgr: ZwlrDataControlManagerV1,
    pub device: ZwlrDataControlDeviceV1,
    pub payload: ClipboardPayload,
}