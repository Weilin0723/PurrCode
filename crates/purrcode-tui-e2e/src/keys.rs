//! Key sequences, encoded exactly as a terminal emulator would send them.
//!
//! The bytes here are what a real terminal puts on the wire, so a passing test
//! proves the workbench responds to what the user's keyboard actually produces —
//! not to a synthesized `KeyEvent` the test invented.

/// One key press.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Key {
    Char(char),
    Text(String),
    Enter,
    Tab,
    BackTab,
    Backspace,
    Escape,
    Up,
    Down,
    Left,
    Right,
    /// Up/Down with the control modifier, e.g. card selection in the
    /// conversation. Encoded as the standard xterm CSI modifier sequence
    /// (`ESC [ 1 ; 5 A`), which is what a real terminal sends and what
    /// crossterm parses on the other end.
    CtrlUp,
    CtrlDown,
    Home,
    End,
    PageUp,
    PageDown,
    Delete,
    /// Function key, 1-indexed.
    Function(u8),
    /// Control modifier applied to an ASCII letter, e.g. `Key::Ctrl('g')`.
    Ctrl(char),
    /// Content delivered as a bracketed paste, the way a terminal delivers a
    /// real paste. This is the only way to test that a multi-line paste does not
    /// submit early.
    Paste(String),
}

impl Key {
    /// The bytes a terminal emulator sends for this key.
    pub fn bytes(&self) -> Vec<u8> {
        match self {
            Self::Char(character) => character.to_string().into_bytes(),
            Self::Text(text) => text.clone().into_bytes(),
            Self::Enter => b"\r".to_vec(),
            Self::Tab => b"\t".to_vec(),
            Self::BackTab => b"\x1b[Z".to_vec(),
            Self::Backspace => b"\x7f".to_vec(),
            Self::Escape => b"\x1b".to_vec(),
            Self::Up => b"\x1b[A".to_vec(),
            Self::Down => b"\x1b[B".to_vec(),
            Self::Right => b"\x1b[C".to_vec(),
            Self::Left => b"\x1b[D".to_vec(),
            Self::CtrlUp => b"\x1b[1;5A".to_vec(),
            Self::CtrlDown => b"\x1b[1;5B".to_vec(),
            Self::Home => b"\x1b[H".to_vec(),
            Self::End => b"\x1b[F".to_vec(),
            Self::PageUp => b"\x1b[5~".to_vec(),
            Self::PageDown => b"\x1b[6~".to_vec(),
            Self::Delete => b"\x1b[3~".to_vec(),
            Self::Function(number) => match number {
                1 => b"\x1bOP".to_vec(),
                2 => b"\x1bOQ".to_vec(),
                3 => b"\x1bOR".to_vec(),
                4 => b"\x1bOS".to_vec(),
                5 => b"\x1b[15~".to_vec(),
                6 => b"\x1b[17~".to_vec(),
                7 => b"\x1b[18~".to_vec(),
                8 => b"\x1b[19~".to_vec(),
                9 => b"\x1b[20~".to_vec(),
                10 => b"\x1b[21~".to_vec(),
                11 => b"\x1b[23~".to_vec(),
                _ => b"\x1b[24~".to_vec(),
            },
            // Control codes are the letter's position in the alphabet: Ctrl+A is
            // 0x01, so Ctrl+G is 0x07.
            Self::Ctrl(letter) => {
                let lower = letter.to_ascii_lowercase();
                if lower.is_ascii_lowercase() {
                    vec![(lower as u8) - b'a' + 1]
                } else {
                    Vec::new()
                }
            }
            Self::Paste(content) => {
                let mut bytes = b"\x1b[200~".to_vec();
                bytes.extend_from_slice(content.as_bytes());
                bytes.extend_from_slice(b"\x1b[201~");
                bytes
            }
        }
    }

    /// Type a string as individual character presses.
    pub fn typed(text: &str) -> Vec<Self> {
        text.chars().map(Self::Char).collect()
    }
}

