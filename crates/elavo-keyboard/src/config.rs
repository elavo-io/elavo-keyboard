//! `~/.config/elavo/onscreen-keyboard.conf`, written by the Touchscreen pane
//! in System Settings. The daemon watches it and applies changes at once.
//!
//! ```ini
//! [OnScreenKeyboard]
//! Mode=automatic   # automatic, always or never
//! Size=standard    # compact, standard or large
//! ```
//!
//! `ELAVO_KEYBOARD_MODE` and `ELAVO_KEYBOARD_SIZE` override the file.
//! Anything missing or unknown means Automatic and Standard.

use std::path::PathBuf;

use elavo_keyboard_widget::Size;

pub const FILE_NAME: &str = "onscreen-keyboard.conf";
pub const GROUP: &str = "OnScreenKeyboard";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    /// After a tap or stylus touch, while no physical key is in use. With no
    /// keyboard attached at all, on every text field.
    #[default]
    Automatic,
    /// On every text field.
    Always,
    Never,
}

impl Mode {
    pub fn parse(text: &str) -> Option<Self> {
        match text.trim().to_ascii_lowercase().as_str() {
            "automatic" | "auto" => Some(Self::Automatic),
            "always" | "on" | "true" => Some(Self::Always),
            "never" | "off" | "false" => Some(Self::Never),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Config {
    pub mode: Mode,
    pub size: Size,
}

pub fn path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("elavo").join(FILE_NAME))
}

pub fn load() -> Config {
    let mut config = path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .map(|text| parse(&text))
        .unwrap_or_default();
    if let Some(mode) = std::env::var("ELAVO_KEYBOARD_MODE")
        .ok()
        .and_then(|v| Mode::parse(&v))
    {
        config.mode = mode;
    }
    if let Some(size) = std::env::var("ELAVO_KEYBOARD_SIZE")
        .ok()
        .and_then(|v| Size::parse(&v))
    {
        config.size = size;
    }
    config
}

pub fn parse(text: &str) -> Config {
    let mut config = Config::default();
    let mut in_group = false;
    for raw in text.lines() {
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.is_empty() || line.starts_with(';') {
            continue;
        }
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            in_group = name.trim().eq_ignore_ascii_case(GROUP);
            continue;
        }
        if !in_group {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        if key.eq_ignore_ascii_case("Mode") {
            if let Some(mode) = Mode::parse(value) {
                config.mode = mode;
            }
        } else if key.eq_ignore_ascii_case("Size") {
            if let Some(size) = Size::parse(value) {
                config.size = size;
            }
        }
    }
    config
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_mode_and_size() {
        let c = parse("[OnScreenKeyboard]\nMode=never\nSize=large\n");
        assert_eq!(c.mode, Mode::Never);
        assert_eq!(c.size, Size::Large);
        assert_eq!(
            parse("[OnScreenKeyboard]\nMode=Always\n").mode,
            Mode::Always
        );
    }

    #[test]
    fn missing_or_unknown_is_automatic_standard() {
        assert_eq!(parse(""), Config::default());
        let c = parse("[Other]\nMode=never\n[OnScreenKeyboard]\nSize=huge\nMode=sometimes\n");
        assert_eq!(c.mode, Mode::Automatic);
        assert_eq!(c.size, Size::Standard);
    }
}
