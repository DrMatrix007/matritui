use std::str::FromStr;

use crossterm::event::{KeyCode, KeyModifiers};
use ratatui::style::Color;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeySpec {
    pub code: KeyCode,
    pub modifiers: KeyModifiers,
}

pub fn parse_key(s: &str) -> Option<KeySpec> {
    if let Some(letter) = s.strip_prefix("C-") {
        let mut chars = letter.chars();
        let c = chars.next()?;
        if chars.next().is_some() {
            return None;
        }
        return Some(KeySpec {
            code: KeyCode::Char(c),
            modifiers: KeyModifiers::CONTROL,
        });
    }

    match s {
        "Up" => Some(KeySpec { code: KeyCode::Up, modifiers: KeyModifiers::NONE }),
        "Down" => Some(KeySpec { code: KeyCode::Down, modifiers: KeyModifiers::NONE }),
        "Left" => Some(KeySpec { code: KeyCode::Left, modifiers: KeyModifiers::NONE }),
        "Right" => Some(KeySpec { code: KeyCode::Right, modifiers: KeyModifiers::NONE }),
        _ => {
            let mut chars = s.chars();
            let c = chars.next()?;
            if chars.next().is_some() {
                return None;
            }
            Some(KeySpec { code: KeyCode::Char(c), modifiers: KeyModifiers::NONE })
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct KeyBindings {
    pub prefix: KeySpec,
    pub new_window: KeyCode,
    pub next_window: KeyCode,
    pub prev_window: KeyCode,
    pub split_horizontal: KeyCode,
    pub split_vertical: KeyCode,
    pub focus_up: KeyCode,
    pub focus_down: KeyCode,
    pub focus_left: KeyCode,
    pub focus_right: KeyCode,
    pub kill_pane: KeyCode,
    pub quit: KeyCode,
}

impl Default for KeyBindings {
    fn default() -> KeyBindings {
        KeyBindings {
            prefix: KeySpec { code: KeyCode::Char('b'), modifiers: KeyModifiers::CONTROL },
            new_window: KeyCode::Char('c'),
            next_window: KeyCode::Char('n'),
            prev_window: KeyCode::Char('p'),
            split_horizontal: KeyCode::Char('%'),
            split_vertical: KeyCode::Char('"'),
            focus_up: KeyCode::Up,
            focus_down: KeyCode::Down,
            focus_left: KeyCode::Left,
            focus_right: KeyCode::Right,
            kill_pane: KeyCode::Char('x'),
            quit: KeyCode::Char('d'),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Colors {
    pub focused_border: Color,
    pub unfocused_border: Color,
    pub status_fg: Option<Color>,
    pub status_bg: Option<Color>,
}

impl Default for Colors {
    fn default() -> Colors {
        Colors {
            focused_border: Color::Yellow,
            unfocused_border: Color::DarkGray,
            status_fg: None,
            status_bg: None,
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct Config {
    pub keybindings: KeyBindings,
    pub colors: Colors,
}

#[derive(serde::Deserialize, Default)]
struct RawConfig {
    prefix: Option<String>,
    keybindings: Option<RawKeyBindings>,
    colors: Option<RawColors>,
}

#[derive(serde::Deserialize, Default)]
struct RawKeyBindings {
    new_window: Option<String>,
    next_window: Option<String>,
    prev_window: Option<String>,
    split_horizontal: Option<String>,
    split_vertical: Option<String>,
    focus_up: Option<String>,
    focus_down: Option<String>,
    focus_left: Option<String>,
    focus_right: Option<String>,
    kill_pane: Option<String>,
    quit: Option<String>,
}

#[derive(serde::Deserialize, Default)]
struct RawColors {
    focused_border: Option<String>,
    unfocused_border: Option<String>,
    status_fg: Option<String>,
    status_bg: Option<String>,
}

fn warn_bad_key(field: &str, value: &str) {
    eprintln!("mmux: warning: invalid key value for {field}: {value}; using default");
}

fn warn_bad_color(field: &str, value: &str) {
    eprintln!("mmux: warning: invalid color value for {field}: {value}; using default");
}

impl Config {
    pub fn from_toml_str(s: &str) -> Config {
        let raw: RawConfig = match toml::from_str(s) {
            Ok(raw) => raw,
            Err(err) => {
                eprintln!("mmux: warning: failed to parse config file: {err}; using defaults");
                return Config::default();
            }
        };

        let mut config = Config::default();

        if let Some(prefix) = &raw.prefix {
            match parse_key(prefix) {
                Some(spec) => config.keybindings.prefix = spec,
                None => warn_bad_key("prefix", prefix),
            }
        }

        if let Some(raw_bindings) = &raw.keybindings {
            macro_rules! apply_key {
                ($field:ident) => {
                    if let Some(value) = &raw_bindings.$field {
                        match parse_key(value) {
                            Some(spec) => config.keybindings.$field = spec.code,
                            None => warn_bad_key(stringify!($field), value),
                        }
                    }
                };
            }
            apply_key!(new_window);
            apply_key!(next_window);
            apply_key!(prev_window);
            apply_key!(split_horizontal);
            apply_key!(split_vertical);
            apply_key!(focus_up);
            apply_key!(focus_down);
            apply_key!(focus_left);
            apply_key!(focus_right);
            apply_key!(kill_pane);
            apply_key!(quit);
        }

        if let Some(raw_colors) = &raw.colors {
            if let Some(value) = &raw_colors.focused_border {
                match Color::from_str(value) {
                    Ok(color) => config.colors.focused_border = color,
                    Err(_) => warn_bad_color("focused_border", value),
                }
            }
            if let Some(value) = &raw_colors.unfocused_border {
                match Color::from_str(value) {
                    Ok(color) => config.colors.unfocused_border = color,
                    Err(_) => warn_bad_color("unfocused_border", value),
                }
            }
            if let Some(value) = &raw_colors.status_fg {
                match Color::from_str(value) {
                    Ok(color) => config.colors.status_fg = Some(color),
                    Err(_) => warn_bad_color("status_fg", value),
                }
            }
            if let Some(value) = &raw_colors.status_bg {
                match Color::from_str(value) {
                    Ok(color) => config.colors.status_bg = Some(color),
                    Err(_) => warn_bad_color("status_bg", value),
                }
            }
        }

        config
    }

    pub fn load() -> Config {
        let Some(dirs) = directories::ProjectDirs::from("", "", "mmux") else {
            return Config::default();
        };
        let path = dirs.config_dir().join("config.toml");
        if !path.exists() {
            return Config::default();
        }
        match std::fs::read_to_string(&path) {
            Ok(contents) => Config::from_toml_str(&contents),
            Err(err) => {
                eprintln!("mmux: warning: failed to read config file {}: {err}; using defaults", path.display());
                Config::default()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_matches_hardcoded_values() {
        let config = Config::default();
        assert_eq!(
            config.keybindings.prefix,
            KeySpec { code: KeyCode::Char('b'), modifiers: KeyModifiers::CONTROL }
        );
        assert_eq!(config.keybindings.new_window, KeyCode::Char('c'));
        assert_eq!(config.keybindings.next_window, KeyCode::Char('n'));
        assert_eq!(config.keybindings.prev_window, KeyCode::Char('p'));
        assert_eq!(config.keybindings.split_horizontal, KeyCode::Char('%'));
        assert_eq!(config.keybindings.split_vertical, KeyCode::Char('"'));
        assert_eq!(config.keybindings.focus_up, KeyCode::Up);
        assert_eq!(config.keybindings.focus_down, KeyCode::Down);
        assert_eq!(config.keybindings.focus_left, KeyCode::Left);
        assert_eq!(config.keybindings.focus_right, KeyCode::Right);
        assert_eq!(config.keybindings.kill_pane, KeyCode::Char('x'));
        assert_eq!(config.keybindings.quit, KeyCode::Char('d'));

        assert_eq!(config.colors.focused_border, Color::Yellow);
        assert_eq!(config.colors.unfocused_border, Color::DarkGray);
        assert_eq!(config.colors.status_fg, None);
        assert_eq!(config.colors.status_bg, None);
    }

    #[test]
    fn partial_keybinding_override_only_changes_that_field() {
        let config = Config::from_toml_str("[keybindings]\nnew_window = \"a\"\n");
        let default = Config::default();
        assert_eq!(config.keybindings.new_window, KeyCode::Char('a'));
        assert_eq!(config.keybindings.next_window, default.keybindings.next_window);
        assert_eq!(config.keybindings.prev_window, default.keybindings.prev_window);
        assert_eq!(config.keybindings.prefix, default.keybindings.prefix);
    }

    #[test]
    fn partial_color_override_only_changes_that_field() {
        let config = Config::from_toml_str("[colors]\nfocused_border = \"red\"\n");
        assert_eq!(config.colors.focused_border, Color::Red);
        assert_eq!(config.colors.unfocused_border, Color::DarkGray);
    }

    #[test]
    fn malformed_toml_falls_back_to_default() {
        let config = Config::from_toml_str("this is not valid toml {{{");
        assert_eq!(config.keybindings.prefix, Config::default().keybindings.prefix);
        assert_eq!(config.colors.focused_border, Config::default().colors.focused_border);
    }

    #[test]
    fn invalid_field_falls_back_while_valid_field_applies() {
        let config = Config::from_toml_str(
            "[keybindings]\nnew_window = \"???\"\nnext_window = \"a\"\n",
        );
        assert_eq!(config.keybindings.new_window, Config::default().keybindings.new_window);
        assert_eq!(config.keybindings.next_window, KeyCode::Char('a'));
    }

    #[test]
    fn parse_key_control_letter() {
        assert_eq!(
            parse_key("C-b"),
            Some(KeySpec { code: KeyCode::Char('b'), modifiers: KeyModifiers::CONTROL })
        );
    }

    #[test]
    fn parse_key_arrow() {
        assert_eq!(parse_key("Up"), Some(KeySpec { code: KeyCode::Up, modifiers: KeyModifiers::NONE }));
    }

    #[test]
    fn parse_key_single_char() {
        assert_eq!(parse_key("c"), Some(KeySpec { code: KeyCode::Char('c'), modifiers: KeyModifiers::NONE }));
    }

    #[test]
    fn parse_key_invalid() {
        assert_eq!(parse_key("???"), None);
    }

    #[test]
    fn full_example_toml_deserializes() {
        let toml = r#"
prefix = "C-b"

[keybindings]
new_window = "c"
next_window = "n"
prev_window = "p"
split_horizontal = "%"
split_vertical = "\""
focus_up = "Up"
focus_down = "Down"
focus_left = "Left"
focus_right = "Right"
kill_pane = "x"
quit = "d"

[colors]
focused_border = "yellow"
unfocused_border = "darkgray"
status_fg = "white"
status_bg = "blue"
"#;
        let config = Config::from_toml_str(toml);
        assert_eq!(config.keybindings.prefix, Config::default().keybindings.prefix);
        assert_eq!(config.keybindings.new_window, KeyCode::Char('c'));
        assert_eq!(config.keybindings.quit, KeyCode::Char('d'));
        assert_eq!(config.colors.focused_border, Color::Yellow);
        assert_eq!(config.colors.unfocused_border, Color::DarkGray);
        assert_eq!(config.colors.status_fg, Some(Color::White));
        assert_eq!(config.colors.status_bg, Some(Color::Blue));
    }
}
