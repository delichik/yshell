//! Key encoding from Slint key events into terminal byte sequences.
//!
//! Slint reports special keys as private-use control characters (see
//! `i-slint-common` `key_codes.rs`), so this module owns the translation to
//! the VT/CSI sequences a PTY expects.

const RETURN: &str = "\u{000a}";
const BACKSPACE: &str = "\u{0008}";
const TAB: &str = "\u{0009}";
const ESCAPE: &str = "\u{001b}";
const BACKTAB: &str = "\u{0019}";
const DELETE: &str = "\u{007f}";

const UP_ARROW: &str = "\u{F700}";
const DOWN_ARROW: &str = "\u{F701}";
const LEFT_ARROW: &str = "\u{F702}";
const RIGHT_ARROW: &str = "\u{F703}";
const F1: &str = "\u{F704}";
const F2: &str = "\u{F705}";
const F3: &str = "\u{F706}";
const F4: &str = "\u{F707}";
const F5: &str = "\u{F708}";
const F6: &str = "\u{F709}";
const F7: &str = "\u{F70A}";
const F8: &str = "\u{F70B}";
const F9: &str = "\u{F70C}";
const F10: &str = "\u{F70D}";
const F11: &str = "\u{F70E}";
const F12: &str = "\u{F70F}";
const INSERT: &str = "\u{F727}";
const HOME: &str = "\u{F729}";
const END: &str = "\u{F72B}";
const PAGE_UP: &str = "\u{F72C}";
const PAGE_DOWN: &str = "\u{F72D}";

const SHIFT: &str = "\u{0010}";
const CONTROL: &str = "\u{0011}";
const ALT: &str = "\u{0012}";
const ALT_GR: &str = "\u{0013}";
const CAPS_LOCK: &str = "\u{0014}";
const SHIFT_RIGHT: &str = "\u{0015}";
const CONTROL_RIGHT: &str = "\u{0016}";
const META: &str = "\u{0017}";
const META_RIGHT: &str = "\u{0018}";

/// Encode a Slint key event into the bytes a terminal expects.
///
/// Pure modifier keys and unmapped private-use key codes produce no input.
/// `meta` has no portable terminal encoding and is intentionally ignored.
#[must_use]
pub fn encode_key(text: &str, ctrl: bool, alt: bool, shift: bool, meta: bool) -> Vec<u8> {
    let _ = meta;
    let modifiers = xterm_modifier(ctrl, alt, shift);
    match text {
        SHIFT | CONTROL | ALT | ALT_GR | CAPS_LOCK | SHIFT_RIGHT | CONTROL_RIGHT | META
        | META_RIGHT => Vec::new(),
        RETURN => with_alt(b"\r", alt),
        BACKSPACE => with_alt(b"\x7f", alt),
        TAB => {
            if shift {
                with_alt(b"\x1b[Z", alt)
            } else {
                with_alt(b"\t", alt)
            }
        }
        BACKTAB => b"\x1b[Z".to_vec(),
        ESCAPE => b"\x1b".to_vec(),
        DELETE => tilde_sequence(3, modifiers),
        UP_ARROW => cursor_sequence('A', modifiers),
        DOWN_ARROW => cursor_sequence('B', modifiers),
        RIGHT_ARROW => cursor_sequence('C', modifiers),
        LEFT_ARROW => cursor_sequence('D', modifiers),
        HOME => cursor_sequence('H', modifiers),
        END => cursor_sequence('F', modifiers),
        INSERT => tilde_sequence(2, modifiers),
        PAGE_UP => tilde_sequence(5, modifiers),
        PAGE_DOWN => tilde_sequence(6, modifiers),
        F1 => function_sequence('P', 1, modifiers),
        F2 => function_sequence('Q', 2, modifiers),
        F3 => function_sequence('R', 3, modifiers),
        F4 => function_sequence('S', 4, modifiers),
        F5 => tilde_sequence(15, modifiers),
        F6 => tilde_sequence(17, modifiers),
        F7 => tilde_sequence(18, modifiers),
        F8 => tilde_sequence(19, modifiers),
        F9 => tilde_sequence(20, modifiers),
        F10 => tilde_sequence(21, modifiers),
        F11 => tilde_sequence(23, modifiers),
        F12 => tilde_sequence(24, modifiers),
        _ => encode_text(text, ctrl, alt),
    }
}

