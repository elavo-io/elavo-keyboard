//! Key icons drawn with cairo, so they don't depend on which fonts or icon
//! themes are installed (the login screen has very few). They take the
//! key's text color.

use std::cell::Cell;
use std::rc::Rc;

use gtk::prelude::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Icon {
    Shift,
    ShiftOn,
    CapsLock,
    Backspace,
    Hide,
}

/// A drawing area showing `icon`, `size` pixels tall. Change the cell and
/// call `queue_draw` to switch icons.
pub fn icon(icon: Icon, size: i32) -> (gtk::DrawingArea, Rc<Cell<Icon>>) {
    let state = Rc::new(Cell::new(icon));
    let width = match icon {
        Icon::Backspace => (f64::from(size) * 1.35).round() as i32,
        Icon::Hide => (f64::from(size) * 1.25).round() as i32,
        _ => size,
    };
    let area = gtk::DrawingArea::builder()
        .content_width(width)
        .content_height(size)
        .halign(gtk::Align::Center)
        .valign(gtk::Align::Center)
        .can_target(false)
        .build();
    let cell = state.clone();
    area.set_draw_func(move |area, cr, w, h| {
        let color = area.color();
        cr.set_source_rgba(
            f64::from(color.red()),
            f64::from(color.green()),
            f64::from(color.blue()),
            f64::from(color.alpha()),
        );
        let (w, h) = (f64::from(w), f64::from(h));
        let line = (h / 13.0).max(1.4);
        cr.set_line_width(line);
        cr.set_line_join(gtk::cairo::LineJoin::Round);
        cr.set_line_cap(gtk::cairo::LineCap::Round);
        let _ = match cell.get() {
            Icon::Shift => shift(cr, w, h, line, false, false),
            Icon::ShiftOn => shift(cr, w, h, line, true, false),
            Icon::CapsLock => shift(cr, w, h, line, true, true),
            Icon::Backspace => backspace(cr, w, h, line),
            Icon::Hide => hide(cr, w, h, line),
        };
    });
    (area, state)
}

type Result = std::result::Result<(), gtk::cairo::Error>;

fn shift(cr: &gtk::cairo::Context, w: f64, h: f64, line: f64, filled: bool, bar: bool) -> Result {
    let s = h - line * 2.0;
    let x0 = (w - s) / 2.0;
    let y0 = line;
    let bottom = if bar { 0.74 } else { 0.92 };
    let pts = [
        (0.5, 0.04),
        (0.96, 0.5),
        (0.7, 0.5),
        (0.7, bottom),
        (0.3, bottom),
        (0.3, 0.5),
        (0.04, 0.5),
    ];
    cr.move_to(x0 + pts[0].0 * s, y0 + pts[0].1 * s);
    for &(px, py) in &pts[1..] {
        cr.line_to(x0 + px * s, y0 + py * s);
    }
    cr.close_path();
    if filled {
        cr.fill_preserve()?;
    }
    cr.stroke()?;
    if bar {
        cr.rectangle(x0 + 0.3 * s, y0 + 0.84 * s, 0.4 * s, 0.12 * s);
        cr.fill()?;
    }
    Ok(())
}

fn backspace(cr: &gtk::cairo::Context, w: f64, h: f64, line: f64) -> Result {
    let (l, r) = (line, w - line);
    let (t, b) = (h * 0.12, h * 0.88);
    let mid = h / 2.0;
    let notch = l + (b - t) * 0.5;
    let radius = (b - t) * 0.12;
    cr.move_to(l, mid);
    cr.line_to(notch, t);
    cr.line_to(r - radius, t);
    cr.arc(
        r - radius,
        t + radius,
        radius,
        -std::f64::consts::FRAC_PI_2,
        0.0,
    );
    cr.line_to(r, b - radius);
    cr.arc(
        r - radius,
        b - radius,
        radius,
        0.0,
        std::f64::consts::FRAC_PI_2,
    );
    cr.line_to(notch, b);
    cr.close_path();
    cr.stroke()?;
    let cx = (notch + r) / 2.0 + line * 0.3;
    let d = (b - t) * 0.2;
    cr.move_to(cx - d, mid - d);
    cr.line_to(cx + d, mid + d);
    cr.move_to(cx + d, mid - d);
    cr.line_to(cx - d, mid + d);
    cr.stroke()
}

fn hide(cr: &gtk::cairo::Context, w: f64, h: f64, line: f64) -> Result {
    // A small keyboard with a chevron under it.
    let (l, r) = (line, w - line);
    let (t, b) = (line, h * 0.62);
    let radius = (b - t) * 0.16;
    cr.new_sub_path();
    cr.arc(
        l + radius,
        t + radius,
        radius,
        std::f64::consts::PI,
        -std::f64::consts::FRAC_PI_2,
    );
    cr.arc(
        r - radius,
        t + radius,
        radius,
        -std::f64::consts::FRAC_PI_2,
        0.0,
    );
    cr.arc(
        r - radius,
        b - radius,
        radius,
        0.0,
        std::f64::consts::FRAC_PI_2,
    );
    cr.arc(
        l + radius,
        b - radius,
        radius,
        std::f64::consts::FRAC_PI_2,
        std::f64::consts::PI,
    );
    cr.close_path();
    cr.stroke()?;
    let dot = (line * 0.9).max(1.0);
    let inner_w = r - l - 4.0 * line;
    for (row, count) in [(0.32, 6), (0.52, 6)] {
        let y = t + (b - t) * row + (b - t) * 0.06;
        for i in 0..count {
            let x = l + 2.0 * line + inner_w * (f64::from(i) + 0.5) / f64::from(count);
            cr.rectangle(x - dot / 2.0, y - dot / 2.0, dot, dot);
        }
    }
    cr.fill()?;
    let y = t + (b - t) * 0.78;
    cr.move_to(l + (r - l) * 0.3, y);
    cr.line_to(r - (r - l) * 0.3, y);
    cr.stroke()?;
    let c = w / 2.0;
    let d = h * 0.13;
    cr.move_to(c - d, h * 0.74);
    cr.line_to(c, h * 0.74 + d);
    cr.line_to(c + d, h * 0.74);
    cr.stroke()
}
