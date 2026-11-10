//! Self-contained XKB keymap generation for `zwp_virtual_keyboard_v1`.
//!
//! The virtual keyboard carries its own keymap; the compositor derives keysyms
//! (hence Unicode text) from *this* keymap when routing injected keycodes to the
//! focused app. The keymap is fully generated text — no `include`s, because XKB
//! data files cannot be assumed to exist on store paths.

use std::collections::BTreeMap;

pub const KEYSYM_BACKSPACE: u32 = 0xff08;
pub const KEYSYM_RETURN: u32 = 0xff0d;
pub const KEYSYM_TAB: u32 = 0xff09;
pub const KEYSYM_ESCAPE: u32 = 0xff1b;
pub const KEYSYM_SPACE: u32 = 0x20;

const KEY_BACKSPACE: u16 = 14;
const KEY_TAB: u16 = 15;
const KEY_RETURN: u16 = 28;
const KEY_ESC: u16 = 1;
const KEY_SHIFT_L: u16 = 42;
const KEY_CTRL_L: u16 = 29;

const EXT_STATIC_START: u16 = 150;
const EXT_DYNAMIC_START: u16 = 180;
const CODE_MAX: u16 = 255;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyAction {
    pub keycode: u16,
    pub shift: bool,
}

/// XKB keysym for a Unicode char (convention: 0x0100_0000 + codepoint).
pub fn keysym_of(c: char) -> u32 {
    let cp = c as u32;
    if cp < 0x100 {
        cp
    } else {
        0x0100_0000 + cp
    }
}

/// Direct (static) extended characters — covers Polish/German/French/Spanish/Nordic/
/// Turkish/Romanian/punctuation/CJK bonus. Keycodes 150..179 are assigned directly.
fn extended_static() -> Vec<u32> {
    let mut v: Vec<u32> = Vec::new();
    // Polish
    v.extend("ąćęłńóśźżĄĆĘŁŃÓŚŹŻ".chars().map(|c| c as u32));
    // German
    v.extend("äöüÄÖÜß€".chars().map(|c| c as u32));
    // French
    v.extend("àâçéèêëîïôùûüÿœŒÀÂÇÉÈÊËÎÏÔÙÛÜ".chars().map(|c| c as u32));
    // Spanish
    v.extend("ñ¿¡".chars().map(|c| c as u32));
    // Nordic/Dutch/Portuguese
    v.extend("åæøÅÆØãõçÃÕ".chars().map(|c| c as u32));
    // Czech/Slovak/Slovene/Croatian
    v.extend("řžěščůďťňŘŽĚŠČŮĎŤŇđĐ".chars().map(|c| c as u32));
    // Turkish
    v.extend("ğışĞİŞ".chars().map(|c| c as u32));
    // Romanian
    v.extend("ștàîȘȚĂÎ".chars().map(|c| c as u32));
    // Common punctuation / symbols
    v.extend("—–…‘’“”«»°·±×÷½¼¾£¥".chars().map(|c| c as u32));
    v.dedup();
    v
}

/// ASCII table: keycode -> (base keysym, shifted keysym).
fn ascii_table() -> Vec<(u16, u32, u32)> {
    let mut t: Vec<(u16, u32, u32)> = Vec::new();
    // Digits & common punct
    let digit_pairs: [(u16, (char, char)); 10] = [
        (2, ('1', '!')), (3, ('2', '@')), (4, ('3', '#')), (5, ('4', '$')),
        (6, ('5', '%')), (7, ('6', '^')), (8, ('7', '&')), (9, ('8', '*')),
        (10, ('9', '(')), (11, ('0', ')')),
    ];
    for (code, (a, b)) in digit_pairs {
        t.push((code, a as u32, b as u32));
    }
    let punct: [(u16, (char, char)); 9] = [
        (12, ('-', '_')), (13, ('=', '+')), (26, ('[', '{')), (27, (']', '}')),
        (39, (';', ':')), (40, ('\'', '"')), (41, ('`', '~')), (43, ('\\', '|')),
        (51, (',', '<')), (52, ('.', '>')), (53, ('/', '?')),
    ];
    for (code, (a, b)) in punct {
        t.push((code, a as u32, b as u32));
    }
    // Shifted lower-row letters
    let lower_row: [(u16, char); 20] = [
        (16, 'q'), (17, 'w'), (18, 'e'), (19, 'r'), (20, 't'), (21, 'y'), (22, 'u'),
        (23, 'i'), (24, 'o'), (25, 'p'), (30, 'a'), (31, 's'), (32, 'd'), (33, 'f'),
        (34, 'g'), (35, 'h'), (36, 'j'), (37, 'k'), (38, 'l'), (44, 'z'),
    ];
    for (code, c) in lower_row {
        let up = c.to_ascii_uppercase() as u32;
        t.push((code, c as u32, up));
    }
    let row2: [(u16, char); 7] = [(45, 'x'), (46, 'c'), (47, 'v'), (48, 'b'), (49, 'n'), (50, 'm')];
    for (code, c) in row2 {
        let up = c.to_ascii_uppercase() as u32;
        t.push((code, c as u32, up));
    }
    // Special keys — always carry their keysym (not a character).
    t.push((KEY_BACKSPACE, KEYSYM_BACKSPACE, KEYSYM_BACKSPACE));
    t.push((KEY_RETURN, KEYSYM_RETURN, KEYSYM_RETURN));
    t.push((KEY_TAB, KEYSYM_TAB, KEYSYM_TAB));
    t.push((KEY_ESC, KEYSYM_ESCAPE, KEYSYM_ESCAPE));
    t
}

