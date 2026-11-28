//! Genoa's standalone GTK4 settings application.
//!
//! The lock and sleep page persists the timing policy used by Genoa's managed
//! Niri and Sway idle services. Other settings areas remain intentionally
//! disconnected until their behavior has been reviewed independently.

#![deny(unsafe_op_in_unsafe_fn)]

mod settings;

use gtk4::prelude::*;
use gtk4::{Align, Application, ApplicationWindow, Box as GtkBox, Button, Label, Orientation};

fn main() {
    let smoke_test = matches!(std::env::args().nth(1).as_deref(), Some("--smoke-test"));
    let application = Application::builder()
        .application_id("io.github.genoa.Settings")
        .build();
    application.connect_activate(move |application| {
        if smoke_test {
            build_smoke_test(application);
        } else {
            settings::build(application);
        }
    });
    application.run();
}

fn build_smoke_test(application: &Application) {
    let content = GtkBox::builder()
        .orientation(Orientation::Vertical)
        .spacing(18)
        .margin_top(28)
        .margin_bottom(28)
        .margin_start(32)
        .margin_end(32)
        .build();
    content.append(
        &Label::builder()
            .label("Genoa Settings is connected")
            .css_classes(["title-2"])
            .build(),
    );
    content.append(
        &Label::builder()
            .label("The sidebar launcher successfully opened the GTK settings application.")
            .wrap(true)
            .max_width_chars(48)
            .build(),
    );
    let close = Button::with_label("Close");
    close.set_halign(Align::End);
    content.append(&close);

    let window = ApplicationWindow::builder()
        .application(application)
        .title("Genoa Settings")
        .default_width(420)
        .resizable(false)
        .child(&content)
        .build();
    let window_for_close = window.clone();
    close.connect_clicked(move |_| window_for_close.close());
    window.present();
}
