//! Self-contained XKB keymap generation for `zwp_virtual_keyboard_v1`.
//!
//! A virtual keyboard carries *its own* keymap: the compositor derives the
//! keysym (hence the character) of every injected keycode from this text. The
//! keymap is generated from scratch — no `include "..."`, because XKB data
//! files cannot be assumed to exist at store paths on every system.
//!
//! Design: **one keycode per character**, no shift levels. Shift requires a
//! modifier map that a minimal generated keymap cannot express reliably, and
//! the virtual keyboard protocol's `modifiers` request is not honoured by all
//! compositors. Static slots cover printable ASCII plus extended Latin (Polish,
//! German, French, Spanish, Nordic, Czech/Slavic, Turkish, Romanian) and common
//! punctuation; remaining characters (CJK candidates and other rare glyphs) are
//! assigned dynamically and recycled LRU-style when the keycode space is full.

use std::collections::BTreeMap;
use std::io::{Seek, SeekFrom, Write};
use std::os::fd::FromRawFd;

pub const KEYSYM_BACKSPACE: u32 = 0xff08;
pub const KEYSYM_RETURN: u32 = 0xff0d;
pub const KEYSYM_TAB: u32 = 0xff09;
pub const KEYSYM_ESCAPE: u32 = 0xff1b;

/// Standard evdev codes for the special keys (also reserved from char use).
const KEY_ESC: u16 = 1;
const KEY_BACKSPACE: u16 = 14;
const KEY_TAB: u16 = 15;
const KEY_RETURN: u16 = 28;

/// Keycodes left alone so that modifiers and other hardware keys never collide.
const RESERVED: &[u16] = &[
    KEY_ESC, KEY_BACKSPACE, KEY_TAB, KEY_RETURN, 29, 36, 42, 50, 54, 62, 64, 97, 100, 105, 108,
];

const CODE_MIN: u16 = 8;
const CODE_MAX: u16 = 255;

/// A single key press: which keycode to tap. No modifiers, by design.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyAction {
    pub keycode: u16,
}

/// XKB keysym for a Unicode character (the `U+` convention).
pub fn keysym_of(c: char) -> u32 {
    let cp = c as u32;
    if cp < 0x100 {
        cp
    } else {
        0x0100_0000 + cp
    }
}

/// Characters that get a fixed keycode at startup.
fn static_chars() -> Vec<char> {
    let mut out: Vec<char> = (0x20u32..0x7f).filter_map(char::from_u32).collect();
    const LATIN: &str = concat!(
        "ąćęłńóśźżĄĆĘŁŃÓŚŹŻ",          // Polish
        "äöüÄÖÜß€",                      // German
        "àâçéèêëîïôùûüÿœŒÀÂÇÉÈÊËÎÏÔÙÛÜ", // French
        "ñ¿¡",                           // Spanish
        "åæøÅÆØãõÃÕ",                    // Nordic / Portuguese
        "řžěščůďťňŘŽĚŠČŮĎŤŇđĐ",         // Czech / Slovak / Slovene / Croatian
        "ğışĞİŞ",                        // Turkish
        "șțȘȚĂÎ",                        // Romanian
        "—–…‘’“”«»°·±×÷½¼¾£¥",          // punctuation
    );
    out.extend(LATIN.chars());
    out.sort_unstable();
    out.dedup();
    out
}

/// Builds and caches the keymap text while allocating keycodes on demand.
pub struct KeymapBuilder {
    /// keycode -> keysym
    codes: BTreeMap<u16, u32>,
    /// character -> keycode
    chars: BTreeMap<char, u16>,
    /// special keysym -> keycode
    specials: BTreeMap<u32, u16>,
    /// hint for the next fresh keycode scan
    next_code: u16,
    /// LRU order of dynamically assigned characters (oldest first)
    dynamic_order: Vec<char>,
    dirty: bool,
    rendered: String,
}

