//! The Elavo on-screen keyboard as a plain GTK4 widget.
//!
//! The widget only draws keys and reports what was tapped through
//! [`Keyboard::connect_action`]. It knows nothing about Wayland, so the same
//! widget can sit in a layer-shell surface (the `elavo-keyboard` daemon) or
//! inside another app's window, such as the greeter, which can feed the
//! actions straight into its own `gtk::Entry`.

mod icons;
mod layout;
mod style;

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use std::time::{Duration, Instant};

use gtk::prelude::*;
use gtk::{gdk, glib, graphene};

pub use layout::{Kind, Page, COLUMNS};

/// What a key tap asks the host to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    /// Insert this text at the cursor.
    Text(String),
    /// Delete the character before the cursor (repeats while held).
    Backspace,
    Return,
    /// The hide-keyboard key.
    Hide,
}

/// Keyboard height classes, offered in the Touchscreen pane of System Settings.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Size {
    Compact,
    #[default]
    Standard,
    Large,
}

impl Size {
    pub fn parse(text: &str) -> Option<Self> {
        match text.trim().to_ascii_lowercase().as_str() {
            "compact" | "small" => Some(Self::Compact),
            "standard" | "medium" | "default" => Some(Self::Standard),
            "large" => Some(Self::Large),
            _ => None,
        }
    }

    /// Share of the screen height, and the floor and ceiling in pixels.
    fn rule(self) -> (f64, i32, i32) {
        match self {
            Self::Compact => (0.32, 240, 340),
            Self::Standard => (0.38, 260, 400),
            Self::Large => (0.44, 300, 460),
        }
    }

    /// Keyboard height for a landscape screen of this logical height.
    /// Standard on a 768 px screen is 292 px.
    pub fn height_for(self, screen_height: i32) -> i32 {
        let (share, min, max) = self.rule();
        ((f64::from(screen_height) * share).round() as i32).clamp(min, max)
    }
}

/// Pixel measurements derived from the total height.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Metrics {
    pub height: i32,
    pub strip: i32,
    pub gap: i32,
    pub key: i32,
}