/// Builders for the keymap state.
pub struct KeymapBuilder {
    codes: BTreeMap<u16, (u32, u32)>,
    chars: BTreeMap<char, KeyAction>,
    specials: BTreeMap<u32, KeyAction>,
    next_dynamic: u16,
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
            next_dynamic: EXT_DYNAMIC_START,
            dynamic_order: Vec::new(),
            dirty: true,
            rendered: String::new(),
        };
        for (code, base, shift) in ascii_table() {
            kb.codes.insert(code, (base, shift));
            let base_c = char::from_u32(base);
            let shift_c = char::from_u32(shift);
            if let Some(c) = base_c {
                kb.chars.insert(c, KeyAction { keycode: code, shift: false });
            }
            if let Some(c) = shift_c {
                if Some(c) != base_c {
                    kb.chars.insert(c, KeyAction { keycode: code, shift: true });
                }
            }
        }
        for (i, ks) in extended_static().into_iter().enumerate() {
            let code = EXT_STATIC_START + i as u16;
            if code >= EXT_DYNAMIC_START {
                break;
            }
            kb.codes.insert(code, (ks, ks));
            if let Some(c) = char::from_u32(ks) {
                kb.chars.insert(c, KeyAction { keycode: code, shift: false });
            }
        }
        // Special keys
        for (ks, code) in [
            (KEYSYM_BACKSPACE, KEY_BACKSPACE),
            (KEYSYM_RETURN, KEY_RETURN),
            (KEYSYM_TAB, KEY_TAB),
            (KEYSYM_ESCAPE, KEY_ESC),
        ] {
            kb.specials
                .insert(ks, KeyAction { keycode: code, shift: false });
        }
        kb.rendered = kb.render();
        kb.dirty = false;
        kb
    }

    pub fn action_for_char(&self, c: char) -> Option<KeyAction> {
        self.chars.get(&c).copied()
    }

    pub fn action_for_keysym(&self, ks: u32) -> Option<KeyAction> {
        self.specials.get(&ks).copied()
    }

    /// Assign a slot for a char, evicting an older dynamic char if needed.
    fn assign(&mut self, c: char) -> Option<KeyAction> {
        let code = if self.next_dynamic <= CODE_MAX {
            let code = self.next_dynamic;
            self.next_dynamic += 1;
            code
        } else if let Some(old) = self.dynamic_order.first().copied() {
            self.dynamic_order.remove(0);
            let old_code = self.chars.remove(&old)?.keycode;
            old_code
        } else {
            return None;
        };
        let ks = keysym_of(c);
        self.codes.insert(code, (ks, ks));
        let act = KeyAction { keycode: code, shift: false };
        self.chars.insert(c, act);
        self.dynamic_order.push(c);
        self.dirty = true;
        Some(act)
    }

    pub fn ensure_text(&mut self, text: &str) -> bool {
        let mut ok = true;
        for c in text.chars() {
            if !self.chars.contains_key(&c) && self.assign(c).is_none() {
                ok = false;
            }
        }
        ok
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    pub fn keymap_text(&mut self) -> &str {
        if self.dirty {
            self.rendered = self.render();
            self.dirty = false;
        }
        &self.rendered
    }

    fn render(&self) -> String {
        let mut s = String::with_capacity(8 * 1024);
        s.push_str("xkb_keymap {\nxkb_keycodes \"hw\" {\n  minimum = 8;\n  maximum = 256;\n");
        for (code, _) in &self.codes {
            s.push_str(&format!("  <K{code}> = {code};\n"));
        }
        s.push_str("  indicator 1 = \"Caps Lock\";\n};\nxkb_types \"hw\" {\n");
        s.push_str("  type \"ONE\" { modifiers= none; map[None]= Level1; level_name[Level1]= \"Any\"; };\n");
        s.push_str("  type \"TWO\" { modifiers= Shift; map[Shift]= Level2; level_name[Level1]= \"Base\"; level_name[Level2]= \"Shift\"; };\n");
        s.push_str("};\nxkb_symbols \"hw\" {\n");
        for (code, (base, shift)) in &self.codes {
            if base == shift {
                s.push_str(&format!("  key <K{code}> {{ [ 0x{base:08x} ] }};\n"));
            } else {
                s.push_str(&format!(
                    "  key <K{code}> {{ type[Group1]=\"TWO\", [ 0x{base:08x}, 0x{shift:08x} ] }};\n"
                ));
            }
        }
        s.push_str("};\n};\n");
        s
    }
}

/// Create a memfd containing `text`, rewound to 0.
pub fn keymap_memfd(text: &str) -> Result<std::fs::File, std::io::Error> {
    let name = std::ffi::CString::new("keymap").unwrap();
    let fd = unsafe { libc::memfd_create(name.as_ptr(), libc::MFD_CLOEXEC) };
    if fd < 0 {
        return Err(std::io::Error::last_os_error());
    }
    let file: std::fs::File = unsafe { std::fs::File::from_raw_fd(fd) };
    use std::io::Seek;
    file.write_all(text.as_bytes())?;
    file.seek(std::io::SeekFrom::Start(0))?;
    Ok(file)
}