impl Default for KeymapBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl KeymapBuilder {
    pub fn new() -> Self {
        let mut kb = Self {
            codes: BTreeMap::new(),
            chars: BTreeMap::new(),
            specials: BTreeMap::new(),
            next_code: CODE_MIN,
            dynamic_order: Vec::new(),
            dirty: true,
            rendered: String::new(),
        };
        for (keysym, code) in [
            (KEYSYM_BACKSPACE, KEY_BACKSPACE),
            (KEYSYM_RETURN, KEY_RETURN),
            (KEYSYM_TAB, KEY_TAB),
            (KEYSYM_ESCAPE, KEY_ESC),
        ] {
            kb.codes.insert(code, keysym);
            kb.specials.insert(keysym, code);
        }
        for c in static_chars() {
            if kb.alloc(c, true).is_none() {
                break;
            }
        }
        kb.rendered = kb.render();
        kb.dirty = false;
        kb
    }

    pub fn action_for_char(&self, c: char) -> Option<KeyAction> {
        self.chars.get(&c).map(|&keycode| KeyAction { keycode })
    }

    pub fn action_for_keysym(&self, keysym: u32) -> Option<KeyAction> {
        self.specials.get(&keysym).map(|&keycode| KeyAction { keycode })
    }

    /// Ensure every character of `text` has a keycode; false if any could not be placed.
    pub fn ensure_text(&mut self, text: &str) -> bool {
        let mut ok = true;
        for c in text.chars() {
            if !self.chars.contains_key(&c) && self.alloc(c, false).is_none() {
                ok = false;
            }
        }
        ok
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// The keymap text, re-rendered lazily after assignments.
    pub fn keymap_text(&mut self) -> &str {
        if self.dirty {
            self.rendered = self.render();
            self.dirty = false;
        }
        &self.rendered
    }

    fn usable(code: u16, codes: &BTreeMap<u16, u32>) -> bool {
        !codes.contains_key(&code) && !RESERVED.contains(&code)
    }

    /// Allocate a keycode for `c`; when the space is exhausted the least
    /// recently used dynamic character gives up its keycode.
    fn alloc(&mut self, c: char, static_slot: bool) -> Option<u16> {
        let mut code = None;
        let mut candidate = self.next_code;
        while candidate <= CODE_MAX {
            if Self::usable(candidate, &self.codes) {
                code = Some(candidate);
                break;
            }
            candidate += 1;
        }
        if code.is_none() {
            candidate = CODE_MIN;
            while candidate <= CODE_MAX {
                if Self::usable(candidate, &self.codes) {
                    code = Some(candidate);
                    break;
                }
                candidate += 1;
            }
        }
        let code = match code {
            Some(code) => code,
            None => {
                let oldest = *self.dynamic_order.first()?;
                self.dynamic_order.remove(0);
                let freed = self.chars.remove(&oldest)?;
                self.codes.remove(&freed);
                freed
            }
        };
        self.next_code = code.saturating_add(1);

        self.codes.insert(code, keysym_of(c));
        self.chars.insert(c, code);
        if !static_slot {
            self.dynamic_order.push(c);
        }
        self.dirty = true;
        Some(code)
    }

    fn render(&self) -> String {
        let mut s = String::with_capacity(8 * 1024);
        s.push_str("xkb_keymap {\nxkb_keycodes \"hw\" {\n  minimum = 8;\n  maximum = 256;\n");
        for code in self.codes.keys() {
            s.push_str(&format!("  <K{code}> = {code};\n"));
        }
        s.push_str("};\nxkb_types \"hw\" {\n");
        s.push_str("  type \"ONE\" { modifiers= none; map[None]= Level1; level_name[Level1]= \"Any\"; };\n");
        s.push_str("};\nxkb_symbols \"hw\" {\n");
        for (code, keysym) in &self.codes {
            s.push_str(&format!(
                "  key <K{code}> {{ type[Group1]=\"ONE\", [ 0x{keysym:08x} ] }};\n"
            ));
        }
        s.push_str("};\n};\n");
        s
    }
}

/// Create a memfd containing `text`, rewound to 0, ready to send over the socket.
pub fn keymap_memfd(text: &str) -> Result<std::fs::File, std::io::Error> {
    let name = std::ffi::CString::new("hw-keymap").unwrap();
    let fd = unsafe { libc::memfd_create(name.as_ptr(), libc::MFD_CLOEXEC) };
    if fd < 0 {
        return Err(std::io::Error::last_os_error());
    }
    let mut file: std::fs::File = unsafe { std::fs::File::from_raw_fd(fd) };
    file.write_all(text.as_bytes())?;
    file.seek(SeekFrom::Start(0))?;
    Ok(file)
}
