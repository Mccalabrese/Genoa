//! Shared UI Utilities (helpers)
//!
//! A collection of factory functions to create consistent UI elements (Buttons, Sliders, Badges)
//! and handle command execution. This reduces boilerplate in `ui.rs`.

use gtk4::prelude::*;
use chrono::{Datelike, Local, NaiveDate};

// --- Button Factories ---

/// Creates a small, square button (20x20) typically used for the Session Control row.
pub fn make_squared_button(icon_name: &str, tooltip: &str) -> gtk4::Button {
    let icon = gtk4::Image::builder()
        .icon_name(icon_name)
        .pixel_size(20)
        .build();
    gtk4::Button::builder()
        .child(&icon)
        .css_classes(vec!["squared-btn".to_string()]) // Matches CSS rule for square radius
        .height_request(20)
        .tooltip_text(tooltip)
        .build()
}

/// Creates a larger, circular button (30x30) used for the Feature Toggles.
pub fn make_icon_button(icon_name: &str, tooltip: &str) -> gtk4::Button {
    // We explicitly build the Image to control pixel_size, otherwise GTK picks a default.
    let icon = gtk4::Image::builder()
        .icon_name(icon_name)
        .pixel_size(24)
        .build();

    gtk4::Button::builder()
        .child(&icon)
        .css_classes(vec!["circular-btn".to_string()]) // Matches CSS rule for 99px radius
        .height_request(30)
        .tooltip_text(tooltip)
        .build()
        
}
/// Creates a button with a "Notification Badge" overlay (Red circle with number).
/// Returns tuple: (The Button Widget, The Label Widget for the count).
pub fn make_badged_button(icon_name: &str, count: &str, tooltip: &str) -> (gtk4::Button, gtk4::Label) {
    // 1. Base Layer: The Icon
    let icon = gtk4::Image::builder()
        .icon_name(icon_name)
        .pixel_size(24)
        .build();
        
    // 2. Top Layer: The Badge
    let badge = gtk4::Label::builder()
        .label(count)
        .css_classes(vec!["badge".to_string()])
        .halign(gtk4::Align::End)   // Align to Top-Right corner
        .valign(gtk4::Align::Start) 
        .visible(count != "0")      // Auto-hide if count is zero
        .build();

    // 3. Stack them using GTK Overlay
    let overlay = gtk4::Overlay::builder()
        .child(&icon)
        .build();
    overlay.add_overlay(&badge);

    // 4. Wrap in Button
    let btn = gtk4::Button::builder()
        .child(&overlay)
        .css_classes(vec!["circular-btn".to_string()])
        .height_request(30)
        .tooltip_text(tooltip)
        .build();
    (btn, badge)
}

// --- Calendar Logic ---

