//! What is plugged in, and whether the touchscreen was just used, read
//! without root.
//!
//! `/dev/input` is not readable by the user, and Wayland does not tell one
//! client about another's input. So:
//!
//! - Keyboards come from `/proc/bus/input/devices`, read again on every
//!   text-field focus, which also covers hotplug.
//! - A recent touch or stylus contact shows up as a rising interrupt count
//!   in `/proc/interrupts` for the touchscreen's I2C device (for example
//!   `ELAN2097:00`). USB touchscreens share their controller's interrupt
//!   with other USB devices, so they are not counted.
//!
//! `ELAVO_KEYBOARD_INPUT_DEVICES` and `ELAVO_KEYBOARD_INTERRUPTS` point both
//! reads at other files, for tests.

use std::time::{Duration, Instant};

fn devices_path() -> String {
    std::env::var("ELAVO_KEYBOARD_INPUT_DEVICES")
        .unwrap_or_else(|_| "/proc/bus/input/devices".to_owned())
}

fn interrupts_path() -> String {
    std::env::var("ELAVO_KEYBOARD_INTERRUPTS").unwrap_or_else(|_| "/proc/interrupts".to_owned())
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Devices {
    /// A device with letter keys, a space bar and Enter is attached.
    pub keyboard: bool,
    /// Interrupt names of I2C touchscreens and pens.
    pub touch_irqs: Vec<String>,
}

pub fn scan() -> Devices {
    std::fs::read_to_string(devices_path())
        .map(|text| parse_devices(&text))
        .unwrap_or_default()
}

pub fn parse_devices(text: &str) -> Devices {
    let mut out = Devices::default();
    for block in text.split("\n\n") {
        let mut name = "";
        let mut sysfs = "";
        let mut ev = 0u64;
        let mut keys: Vec<u64> = Vec::new();
        let mut direct = false;
        for line in block.lines() {
            if let Some(rest) = line.strip_prefix("N: Name=") {
                name = rest.trim().trim_matches('"');
            } else if let Some(rest) = line.strip_prefix("S: Sysfs=") {
                sysfs = rest.trim();
            } else if let Some(rest) = line.strip_prefix("B: EV=") {
                ev = u64::from_str_radix(rest.trim(), 16).unwrap_or(0);
            } else if let Some(rest) = line.strip_prefix("B: KEY=") {
                // Most significant word first.
                keys = rest
                    .split_whitespace()
                    .rev()
                    .map(|w| u64::from_str_radix(w, 16).unwrap_or(0))
                    .collect();
            } else if let Some(rest) = line.strip_prefix("B: PROP=") {
                let bits = u64::from_str_radix(rest.trim(), 16).unwrap_or(0);
                direct = bits & 0x2 != 0;
            }
        }
        let has = |code: usize| {
            keys.get(code / 64)
                .is_some_and(|w| w >> (code % 64) & 1 == 1)
        };
        // EV_KEY, then KEY_Q, KEY_A, KEY_Z, KEY_SPACE and KEY_ENTER.
        if ev & 0x2 != 0 && [16, 30, 44, 57, 28].iter().all(|&c| has(c)) {
            out.keyboard = true;
        }
        if direct || name.to_ascii_lowercase().contains("touchscreen") {
            if let Some(irq) = i2c_name(sysfs) {
                if !out.touch_irqs.contains(&irq) {
                    out.touch_irqs.push(irq);
                }
            }
        }
    }
    out
}

/// `/devices/.../i2c-ELAN2097:00/0018:04F3:2783.0002/input/input18` gives
/// `ELAN2097:00`, the name `/proc/interrupts` uses.
fn i2c_name(sysfs: &str) -> Option<String> {
    sysfs
        .split('/')
        .filter_map(|part| part.strip_prefix("i2c-"))
        .find(|rest| rest.contains(':'))
        .map(str::to_owned)
}

/// The total interrupt count, over all CPUs, of the lines naming any of
/// `names`.
pub fn interrupt_total(text: &str, names: &[String]) -> u64 {
    let mut total = 0;
    for line in text.lines() {
        let Some((_, rest)) = line.split_once(':') else {
            continue;
        };
        let tokens: Vec<&str> = rest
            .split(|c: char| c.is_whitespace() || c == ',')
            .filter(|t| !t.is_empty())
            .collect();
        if !tokens.iter().any(|t| names.iter().any(|n| n == t)) {
            continue;
        }
        total += tokens
            .iter()
            .map_while(|t| t.parse::<u64>().ok())
            .sum::<u64>();
    }
    total
}

/// Notices touchscreen activity by sampling its interrupt count.
#[derive(Debug, Default)]
pub struct TouchWatch {
    names: Vec<String>,
    last_total: Option<u64>,
    last_change: Option<Instant>,
}

impl TouchWatch {
    pub fn set_names(&mut self, names: Vec<String>) {
        if names != self.names {
            self.names = names;
            self.last_total = None;
        }
    }

    pub fn has_touchscreen(&self) -> bool {
        !self.names.is_empty()
    }

    /// Read the counts now. A change since the last sample counts as a
    /// touch at this moment.
    pub fn sample(&mut self) {
        if self.names.is_empty() {
            return;
        }
        let Ok(text) = std::fs::read_to_string(interrupts_path()) else {
            return;
        };
        let total = interrupt_total(&text, &self.names);
        if self.last_total.is_some_and(|last| last != total) {
            self.last_change = Some(Instant::now());
        }
        self.last_total = Some(total);
    }

    /// When the touchscreen was last seen in use, if within `within`.
    pub fn touched_within(&self, within: Duration) -> Option<Instant> {
        self.last_change.filter(|t| t.elapsed() <= within)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DELL: &str = "\
I: Bus=0011 Vendor=0001 Product=0001 Version=ab41
N: Name=\"AT Translated Set 2 keyboard\"
S: Sysfs=/devices/platform/i8042/serio0/input/input3
B: PROP=0
B: EV=120013
B: KEY=2000000000000000 0 40000 0 0 0 0 11100f02902007 f780307cfb10f001 feffffdfffcfffff fffffffffffffffe

I: Bus=0019 Vendor=0000 Product=0001 Version=0000
N: Name=\"Power Button\"
S: Sysfs=/devices/platform/PNP0C0C:00/input/input1
B: PROP=0
B: EV=3
B: KEY=8000 10000000000000 0

I: Bus=0018 Vendor=04f3 Product=2783 Version=0100
N: Name=\"ELAN2097:00 04F3:2783\"
S: Sysfs=/devices/pci0000:00/0000:00:15.1/i2c_designware.1/i2c-1/i2c-ELAN2097:00/0018:04F3:2783.0002/input/input18
B: PROP=2
B: EV=1b
B: KEY=400 0 0 0 0 0

I: Bus=0018 Vendor=04f3 Product=30b6 Version=0100
N: Name=\"DELL0895:00 04F3:30B6 Touchpad\"
S: Sysfs=/devices/pci0000:00/0000:00:15.0/i2c_designware.0/i2c-0/i2c-DELL0895:00/0018:04F3:30B6.0001/input/input16
B: PROP=5
B: EV=1b
B: KEY=e520 10000 0 0 0 0
";

    #[test]
    fn finds_the_keyboard_and_the_touchscreen() {
        let d = parse_devices(DELL);
        assert!(d.keyboard);
        assert_eq!(d.touch_irqs, vec!["ELAN2097:00".to_owned()]);
    }

    #[test]
    fn a_power_button_is_not_a_keyboard() {
        let only_power = DELL.split("\n\n").nth(1).unwrap();
        assert!(!parse_devices(only_power).keyboard);
    }

    #[test]
    fn counts_only_the_touchscreen_line() {
        let text = "\
           CPU0       CPU1
  31:          0       1786  IR-IO-APIC   31-fasteoi   ELAN2097:00
  51:          4       3971  IR-IO-APIC   51-fasteoi   DELL0895:00
  16:          1         2  IR-IO-APIC   16-fasteoi   i2c_designware.0, idma64.0
";
        assert_eq!(interrupt_total(text, &["ELAN2097:00".to_owned()]), 1786);
        assert_eq!(interrupt_total(text, &["idma64.0".to_owned()]), 3);
    }
}
