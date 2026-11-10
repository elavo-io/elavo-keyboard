//! elavo-keyboard: the on-screen keyboard daemon.
//!
//! It holds the seat's input method, shows the keyboard in a bottom layer
//! surface when a text field takes focus, and hides it when focus leaves.
//!
//! ```text
//! elavo-keyboard            run (labwc allows one input method per seat)
//! ELAVO_KEYBOARD_LOG=1      log activate, deactivate and every edit to stderr
//! ELAVO_KEYBOARD_SIZE=large compact, standard or large
//! ELAVO_KEYBOARD_KEYS_FILE=/path
//!                           after each show, write "id x y" screen centers
//!                           of every key, for the test scripts
//! ```

mod config;
mod im;
mod protocol;

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use elavo_keyboard_widget::{Action, Keyboard};
use gtk::prelude::*;
use gtk::{gdk, gio, glib};
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};

use crate::im::{keys, Command, Commands, UiEvent};

/// Focus moving from one field to the next deactivates and activates in
/// quick succession. Waiting this long before hiding avoids a flicker.
const HIDE_DELAY: Duration = Duration::from_millis(120);

fn main() -> glib::ExitCode {
    let app = gtk::Application::builder()
        .application_id("io.elavo.Keyboard")
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();
    app.connect_activate(start);
    app.run_with_args::<&str>(&[])
}

fn monitor_size() -> (i32, i32) {
    gdk::Display::default()
        .and_then(|d| d.monitors().item(0))
        .and_downcast::<gdk::Monitor>()
        .map(|m| {
            let g = m.geometry();
            (g.width(), g.height())
        })
        .unwrap_or((1366, 768))
}

struct Daemon {
    window: gtk::ApplicationWindow,
    keyboard: Keyboard,
    commands: Commands,
    hide_timer: Rc<RefCell<Option<glib::SourceId>>>,
    height: Cell<i32>,
    log: bool,
}

fn start(app: &gtk::Application) {
    let log = std::env::var_os("ELAVO_KEYBOARD_LOG").is_some_and(|v| !v.is_empty() && v != "0");
    let (commands, events) = match im::spawn(log) {
        Ok(pair) => pair,
        Err(e) => {
            eprintln!("elavo-keyboard: {e}");
            app.quit();
            return;
        }
    };
    // There is no window until a field takes focus; keep running anyway.
    std::mem::forget(app.hold());

    let config = config::load();
    let (_, screen_h) = monitor_size();
    let height = config.size.height_for(screen_h);

    let window = gtk::ApplicationWindow::builder()
        .application(app)
        .title("Keyboard")
        .css_classes(["elavo-osk-window"])
        .build();
    window.init_layer_shell();
    window.set_namespace(Some("elavo-keyboard"));
    window.set_layer(Layer::Overlay);
    window.set_anchor(Edge::Bottom, true);
    window.set_anchor(Edge::Left, true);
    window.set_anchor(Edge::Right, true);
    window.set_keyboard_mode(KeyboardMode::None);
    window.set_exclusive_zone(height);

    let keyboard = Keyboard::new(height);
    window.set_child(Some(&keyboard.widget()));

    let daemon = Rc::new(Daemon {
        window,
        keyboard,
        commands,
        hide_timer: Rc::default(),
        height: Cell::new(height),
        log,
    });
    if log {
        eprintln!("elavo-keyboard: ready, {height} px for a {screen_h} px screen");
    }

    let weak = Rc::downgrade(&daemon);
    daemon.keyboard.connect_action(move |action| {
        if let Some(d) = weak.upgrade() {
            d.on_action(action);
        }
    });

    // This loop owns the daemon for the life of the process.
    let owner = daemon.clone();
    let app_weak = app.downgrade();
    glib::spawn_future_local(async move {
        while let Ok(event) = events.recv().await {
            if event == UiEvent::Unavailable {
                break;
            }
            owner.on_event(event);
        }
        eprintln!("elavo-keyboard: the input method is unavailable (another one, such as elavo-input, may hold this seat)");
        if let Some(app) = app_weak.upgrade() {
            app.quit();
        }
    });

    let weak = Rc::downgrade(&daemon);
    daemon.keyboard.connect_page_changed(move |_| {
        let weak = weak.clone();
        glib::timeout_add_local_once(Duration::from_millis(150), move || {
            if let Some(d) = weak.upgrade() {
                if d.window.is_visible() {
                    d.write_key_centers();
                }
            }
        });
    });

    let weak = Rc::downgrade(&daemon);
    daemon.window.connect_map(move |_| {
        let weak = weak.clone();
        glib::timeout_add_local_once(Duration::from_millis(150), move || {
            if let Some(d) = weak.upgrade() {
                d.write_key_centers();
            }
        });
    });
}

impl Daemon {
    fn on_event(&self, event: UiEvent) {
        if self.log {
            eprintln!(
                "elavo-keyboard: ui {event:?}, visible={}",
                self.window.is_visible()
            );
        }
        match event {
            UiEvent::Show { .. } => {
                self.cancel_hide();
                if !self.window.is_visible() {
                    self.keyboard.reset();
                    self.window.set_visible(true);
                }
            }
            UiEvent::Hide => self.schedule_hide(),
            UiEvent::Unavailable => {}
        }
    }

    fn on_action(&self, action: Action) {
        match action {
            Action::Text(text) => self.commands.send(Command::Text(text)),
            Action::Backspace => self.commands.send(Command::Backspace),
            Action::Return => self.commands.send(Command::Key(keys::ENTER)),
            Action::Hide => {
                self.cancel_hide();
                self.window.set_visible(false);
            }
        }
    }

    fn cancel_hide(&self) {
        if let Some(id) = self.hide_timer.take() {
            id.remove();
        }
    }

    fn schedule_hide(&self) {
        self.cancel_hide();
        let window = self.window.clone();
        let timer = self.hide_timer.clone();
        let id = glib::timeout_add_local_once(HIDE_DELAY, move || {
            timer.replace(None);
            window.set_visible(false);
        });
        self.hide_timer.replace(Some(id));
    }

    fn write_key_centers(&self) {
        let Some(path) = std::env::var_os("ELAVO_KEYBOARD_KEYS_FILE") else {
            return;
        };
        let (_, screen_h) = monitor_size();
        let top = (screen_h - self.height.get()) as f32;
        let mut out = String::new();
        for (id, x, y) in self.keyboard.key_centers(&self.window) {
            out.push_str(&format!("{id} {} {}\n", x.round(), (y + top).round()));
        }
        if let Err(e) = std::fs::write(&path, out) {
            eprintln!("elavo-keyboard: could not write key centers: {e}");
        } else if self.log {
            let root = self.keyboard.widget();
            eprintln!(
                "elavo-keyboard: key centers written; surface {}x{}, keyboard {:?}",
                self.window.width(),
                self.window.height(),
                self.keyboard.page()
            );
            let _ = root;
        }
    }
}