/// Generates a Month View Grid for the given Year/Month.
/// Handles the math for "Empty slots before the 1st" and "Total days in month".
pub fn build_calendar_grid(year: i32, month: u32) -> gtk4::Grid {
    let grid = gtk4::Grid::builder()
        .column_spacing(5)
        .row_spacing(5)
        .hexpand(true)
        .vexpand(true)
        .halign(gtk4::Align::Fill)
        .valign(gtk4::Align::Fill)
        .column_homogeneous(true) // Force all day cells to be equal width
        .row_homogeneous(true)
        .build();

    // 1. Draw Headers (Su, Mo, Tu...)
    let days = ["Su", "Mo", "Tu", "We", "Th", "Fr", "Sa"];
    for (i, day) in days.iter().enumerate() {
        let label = gtk4::Label::builder()
            .label(*day)
            .css_classes(vec!["calendar-header".to_string()])
            .hexpand(true)
            .build();
        grid.attach(&label, i as i32, 0, 1, 1); // Row 0 is reserved for headers
    }

    // 2. Date Math
    // Find the first day of the month (e.g., Nov 1st)
    let first_day = NaiveDate::from_ymd_opt(year, month, 1).unwrap();
    
    // Calculate padding: If Nov 1st is Wednesday (3), we need 3 empty slots (Sun, Mon, Tue).
    let start_offset = first_day.weekday().num_days_from_sunday(); 
    
    // Calculate total days in month:
    // Rust's chrono doesn't have `days_in_month()`, so we subtract:
    // (First day of NEXT month) - (First day of THIS month)
    let next_month = if month == 12 { 1 } else { month + 1 };
    let next_year = if month == 12 { year + 1 } else { year };
    let next_first = NaiveDate::from_ymd_opt(next_year, next_month, 1).unwrap();
    let days_in_month = next_first.signed_duration_since(first_day).num_days();

    // 3. Render the Grid
    let mut col = start_offset as i32;
    let mut row = 1; // Start at Row 1

    let today = Local::now().date_naive();

    for day_num in 1..=days_in_month {
        // Build the Cell Content (Vertical Box: Number + Dot)
        let vbox = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
        vbox.set_valign(gtk4::Align::Center);
        
        let num_label = gtk4::Label::builder()
            .label(day_num.to_string())
            .css_classes(vec!["calendar-day-num".to_string()])
            .build();
        
        // Appointment Indicator (The "Red Dot")
        // TODO: Hook this up to real data from cal-tui json export later.
        // Currently assumes every 5th day has an appointment for visual testing.
        let has_appointment = day_num % 5 == 0; 
        
        let dot_label = gtk4::Label::builder()
            .label("•")
            .css_classes(vec!["calendar-dot".to_string()])
            .visible(has_appointment) // <--- Logic hooks here later
            .build();

        vbox.append(&num_label);
        vbox.append(&dot_label);

        // Wrap in a transparent button to make it clickable
        let btn = gtk4::Button::builder()
            .child(&vbox)
            .css_classes(vec!["calendar-day-btn".to_string()])
            .hexpand(true)
            .vexpand(true)
            .valign(gtk4::Align::Fill)
            .build();

        // Highlight Today
        if today.year() == year && today.month() == month && today.day() == day_num as u32 {
            btn.add_css_class("today");
        }
        
        // Click Action: Launch Calendar TUI focused on this date
        btn.connect_clicked(move |_| {
            println!("Clicked Date: {}/{}/{}", year, month, day_num);
            let cmd = format!("ghostty --title=calendar-tui -e $HOME/.cargo/bin/cal-tui --date {}-{}-{}", year, month, day_num);
            run_cmd(&cmd);
        });

        grid.attach(&btn, col, row, 1, 1);

        // Cursor Management: Move right, wrap to new row if needed
        col += 1;
        if col > 6 {
            col = 0;
            row += 1;
        }
    }

    grid
}

// --- Slider Factory ---

/// Creates a standardized Slider Row (Icon + Scale).
/// Returns (Container Box, The Scale Widget).
/// Note: The caller must attach the `value_changed` signal to the returned Scale.
pub fn make_slider_row(icon_name: &str) -> (gtk4::Box, gtk4::Scale) {
    let box_row = gtk4::Box::new(gtk4::Orientation::Horizontal, 10);

    let icon = gtk4::Image::builder()
        .icon_name(icon_name)
        .pixel_size(20)
        .build();

    let scale = gtk4::Scale::with_range(gtk4::Orientation::Horizontal, 0.0, 100.0, 1.0);
    scale.set_hexpand(true);
    scale.set_draw_value(false); // Hide the number (we use visual feedback)

    box_row.append(&icon);
    box_row.append(&scale);

    (box_row, scale)
}

// --- System Utilities ---

/// Fires a shell command asynchronously (fire-and-forget).
/// Uses `spawn()` instead of `output()` to avoid blocking the UI thread.
pub fn run_cmd(cmd: &str) {
    let _ = std::process::Command::new("sh")
        .arg("-c")
        .arg(cmd)
        .spawn();
}
/// Executes a shell command and returns its trimmed stdout.
/// Handles both simple commands (e.g., "hostname") and complex piped commands (e.g., "sh -c ...").
/// Returns "N/A" on failure instead of panicking to keep the UI stable.
pub fn get_stdout(cmd: &str) -> String {
    let output = if cmd.contains('\'') {
        // Handle complex piped commands by invoking the shell directly
        std::process::Command::new("sh").arg("-c").arg(cmd).output()
    } else {
        // Handle simple commands directly (cleaner process tree)
        let parts: Vec<&str> = cmd.split_whitespace().collect();
        std::process::Command::new(parts[0]).args(&parts[1..]).output()
    };

    match output {
        Ok(o) => String::from_utf8_lossy(&o.stdout).trim().to_string(),
        Err(_) => "N/A".to_string(),
    }
}
