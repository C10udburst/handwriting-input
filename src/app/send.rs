//! Injection decisions: which layer receives the text, and clipboard fallback.

use std::time::Duration;

use super::App;
use crate::app::Action;
use crate::config::SendMode;
use crate::inject::{KEYSYM_BACKSPACE, KEYSYM_RETURN};

impl App {
    fn key_delay(&self) -> Duration {
        Duration::from_millis(self.cfg.key_delay_ms)
    }

    /// True when the input method is usable right now (an edit field holds focus).
    pub fn im_ready(&self) -> bool {
        self.im
            .as_ref()
            .is_some_and(|c| c.state.active && !c.state.unavailable)
    }

    /// Send `text` to the focused application using the best available layer.
    pub fn inject_text(&mut self, text: &str) -> Result<(), String> {
        if text.is_empty() {
            return Ok(());
        }
        let delay = self.key_delay();
        match self.cfg.send {
            SendMode::None => Ok(()),
            SendMode::Clipboard => self.copy_to_clipboard(text),
            SendMode::InputMethod => match &self.im {
                Some(ctl) if self.im_ready() => {
                    ctl.commit_text(text);
                    Ok(())
                }
                _ => Err("input method not active (no text field focused)".into()),
            },
            SendMode::Virtual => match &mut self.vk {
                Some(vk) => vk.send_text(text, delay),
                None => Err("compositor offers no virtual keyboard".into()),
            },
            SendMode::Auto => {
                if self.im_ready() {
                    if let Some(ctl) = &self.im {
                        ctl.commit_text(text);
                        return Ok(());
                    }
                }
                if let Some(vk) = &mut self.vk {
                    return vk.send_text(text, delay);
                }
                self.copy_to_clipboard(text)
            }
        }
    }

    pub fn inject_backspace(&mut self) -> Result<(), String> {
        let delay = self.key_delay();
        let mode = self.cfg.send;
        if matches!(mode, SendMode::Auto | SendMode::InputMethod) && self.im_ready() {
            if let Some(ctl) = &self.im {
                if ctl.delete_before_caret(None) {
                    return Ok(());
                }
            }
        }
        if matches!(mode, SendMode::Auto | SendMode::Virtual) {
            if let Some(vk) = &mut self.vk {
                return vk.send_keysym(KEYSYM_BACKSPACE, delay);
            }
        }
        Err("no way to send Backspace (input method lacks surrounding-text support and no virtual keyboard)".into())
    }

    pub fn inject_enter(&mut self) -> Result<(), String> {
        // input-method has no keysym request; Enter is always a key press.
        let delay = self.key_delay();
        match &mut self.vk {
            Some(vk) => vk.send_keysym(KEYSYM_RETURN, delay),
            None => Err("Enter requires zwp_virtual_keyboard_v1".into()),
        }
    }

    pub fn copy_to_clipboard(&mut self, text: &str) -> Result<(), String> {
        match &self.clipboard {
            Some(ctl) => {
                let qh = self.qh.clone();
                ctl.copy(text, &qh);
                Ok(())
            }
            None => Err("no clipboard device (zwlr_data_control unavailable)".into()),
        }
    }

    /// Execute one UI intent.
    pub fn do_action(&mut self, action: Action) {
        let delay = self.key_delay();
        let result = match action {
            Action::CommitText { text, clear } => {
                let r = self.inject_text(&text);
                if r.is_ok() && clear && self.cfg.clear_after_send {
                    self.clear_canvas();
                }
                r
            }
            Action::Keysym(KEYSYM_BACKSPACE) => self.inject_backspace(),
            Action::Keysym(KEYSYM_RETURN) => self.inject_enter(),
            Action::Keysym(k) => match &mut self.vk {
                Some(vk) => vk.send_keysym(k, delay),
                None => Err("no virtual keyboard".into()),
            },
            Action::ClearCanvas => {
                self.clear_canvas();
                return;
            }
            Action::UndoStroke => {
                self.undo_stroke();
                return;
            }
            Action::Space => {
                let r = self.inject_text(" ");
                self.last_error = r.err();
                return;
            }
            Action::Quit => {
                self.quit = true;
                return;
            }
        };
        self.last_error = result.err();
        self.dirty = true;
    }

    /// Mirror the first candidate into the app as preedit while drawing.
    fn sync_preedit_common(&mut self) -> Option<Option<String>> {
        if !self.cfg.preedit_preview {
            return None;
        }
        let want = if self.cur.is_some() && !self.ink.is_empty() {
            self.candidates.first().map(|c| c.text.clone())
        } else {
            None
        };
        Some(want)
    }

    pub fn sync_preedit(&mut self) {
        let Some(want) = self.sync_preedit_common() else {
            return;
        };
        let Some(ctl) = self.im.as_mut() else {
            return;
        };
        if !ctl.state.active || ctl.state.unavailable {
            return;
        }
        if want != ctl.state.last_preedit_sent {
            match &want {
                Some(text) => ctl.set_preedit(text),
                None => ctl.clear_preedit(),
            }
            ctl.state.last_preedit_sent = want;
        }
    }
}