impl Metrics {
    /// A 36 px suggestion strip, 4 key rows and 5 gaps of 8 px. Leftover
    /// pixels go to the strip, so 292 gives 4 rows of 54 px.
    pub fn for_height(height: i32) -> Self {
        let gap = 8;
        let strip = 36;
        let key = ((height - strip - 5 * gap) / 4).max(24);
        let strip = height - 4 * key - 5 * gap;
        Self {
            height,
            strip,
            gap,
            key,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shift {
    Off,
    /// The next letter is uppercase.
    Once,
    /// Caps lock, from a double tap on shift.
    Locked,
}

const DOUBLE_TAP: Duration = Duration::from_millis(350);
const REPEAT_DELAY: Duration = Duration::from_millis(450);
const REPEAT_EVERY: Duration = Duration::from_millis(70);

type ActionHandler = Rc<dyn Fn(Action)>;
type PageHandler = Rc<dyn Fn(Page)>;
/// A shift key: its cell, its icon, and which icon it shows.
type ShiftKey = (gtk::Widget, gtk::DrawingArea, Rc<Cell<icons::Icon>>);

struct Inner {
    root: gtk::Box,
    holder: gtk::Box,
    css: gtk::CssProvider,
    metrics: Cell<Metrics>,
    page: Cell<Page>,
    shift: Cell<Shift>,
    last_shift_tap: Cell<Option<Instant>>,
    /// Character keys on the current page: label, lower, upper.
    chars: RefCell<Vec<(gtk::Label, &'static str, &'static str)>>,
    shift_keys: RefCell<Vec<ShiftKey>>,
    /// Every key on the current page with its id, for tooling.
    keys: RefCell<Vec<(String, gtk::Widget)>>,
    handler: RefCell<Option<ActionHandler>>,
    page_handler: RefCell<Option<PageHandler>>,
    repeat: RefCell<Option<glib::SourceId>>,
}

/// The keyboard widget. Cloning shares the same keyboard.
#[derive(Clone)]
pub struct Keyboard {
    inner: Rc<Inner>,
}

impl Keyboard {
    /// A keyboard `height` pixels tall, filling the width it is given.
    pub fn new(height: i32) -> Self {
        let root = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .css_classes(["elavo-osk"])
            .hexpand(true)
            .build();
        let strip = gtk::Box::builder()
            .css_classes(["elavo-osk-strip"])
            .homogeneous(true)
            .build();
        for i in 0..3 {
            let cell = gtk::Box::builder()
                .css_classes(["elavo-osk-suggestion"])
                .build();
            if i > 0 {
                cell.add_css_class("divided");
            }
            strip.append(&cell);
        }
        root.append(&strip);
        let holder = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .vexpand(true)
            .build();
        root.append(&holder);

        let css = gtk::CssProvider::new();
        if let Some(display) = gdk::Display::default() {
            gtk::style_context_add_provider_for_display(
                &display,
                &css,
                gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
            );
        }

        let inner = Rc::new(Inner {
            root,
            holder,
            css,
            metrics: Cell::new(Metrics::for_height(height)),
            page: Cell::new(Page::Letters),
            shift: Cell::new(Shift::Off),
            last_shift_tap: Cell::new(None),
            chars: RefCell::default(),
            shift_keys: RefCell::default(),
            keys: RefCell::default(),
            handler: RefCell::default(),
            page_handler: RefCell::default(),
            repeat: RefCell::default(),
        });
        let keyboard = Self { inner };
        keyboard.set_height(height);
        keyboard.build_page();
        keyboard
    }

    pub fn widget(&self) -> gtk::Widget {
        self.inner.root.clone().upcast()
    }

    pub fn metrics(&self) -> Metrics {
        self.inner.metrics.get()
    }

    pub fn set_height(&self, height: i32) {
        let m = Metrics::for_height(height);
        self.inner.metrics.set(m);
        self.inner.css.load_from_string(&style::css(&m));
        self.inner.root.set_size_request(-1, height);
    }

    pub fn connect_action(&self, f: impl Fn(Action) + 'static) {
        self.inner.handler.replace(Some(Rc::new(f)));
    }

    /// Called after the keys are rebuilt for another page.
    pub fn connect_page_changed(&self, f: impl Fn(Page) + 'static) {
        self.inner.page_handler.replace(Some(Rc::new(f)));
    }

    pub fn page(&self) -> Page {
        self.inner.page.get()
    }

    pub fn set_page(&self, page: Page) {
        if self.inner.page.get() != page {
            self.inner.page.set(page);
            self.build_page();
        }
    }

    pub fn shift(&self) -> Shift {
        self.inner.shift.get()
    }

    pub fn set_shift(&self, shift: Shift) {
        set_shift(&self.inner, shift);
    }

    /// Back to letters with shift off, as when the keyboard comes up.
    pub fn reset(&self) {
        stop_repeat(&self.inner);
        self.set_page(Page::Letters);
        self.set_shift(Shift::Off);
    }

    /// The center of every key, in `relative_to`'s coordinates. Used by the
    /// test scripts to tap keys.
    pub fn key_centers(&self, relative_to: &impl IsA<gtk::Widget>) -> Vec<(String, f32, f32)> {
        self.inner
            .keys
            .borrow()
            .iter()
            .filter_map(|(id, w)| {
                let p = graphene::Point::new(w.width() as f32 / 2.0, w.height() as f32 / 2.0);
                w.compute_point(relative_to, &p)
                    .map(|p| (id.clone(), p.x(), p.y()))
            })
            .collect()
    }

    fn build_page(&self) {
        let inner = &self.inner;
        while let Some(child) = inner.holder.first_child() {
            inner.holder.remove(&child);
        }
        inner.chars.borrow_mut().clear();
        inner.shift_keys.borrow_mut().clear();
        inner.keys.borrow_mut().clear();

        let grid = gtk::Grid::builder()
            .css_classes(["elavo-osk-keys"])
            .column_homogeneous(true)
            .row_homogeneous(true)
            .column_spacing(inner.metrics.get().gap)
            .row_spacing(inner.metrics.get().gap)
            .vexpand(true)
            .hexpand(true)
            .build();
        for (r, keys) in layout::rows(inner.page.get()).iter().enumerate() {
            let mut col = 0;
            for key in keys {
                if key.kind != Kind::Gap {
                    let widget = make_key(inner, key);
                    grid.attach(&widget, col, r as i32, key.span, 1);
                }
                col += key.span;
            }
        }
        inner.holder.append(&grid);
        refresh_shift(inner);
        let handler = inner.page_handler.borrow().clone();
        if let Some(handler) = handler {
            handler(inner.page.get());
        }
    }
}

fn make_key(inner: &Rc<Inner>, key: &layout::Key) -> gtk::Widget {
    // A vertical box reports no baseline, so the grid does not shift rows
    // whose labels have different font sizes to line up their baselines.
    let cell = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .hexpand(true)
        .build();
    cell.add_css_class("elavo-osk-key");
    cell.add_css_class(if key.is_char_style() { "char" } else { "fn" });
    let icon_size = (f64::from(inner.metrics.get().key) * 0.40).round() as i32;
    match key.kind {
        Kind::Char { lower, upper } => {
            let label = key_label(key.label, "glyph");
            inner.chars.borrow_mut().push((label.clone(), lower, upper));
            cell.append(&label);
        }
        Kind::Shift => {
            let (area, state) = icons::icon(icons::Icon::Shift, icon_size);
            area.set_vexpand(true);
            inner
                .shift_keys
                .borrow_mut()
                .push((cell.clone().upcast(), area.clone(), state));
            cell.append(&area);
        }
        Kind::Backspace | Kind::Hide => {
            let which = if key.kind == Kind::Hide {
                icons::Icon::Hide
            } else {
                icons::Icon::Backspace
            };
            let (area, _) = icons::icon(which, icon_size);
            area.set_vexpand(true);
            cell.append(&area);
        }
        _ => cell.append(&key_label(key.label, "word")),
    }
    inner
        .keys
        .borrow_mut()
        .push((key.id(), cell.clone().upcast()));

    let gesture = gtk::GestureClick::new();
    gesture.set_button(0);
    let kind = key.kind;
    let weak = Rc::downgrade(inner);
    let pressed_cell = cell.clone();
    gesture.connect_pressed(move |_, _, _, _| {
        pressed_cell.add_css_class("pressed");
        let Some(inner) = weak.upgrade() else { return };
        if kind == Kind::Backspace {
            emit(&inner, Action::Backspace);
            start_repeat(&inner);
        }
    });
    let weak = Rc::downgrade(inner);
    let released_cell = cell.clone();
    gesture.connect_released(move |_, _, _, _| {
        released_cell.remove_css_class("pressed");
        let Some(inner) = weak.upgrade() else { return };
        stop_repeat(&inner);
        tap(&inner, kind);
    });
    // "stopped" also fires when the double-click timer runs out while the
    // key is still held, so only a cancelled touch ends the press here.
    let weak = Rc::downgrade(inner);
    let cancelled_cell = cell.clone();
    gesture.connect_cancel(move |_, _| {
        cancelled_cell.remove_css_class("pressed");
        if let Some(inner) = weak.upgrade() {
            stop_repeat(&inner);
        }
    });
    cell.add_controller(gesture);
    cell.upcast()
}

fn key_label(text: &str, class: &str) -> gtk::Label {
    gtk::Label::builder()
        .label(text)
        .hexpand(true)
        .vexpand(true)
        .halign(gtk::Align::Fill)
        .valign(gtk::Align::Fill)
        .xalign(0.5)
        .yalign(0.5)
        .css_classes([class])
        .build()
}

fn tap(inner: &Rc<Inner>, kind: Kind) {
    match kind {
        Kind::Char { lower, upper } => {
            let shift = inner.shift.get();
            let text = if shift == Shift::Off { lower } else { upper };
            emit(inner, Action::Text(text.to_owned()));
            if shift == Shift::Once {
                set_shift(inner, Shift::Off);
            }
        }
        Kind::Space => emit(inner, Action::Text(" ".into())),
        Kind::Return => emit(inner, Action::Return),
        Kind::Backspace | Kind::Gap => {}
        Kind::Shift => {
            let now = Instant::now();
            let quick = inner
                .last_shift_tap
                .get()
                .is_some_and(|t| now.duration_since(t) < DOUBLE_TAP);
            inner.last_shift_tap.set(Some(now));
            let next = match inner.shift.get() {
                Shift::Locked => Shift::Off,
                Shift::Once if quick => Shift::Locked,
                Shift::Once => Shift::Off,
                Shift::Off => Shift::Once,
            };
            set_shift(inner, next);
        }
        Kind::Goto(page) => {
            // Rebuild after the gesture that is running on this key ends.
            let weak = Rc::downgrade(inner);
            glib::idle_add_local_once(move || {
                if let Some(inner) = weak.upgrade() {
                    Keyboard { inner }.set_page(page);
                }
            });
        }
        Kind::Hide => emit(inner, Action::Hide),
    }
}

fn emit(inner: &Inner, action: Action) {
    let handler = inner.handler.borrow().clone();
    if let Some(handler) = handler {
        handler(action);
    }
}

fn set_shift(inner: &Inner, shift: Shift) {
    inner.shift.set(shift);
    refresh_shift(inner);
}

fn refresh_shift(inner: &Inner) {
    let shift = inner.shift.get();
    for (label, lower, upper) in inner.chars.borrow().iter() {
        label.set_text(if shift == Shift::Off { lower } else { upper });
    }
    for (cell, area, icon) in inner.shift_keys.borrow().iter() {
        cell.remove_css_class("on");
        icon.set(match shift {
            Shift::Off => icons::Icon::Shift,
            Shift::Once => icons::Icon::ShiftOn,
            Shift::Locked => icons::Icon::CapsLock,
        });
        if shift != Shift::Off {
            cell.add_css_class("on");
        }
        area.queue_draw();
    }
}

fn start_repeat(inner: &Rc<Inner>) {
    stop_repeat(inner);
    let weak: Weak<Inner> = Rc::downgrade(inner);
    let id = glib::timeout_add_local_once(REPEAT_DELAY, move || {
        let Some(inner) = weak.upgrade() else { return };
        // This one-shot source is finished; forget it before starting the
        // repeating one so stop_repeat never removes a dead source.
        inner.repeat.replace(None);
        let weak = Rc::downgrade(&inner);
        let id = glib::timeout_add_local(REPEAT_EVERY, move || match weak.upgrade() {
            Some(inner) => {
                emit(&inner, Action::Backspace);
                glib::ControlFlow::Continue
            }
            None => glib::ControlFlow::Break,
        });
        inner.repeat.replace(Some(id));
    });
    inner.repeat.replace(Some(id));
}

fn stop_repeat(inner: &Inner) {
    if let Some(id) = inner.repeat.take() {
        id.remove();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standard_fits_1366x768() {
        assert_eq!(Size::Standard.height_for(768), 292);
        let m = Metrics::for_height(292);
        assert_eq!((m.strip, m.key, m.gap), (36, 54, 8));
    }

    #[test]
    fn sizes_are_clamped() {
        assert_eq!(Size::Standard.height_for(1234), 400);
        assert_eq!(Size::Compact.height_for(600), 240);
        assert_eq!(Size::Large.height_for(768), 338);
    }

    #[test]
    fn leftover_pixels_go_to_the_strip() {
        let m = Metrics::for_height(295);
        assert_eq!(m.strip + 4 * m.key + 5 * m.gap, 295);
    }
}