/// Encode text that is not one of the known special keys.
fn encode_text(text: &str, ctrl: bool, alt: bool) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(text.len() + usize::from(alt));
    for character in text.chars() {
        // Unmapped private-use keys (PrintScreen, media keys, ...) must not be
        // typed into the shell as UTF-8 text.
        if ('\u{e000}'..='\u{f8ff}').contains(&character) {
            continue;
        }
        if ctrl {
            if let Some(byte) = control_byte(character) {
                if alt {
                    bytes.push(0x1b);
                }
                bytes.push(byte);
                continue;
            }
        }
        if alt {
            bytes.push(0x1b);
        }
        let mut buffer = [0u8; 4];
        bytes.extend_from_slice(character.encode_utf8(&mut buffer).as_bytes());
    }
    bytes
}

/// The classic Ctrl+key to C0 control byte mapping (xterm compatible).
fn control_byte(character: char) -> Option<u8> {
    match character {
        'a'..='z' => Some(character as u8 - b'a' + 0x01),
        'A'..='Z' => Some(character as u8 - b'A' + 0x01),
        ' ' | '@' | '2' => Some(0x00),
        '[' | '3' => Some(0x1b),
        '\\' | '4' => Some(0x1c),
        ']' | '5' => Some(0x1d),
        '^' | '6' => Some(0x1e),
        '_' | '7' => Some(0x1f),
        '?' | '8' => Some(0x7f),
        _ => None,
    }
}

/// xterm modifier parameter: 1 for none, plus 1=shift, 2=alt, 4=ctrl.
const fn xterm_modifier(ctrl: bool, alt: bool, shift: bool) -> u8 {
    1 + (shift as u8) + (alt as u8) * 2 + (ctrl as u8) * 4
}

fn cursor_sequence(final_byte: char, modifiers: u8) -> Vec<u8> {
    if modifiers <= 1 {
        format!("\x1b[{final_byte}").into_bytes()
    } else {
        format!("\x1b[1;{modifiers}{final_byte}").into_bytes()
    }
}

fn tilde_sequence(number: u8, modifiers: u8) -> Vec<u8> {
    if modifiers <= 1 {
        format!("\x1b[{number}~").into_bytes()
    } else {
        format!("\x1b[{number};{modifiers}~").into_bytes()
    }
}

/// F1-F4 use SS3 without modifiers and CSI 1;{m} with modifiers.
fn function_sequence(final_byte: char, number: u8, modifiers: u8) -> Vec<u8> {
    if modifiers <= 1 {
        format!("\x1bO{final_byte}").into_bytes()
    } else {
        format!("\x1b[{number};{modifiers}{final_byte}").into_bytes()
    }
}

