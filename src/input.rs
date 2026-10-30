use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::config::KeyBindings;

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Forward(Vec<u8>),
    NewWindow,
    NextWindow,
    PrevWindow,
    SelectWindow(usize),
    SplitHorizontal,
    SplitVertical,
    FocusPane(crate::layout::PaneDirection),
    KillPane,
    ConfirmKillPane,
    CancelKillPane,
    Quit,
    ToggleSidebar,
    ExpandNextEdit,
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Normal,
    AwaitingPrefix,
    AwaitingKillConfirm,
}

pub struct InputHandler {
    state: State,
    bindings: KeyBindings,
}

impl InputHandler {
    pub fn new() -> InputHandler {
        Self::with_config(KeyBindings::default())
    }

    pub fn with_config(bindings: KeyBindings) -> InputHandler {
        InputHandler {
            state: State::Normal,
            bindings,
        }
    }

    fn is_prefix(&self, key: KeyEvent) -> bool {
        key.code == self.bindings.prefix.code && key.modifiers.contains(self.bindings.prefix.modifiers)
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> Action {
        match self.state {
            State::Normal => {
                if self.is_prefix(key) {
                    self.state = State::AwaitingPrefix;
                    Action::None
                } else {
                    Action::Forward(key_to_bytes(key))
                }
            }
            State::AwaitingPrefix => {
                self.state = State::Normal;
                if self.is_prefix(key) {
                    return Action::Forward(key_to_bytes(key));
                }
                match key.code {
                    KeyCode::Char(c) if c.is_ascii_digit() => {
                        Action::SelectWindow(c.to_digit(10).unwrap() as usize)
                    }
                    c if c == self.bindings.new_window => Action::NewWindow,
                    c if c == self.bindings.next_window => Action::NextWindow,
                    c if c == self.bindings.prev_window => Action::PrevWindow,
                    c if c == self.bindings.split_horizontal => Action::SplitHorizontal,
                    c if c == self.bindings.split_vertical => Action::SplitVertical,
                    c if c == self.bindings.focus_up => Action::FocusPane(crate::layout::PaneDirection::Up),
                    c if c == self.bindings.focus_down => Action::FocusPane(crate::layout::PaneDirection::Down),
                    c if c == self.bindings.focus_left => Action::FocusPane(crate::layout::PaneDirection::Left),
                    c if c == self.bindings.focus_right => Action::FocusPane(crate::layout::PaneDirection::Right),
                    c if c == self.bindings.kill_pane => {
                        self.state = State::AwaitingKillConfirm;
                        Action::KillPane
                    }
                    c if c == self.bindings.quit => Action::Quit,
                    c if c == self.bindings.toggle_sidebar => Action::ToggleSidebar,
                    c if c == self.bindings.expand_edit => Action::ExpandNextEdit,
                    _ => Action::None,
                }
            }
            State::AwaitingKillConfirm => {
                self.state = State::Normal;
                match key.code {
                    KeyCode::Char('y') | KeyCode::Char('Y') => Action::ConfirmKillPane,
                    _ => Action::CancelKillPane,
                }
            }
        }
    }
}

impl Default for InputHandler {
    fn default() -> InputHandler {
        InputHandler::new()
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

    fn ctrl_b() -> KeyEvent {
        KeyEvent::new(KeyCode::Char('b'), KeyModifiers::CONTROL)
    }

    fn plain(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
    }

    #[test]
    fn prefix_then_c_creates_new_window() {
        let mut handler = InputHandler::new();
        assert_eq!(handler.handle_key(ctrl_b()), Action::None);
        assert_eq!(handler.handle_key(plain('c')), Action::NewWindow);
    }

    #[test]
    fn prefix_then_n_selects_next_window() {
        let mut handler = InputHandler::new();
        assert_eq!(handler.handle_key(ctrl_b()), Action::None);
        assert_eq!(handler.handle_key(plain('n')), Action::NextWindow);
    }

    #[test]
    fn prefix_then_p_selects_prev_window() {
        let mut handler = InputHandler::new();
        assert_eq!(handler.handle_key(ctrl_b()), Action::None);
        assert_eq!(handler.handle_key(plain('p')), Action::PrevWindow);
    }

    #[test]
    fn prefix_then_digit_selects_window_by_index() {
        for digit in 0..=9u32 {
            let c = char::from_digit(digit, 10).unwrap();
            let mut handler = InputHandler::new();
            assert_eq!(handler.handle_key(ctrl_b()), Action::None);
            assert_eq!(
                handler.handle_key(plain(c)),
                Action::SelectWindow(digit as usize)
            );
        }
    }

    #[test]
    fn prefix_then_percent_splits_horizontal() {
        let mut handler = InputHandler::new();
        assert_eq!(handler.handle_key(ctrl_b()), Action::None);
        assert_eq!(handler.handle_key(plain('%')), Action::SplitHorizontal);
    }

    #[test]
    fn prefix_then_quote_splits_vertical() {
        let mut handler = InputHandler::new();
        assert_eq!(handler.handle_key(ctrl_b()), Action::None);
        assert_eq!(handler.handle_key(plain('"')), Action::SplitVertical);
    }

    #[test]
    fn prefix_then_up_focuses_pane_up() {
        let mut handler = InputHandler::new();
        assert_eq!(handler.handle_key(ctrl_b()), Action::None);
        assert_eq!(
            handler.handle_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE)),
            Action::FocusPane(crate::layout::PaneDirection::Up)
        );
    }

