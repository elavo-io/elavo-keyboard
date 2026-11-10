//! A small stub until the Touchscreen pane grows keyboard settings.
//!
//! `~/.config/elavo/onscreen-keyboard.conf`:
//!
//! ```ini
//! [OnScreenKeyboard]
//! Size=standard   # compact, standard or large
//! ```
//!
//! `ELAVO_KEYBOARD_SIZE` overrides the file. Anything missing means Standard.

use std::path::PathBuf;

use elavo_keyboard_widget::Size;

pub const FILE_NAME: &str = "onscreen-keyboard.conf";

#[derive(Clone, Debug, Default)]
pub struct Config {
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
            in_group = name.trim().eq_ignore_ascii_case("OnScreenKeyboard");
            continue;
        }
        if !in_group {
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            if key.trim().eq_ignore_ascii_case("Size") {
                if let Some(size) = Size::parse(value) {
                    config.size = size;
                }
            }
        }
    }
    config
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_size() {
        let c = parse("[OnScreenKeyboard]\nSize=large\n");
        assert_eq!(c.size, Size::Large);
    }

    #[test]
    fn ignores_other_groups_and_junk() {
        let c = parse("[Other]\nSize=large\n[OnScreenKeyboard]\nSize=huge\n");
        assert_eq!(c.size, Size::Standard);
    }
}