/// Bytes for a whole sequence of keys.
pub fn encode(keys: &[Key]) -> Vec<u8> {
    keys.iter().flat_map(|key| key.bytes()).collect()
}

/// A human-readable name for a key, used in failure artifacts.
pub fn describe(key: &Key) -> String {
    match key {
        Key::Char(character) => format!("{character:?}"),
        Key::Text(text) => format!("text({text:?})"),
        Key::Ctrl(letter) => format!("Ctrl+{}", letter.to_ascii_uppercase()),
        Key::Function(number) => format!("F{number}"),
        Key::Paste(content) => format!("paste({} bytes)", content.len()),
        other => format!("{other:?}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn control_codes_match_the_terminal_encoding() {
        assert_eq!(Key::Ctrl('a').bytes(), vec![0x01]);
        assert_eq!(Key::Ctrl('g').bytes(), vec![0x07]);
        assert_eq!(Key::Ctrl('G').bytes(), vec![0x07], "case is irrelevant");
        assert_eq!(Key::Ctrl('c').bytes(), vec![0x03]);
        assert_eq!(Key::Ctrl('p').bytes(), vec![0x10]);
    }

    #[test]
    fn a_non_letter_control_key_encodes_to_nothing_rather_than_a_wrong_byte() {
        assert!(Key::Ctrl('1').bytes().is_empty());
    }

    #[test]
    fn arrows_and_navigation_use_csi_sequences() {
        assert_eq!(Key::Up.bytes(), b"\x1b[A");
        assert_eq!(Key::Down.bytes(), b"\x1b[B");
        assert_eq!(Key::Right.bytes(), b"\x1b[C");
        assert_eq!(Key::Left.bytes(), b"\x1b[D");
        assert_eq!(Key::PageUp.bytes(), b"\x1b[5~");
        assert_eq!(Key::Delete.bytes(), b"\x1b[3~");
        assert_eq!(Key::BackTab.bytes(), b"\x1b[Z");
    }

    #[test]
    fn backspace_uses_delete_as_terminals_do() {
        assert_eq!(Key::Backspace.bytes(), vec![0x7f]);
    }

    #[test]
    fn function_keys_are_encoded() {
        assert_eq!(Key::Function(1).bytes(), b"\x1bOP");
        assert_eq!(Key::Function(5).bytes(), b"\x1b[15~");
    }

    #[test]
    fn a_paste_is_wrapped_in_bracketed_paste_markers() {
        let bytes = Key::Paste("line one\nline two".into()).bytes();
        let text = String::from_utf8(bytes).unwrap();
        assert!(text.starts_with("\x1b[200~"));
        assert!(text.ends_with("\x1b[201~"));
        assert!(
            text.contains("line one\nline two"),
            "a paste must carry its newlines so early submission is testable"
        );
    }

    #[test]
    fn typing_produces_one_key_per_character() {
        let keys = Key::typed("/help");
        assert_eq!(keys.len(), 5);
        assert_eq!(keys[0], Key::Char('/'));
        assert_eq!(encode(&keys), b"/help");
    }

    #[test]
    fn a_sequence_concatenates_in_order() {
        let bytes = encode(&[Key::Char('a'), Key::Enter, Key::Ctrl('g')]);
        assert_eq!(bytes, vec![b'a', b'\r', 0x07]);
    }

    #[test]
    fn multibyte_characters_survive_encoding() {
        assert_eq!(Key::Char('é').bytes(), "é".as_bytes());
        assert_eq!(Key::Text("日本語".into()).bytes(), "日本語".as_bytes());
    }

    #[test]
    fn every_key_has_a_readable_description_for_artifacts() {
        for key in [
            Key::Char('x'),
            Key::Ctrl('g'),
            Key::Enter,
            Key::Escape,
            Key::Function(5),
            Key::Paste("secret".into()),
        ] {
            assert!(!describe(&key).is_empty());
        }
        assert_eq!(describe(&Key::Ctrl('g')), "Ctrl+G");
        assert_eq!(
            describe(&Key::Paste("abc".into())),
            "paste(3 bytes)",
            "artifacts record the paste size, never its contents"
        );
    }
}
