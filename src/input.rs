use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

pub fn key_to_bytes(key: KeyEvent) -> Vec<u8> {
    match key.code {
        KeyCode::Char(c) if key.modifiers.contains(KeyModifiers::CONTROL) && c.is_ascii_alphabetic() => {
            vec![c.to_ascii_lowercase() as u8 - b'a' + 1]
        }
        KeyCode::Char(c) => c.to_string().into_bytes(),
        KeyCode::Enter => vec![b'\r'],
        KeyCode::Backspace => vec![0x7f],
        KeyCode::Tab => vec![0x09],
        KeyCode::Up => vec![0x1b, b'[', b'A'],
        KeyCode::Down => vec![0x1b, b'[', b'B'],
        KeyCode::Right => vec![0x1b, b'[', b'C'],
        KeyCode::Left => vec![0x1b, b'[', b'D'],
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn printable_characters_pass_through_unchanged() {
        assert_eq!(
            key_to_bytes(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE)),
            b"a".to_vec()
        );
        assert_eq!(
            key_to_bytes(KeyEvent::new(KeyCode::Char('5'), KeyModifiers::NONE)),
            b"5".to_vec()
        );
        assert_eq!(
            key_to_bytes(KeyEvent::new(KeyCode::Char('!'), KeyModifiers::NONE)),
            b"!".to_vec()
        );
    }

    #[test]
    fn enter_backspace_and_tab_map_correctly() {
        assert_eq!(
            key_to_bytes(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            vec![0x0d]
        );
        assert_eq!(
            key_to_bytes(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE)),
            vec![0x7f]
        );
        assert_eq!(
            key_to_bytes(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
            vec![0x09]
        );
    }

    #[test]
    fn arrow_keys_map_to_ansi_sequences() {
        assert_eq!(
            key_to_bytes(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE)),
            vec![0x1b, b'[', b'A']
        );
        assert_eq!(
            key_to_bytes(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE)),
            vec![0x1b, b'[', b'B']
        );
        assert_eq!(
            key_to_bytes(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE)),
            vec![0x1b, b'[', b'C']
        );
        assert_eq!(
            key_to_bytes(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE)),
            vec![0x1b, b'[', b'D']
        );
    }

    #[test]
    fn ctrl_letter_maps_to_control_byte() {
        assert_eq!(
            key_to_bytes(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL)),
            vec![0x01]
        );
        assert_eq!(
            key_to_bytes(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
            vec![0x03]
        );
        assert_eq!(
            key_to_bytes(KeyEvent::new(KeyCode::Char('z'), KeyModifiers::CONTROL)),
            vec![0x1a]
        );
    }
}