    #[test]
    fn prefix_then_down_focuses_pane_down() {
        let mut handler = InputHandler::new();
        assert_eq!(handler.handle_key(ctrl_b()), Action::None);
        assert_eq!(
            handler.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE)),
            Action::FocusPane(crate::layout::PaneDirection::Down)
        );
    }

    #[test]
    fn prefix_then_left_focuses_pane_left() {
        let mut handler = InputHandler::new();
        assert_eq!(handler.handle_key(ctrl_b()), Action::None);
        assert_eq!(
            handler.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE)),
            Action::FocusPane(crate::layout::PaneDirection::Left)
        );
    }

    #[test]
    fn prefix_then_right_focuses_pane_right() {
        let mut handler = InputHandler::new();
        assert_eq!(handler.handle_key(ctrl_b()), Action::None);
        assert_eq!(
            handler.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE)),
            Action::FocusPane(crate::layout::PaneDirection::Right)
        );
    }

    #[test]
    fn prefix_then_x_kills_pane() {
        let mut handler = InputHandler::new();
        assert_eq!(handler.handle_key(ctrl_b()), Action::None);
        assert_eq!(handler.handle_key(plain('x')), Action::KillPane);
    }

    #[test]
    fn prefix_then_d_quits() {
        let mut handler = InputHandler::new();
        assert_eq!(handler.handle_key(ctrl_b()), Action::None);
        assert_eq!(handler.handle_key(plain('d')), Action::Quit);
    }

    #[test]
    fn prefix_then_s_toggles_sidebar() {
        let mut handler = InputHandler::new();
        assert_eq!(handler.handle_key(ctrl_b()), Action::None);
        assert_eq!(handler.handle_key(plain('s')), Action::ToggleSidebar);
    }

    #[test]
    fn prefix_then_e_expands_next_edit() {
        let mut handler = InputHandler::new();
        assert_eq!(handler.handle_key(ctrl_b()), Action::None);
        assert_eq!(handler.handle_key(plain('e')), Action::ExpandNextEdit);
    }

    #[test]
    fn unbound_key_after_prefix_resets_to_normal() {
        let mut handler = InputHandler::new();
        assert_eq!(handler.handle_key(ctrl_b()), Action::None);
        assert_eq!(handler.handle_key(plain('z')), Action::None);

        let key = plain('a');
        assert_eq!(
            handler.handle_key(key),
            Action::Forward(key_to_bytes(key))
        );
    }

    #[test]
    fn prefix_then_prefix_sends_literal_ctrl_b() {
        let mut handler = InputHandler::new();
        assert_eq!(handler.handle_key(ctrl_b()), Action::None);
        assert_eq!(handler.handle_key(ctrl_b()), Action::Forward(vec![0x02]));
    }

    #[test]
    fn plain_key_with_no_prefix_forwards_unchanged() {
        let mut handler = InputHandler::new();
        let key = plain('a');
        assert_eq!(
            handler.handle_key(key),
            Action::Forward(key_to_bytes(key))
        );
    }

    #[test]
    fn kill_confirm_y_confirms_kill_pane() {
        let mut handler = InputHandler::new();
        assert_eq!(handler.handle_key(ctrl_b()), Action::None);
        assert_eq!(handler.handle_key(plain('x')), Action::KillPane);
        assert_eq!(handler.handle_key(plain('y')), Action::ConfirmKillPane);

        let key = plain('a');
        assert_eq!(
            handler.handle_key(key),
            Action::Forward(key_to_bytes(key))
        );
    }

    #[test]
    fn kill_confirm_other_key_cancels_kill_pane() {
        let mut handler = InputHandler::new();
        assert_eq!(handler.handle_key(ctrl_b()), Action::None);
        assert_eq!(handler.handle_key(plain('x')), Action::KillPane);
        assert_eq!(handler.handle_key(plain('n')), Action::CancelKillPane);

        let key = plain('a');
        assert_eq!(
            handler.handle_key(key),
            Action::Forward(key_to_bytes(key))
        );
    }

    fn custom_bindings() -> KeyBindings {
        KeyBindings {
            prefix: crate::config::KeySpec {
                code: KeyCode::Char('a'),
                modifiers: KeyModifiers::CONTROL,
            },
            new_window: KeyCode::Char('z'),
            ..KeyBindings::default()
        }
    }

    fn ctrl_a() -> KeyEvent {
        KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL)
    }

    #[test]
    fn custom_prefix_replaces_default_ctrl_b() {
        let mut handler = InputHandler::with_config(custom_bindings());
        assert_eq!(
            handler.handle_key(ctrl_b()),
            Action::Forward(key_to_bytes(ctrl_b()))
        );
        assert_eq!(handler.handle_key(ctrl_a()), Action::None);
        assert_eq!(handler.handle_key(plain('z')), Action::NewWindow);
    }

    #[test]
    fn custom_prefix_repeat_forwards_configured_control_byte() {
        let mut handler = InputHandler::with_config(custom_bindings());
        assert_eq!(handler.handle_key(ctrl_a()), Action::None);
        assert_eq!(handler.handle_key(ctrl_a()), Action::Forward(vec![0x01]));
    }
}
