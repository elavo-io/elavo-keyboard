//! Dark keyboard colors after iPadOS. A light variant comes later.

use crate::Metrics;

pub fn css(m: &Metrics) -> String {
    let glyph = (f64::from(m.key) * 0.42).round() as i32;
    let icon = (f64::from(m.key) * 0.40).round() as i32;
    let word = (f64::from(m.key) * 0.30).round() as i32;
    let gap = m.gap;
    format!(
        r#"
window.elavo-osk-window {{
  background: transparent;
}}
.elavo-osk {{
  background-color: #2b2b2e;
  color: #ffffff;
  padding: 0 {gap}px {gap}px {gap}px;
}}
.elavo-osk-strip {{
  min-height: {strip}px;
}}
.elavo-osk-suggestion.divided {{
  border-left: 1px solid rgba(255, 255, 255, 0.12);
  margin: 9px 0;
}}
.elavo-osk-keys {{
  margin-top: {gap}px;
}}
.elavo-osk-key {{
  min-height: {key}px;
  border-radius: 8px;
  box-shadow: 0 1px 0 rgba(0, 0, 0, 0.55);
  transition: background-color 80ms ease-out;
}}
.elavo-osk-key.char {{
  background-color: #6b6b6f;
}}
.elavo-osk-key.fn {{
  background-color: #46464a;
}}
.elavo-osk-key.char.pressed {{
  background-color: #8e8e93;
  transition: none;
}}
.elavo-osk-key.fn.pressed {{
  background-color: #6b6b6f;
  transition: none;
}}
.elavo-osk-key.on {{
  background-color: #f2f2f7;
  color: #1c1c1e;
}}
.elavo-osk-key label.glyph {{
  font-size: {glyph}px;
  font-weight: 400;
}}
.elavo-osk-key label.icon {{
  font-size: {icon}px;
}}
.elavo-osk-key label.word {{
  font-size: {word}px;
}}
"#,
        strip = m.strip,
        key = m.key,
    )
}
