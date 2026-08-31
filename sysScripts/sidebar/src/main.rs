//! GTK4 layer-shell sidebar for Niri, Sway, and Hyprland.
//!
//! The application keeps the GTK thread responsible for rendering and input while
//! command and calendar work runs away from the main loop.

use gtk4::Application;
use gtk4::prelude::*;

mod helpers;
mod media;
mod style;
mod sysinfo;
mod ui;

fn main() {
    // Set these before GTK initializes. The Cairo renderer is deliberate: it is
    // more reliable for this transparent layer-shell window on this setup.
    unsafe {
        std::env::set_var("GTK_A11Y", "none");
        std::env::set_var("GTK_USE_PORTAL", "0");
        std::env::set_var("GSK_RENDERER", "cairo");
    }

    let app = Application::builder().build();

    app.connect_activate(|app| {
        if let Some(settings) = gtk4::Settings::default() {
            settings.set_gtk_icon_theme_name(Some("Adwaita"));
            // The sidebar supplies a dark palette of its own.  Do not let a
            // system-wide light preference turn GTK's unstyled control nodes
            // (such as StackSwitcher and Scale) into light widgets.
            settings.set_gtk_application_prefer_dark_theme(true);
        }
        ui::build_ui(app);
    });

    app.run();
}