fn with_alt(bytes: &[u8], alt: bool) -> Vec<u8> {
    if alt {
        let mut prefixed = Vec::with_capacity(bytes.len() + 1);
        prefixed.push(0x1b);
        prefixed.extend_from_slice(bytes);
        prefixed
    } else {
        bytes.to_vec()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(text: &str) -> Vec<u8> {
        encode_key(text, false, false, false, false)
    }

    #[test]
    fn enter_backspace_tab_and_escape_map_to_control_bytes() {
        assert_eq!(key(RETURN), b"\r");
        assert_eq!(key(BACKSPACE), b"\x7f");
        assert_eq!(key(TAB), b"\t");
        assert_eq!(key(ESCAPE), b"\x1b");
    }

    #[test]
    fn backtab_and_delete_use_csi_sequences() {
        assert_eq!(key(BACKTAB), b"\x1b[Z");
        assert_eq!(key(DELETE), b"\x1b[3~");
        assert_eq!(encode_key(TAB, false, false, true, false), b"\x1b[Z");
    }

    #[test]
    fn arrow_keys_use_csi_sequences_with_modifiers() {
        assert_eq!(key(UP_ARROW), b"\x1b[A");
        assert_eq!(key(DOWN_ARROW), b"\x1b[B");
        assert_eq!(key(RIGHT_ARROW), b"\x1b[C");
        assert_eq!(key(LEFT_ARROW), b"\x1b[D");
        assert_eq!(
            encode_key(UP_ARROW, true, false, false, false),
            b"\x1b[1;5A"
        );
        assert_eq!(
            encode_key(LEFT_ARROW, false, true, true, false),
            b"\x1b[1;4D"
        );
        assert_eq!(
            encode_key(RIGHT_ARROW, true, true, true, false),
            b"\x1b[1;8C"
        );
    }

    #[test]
    fn home_end_page_and_insert_keys_use_csi_sequences() {
        assert_eq!(key(HOME), b"\x1b[H");
        assert_eq!(key(END), b"\x1b[F");
        assert_eq!(key(PAGE_UP), b"\x1b[5~");
        assert_eq!(key(PAGE_DOWN), b"\x1b[6~");
        assert_eq!(key(INSERT), b"\x1b[2~");
        assert_eq!(encode_key(PAGE_UP, true, false, false, false), b"\x1b[5;5~");
        assert_eq!(encode_key(HOME, false, false, true, false), b"\x1b[1;2H");
    }

    #[test]
    fn function_keys_use_vt_sequences() {
        assert_eq!(key(F1), b"\x1bOP");
        assert_eq!(key(F4), b"\x1bOS");
        assert_eq!(key(F5), b"\x1b[15~");
        assert_eq!(key(F6), b"\x1b[17~");
        assert_eq!(key(F12), b"\x1b[24~");
        assert_eq!(encode_key(F1, true, false, false, false), b"\x1b[1;5P");
        assert_eq!(encode_key(F5, false, true, false, false), b"\x1b[15;3~");
    }

    #[test]
    fn ctrl_letters_map_to_control_bytes() {
        assert_eq!(encode_key("a", true, false, false, false), vec![0x01]);
        assert_eq!(encode_key("c", true, false, false, false), vec![0x03]);
        assert_eq!(encode_key("z", true, false, false, false), vec![0x1a]);
        assert_eq!(encode_key("C", true, false, true, false), vec![0x03]);
    }

    #[test]
    fn ctrl_symbols_and_space_map_to_control_bytes() {
        assert_eq!(encode_key(" ", true, false, false, false), vec![0x00]);
        assert_eq!(encode_key("@", true, false, false, false), vec![0x00]);
        assert_eq!(encode_key("[", true, false, false, false), vec![0x1b]);
        assert_eq!(encode_key("\\", true, false, false, false), vec![0x1c]);
        assert_eq!(encode_key("]", true, false, false, false), vec![0x1d]);
        assert_eq!(encode_key("^", true, false, false, false), vec![0x1e]);
        assert_eq!(encode_key("_", true, false, false, false), vec![0x1f]);
        assert_eq!(encode_key("?", true, false, false, false), vec![0x7f]);
        assert_eq!(encode_key("2", true, false, false, false), vec![0x00]);
        assert_eq!(encode_key("3", true, false, false, false), vec![0x1b]);
    }

    #[test]
    fn alt_prefixes_escape_for_text_and_control_keys() {
        assert_eq!(encode_key("a", false, true, false, false), b"\x1ba");
        assert_eq!(encode_key("a", true, true, false, false), b"\x1b\x01");
        assert_eq!(encode_key(RETURN, false, true, false, false), b"\x1b\r");
        assert_eq!(
            encode_key(BACKSPACE, false, true, false, false),
            b"\x1b\x7f"
        );
    }

    #[test]
    fn utf8_and_control_char_text_passes_through() {
        assert_eq!(key("hi"), b"hi");
        assert_eq!(key("你好"), "你好".as_bytes());
        assert_eq!(key("\u{0003}"), vec![0x03]);
        assert_eq!(
            encode_key("\u{0003}", true, false, false, false),
            vec![0x03]
        );
    }

    #[test]
    fn meta_modifier_does_not_change_encoding() {
        assert_eq!(
            encode_key("a", false, false, false, true),
            encode_key("a", false, false, false, false)
        );
    }

    #[test]
    fn pure_modifier_keys_produce_no_input() {
        for text in [
            SHIFT,
            CONTROL,
            ALT,
            ALT_GR,
            CAPS_LOCK,
            SHIFT_RIGHT,
            CONTROL_RIGHT,
            META,
            META_RIGHT,
        ] {
            assert_eq!(
                key(text),
                Vec::<u8>::new(),
                "{text:?} must not reach the pty"
            );
        }
        assert_eq!(
            encode_key(CONTROL, true, false, false, false),
            Vec::<u8>::new()
        );
    }

    #[test]
    fn unmapped_private_use_keys_produce_no_input() {
        assert_eq!(key("\u{F72E}"), Vec::<u8>::new());
        assert_eq!(key("\u{F731}"), Vec::<u8>::new());
    }
}
