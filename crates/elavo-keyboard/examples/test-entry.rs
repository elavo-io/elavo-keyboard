//! A small dark window with a one-line entry, a multi-line field and a
//! button, for trying the keyboard. Field contents are printed to stdout.
//!
//! cargo run --example test-entry

use gtk::glib;
use gtk::prelude::*;

fn main() -> glib::ExitCode {
    let app = gtk::Application::builder()
        .application_id("io.elavo.KeyboardTest")
        .build();
    app.connect_activate(|app| {
        if let Some(settings) = gtk::Settings::default() {
            settings.set_gtk_application_prefer_dark_theme(true);
        }
        let column = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(12)
            .margin_top(24)
            .margin_bottom(24)
            .margin_start(24)
            .margin_end(24)
            .build();
        let title = gtk::Label::builder()
            .label("Keyboard test")
            .halign(gtk::Align::Start)
            .css_classes(["title-2"])
            .build();
        let entry = gtk::Entry::builder()
            .placeholder_text("Tap here to type")
            .build();
        entry.connect_changed(|e| println!("entry: {:?}", e.text()));
        entry.connect_activate(|e| println!("entry activated: {:?}", e.text()));
        let view = gtk::TextView::builder()
            .wrap_mode(gtk::WrapMode::WordChar)
            .vexpand(true)
            .top_margin(8)
            .left_margin(8)
            .build();
        view.buffer().connect_changed(|b| {
            let (start, end) = b.bounds();
            println!("notes: {:?}", b.text(&start, &end, false));
        });
        let scroller = gtk::ScrolledWindow::builder()
            .child(&view)
            .vexpand(true)
            .has_frame(true)
            .build();
        let leave = gtk::Button::builder()
            .label("Leave the text field")
            .halign(gtk::Align::Start)
            .focus_on_click(true)
            .build();
        leave.connect_clicked(|b| {
            b.grab_focus();
            println!("focus: button");
        });
        column.append(&title);
        column.append(&entry);
        column.append(&scroller);
        column.append(&leave);

        let window = gtk::ApplicationWindow::builder()
            .application(app)
            .title("Keyboard test")
            .default_width(900)
            .default_height(560)
            .child(&column)
            .build();
        if std::env::var_os("TEST_MAXIMIZE").is_some() {
            window.maximize();
        }
        window.present();
        entry.grab_focus();
    });
    app.run()
}
