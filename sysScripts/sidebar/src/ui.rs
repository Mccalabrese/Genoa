//! Sidebar UI Builder
//!
//! Constructs the main GTK4 application window and manages the widget hierarchy.
//! This module handles:
//! 1. **Layer Shell Integration:** Positioning the window as an overlay attached to the active monitor.
//! 2. **State Management:** Tracking interaction times (to prevent slider bounce) and hover state (to fix focus bugs).
//! 3. **Async Polling:** Spawning background threads for Finance, Updates, and System Status to keep the UI responsive.
//! 4. **Event Handling:** Bridging UI clicks to shell commands via the `helpers` module.

use std::rc::Rc;
use std::cell::RefCell;
use gtk4::prelude::*;
use gtk4::{Application, ApplicationWindow, Box, Orientation, Align};
use gtk4_layer_shell::{Edge, Layer, LayerShell};
use serde_json::Value;
use chrono::{Datelike, Local};

use crate::style;
use crate::helpers;
use crate::media;
use crate::sysinfo;

pub fn build_ui(app: &Application) {
    // 1. Setup Window (Fixed Width)
    // Since we rely on the compositor (Sway/Niri) to place the window on the active monitor,
    // we cannot easily query the screen dimensions beforehand.
    // 400px is a safe, usable default for a sidebar.
    let final_width = 400; 

    let window = ApplicationWindow::builder()
        .application(app)
        .default_width(final_width)
        .title("My Sidebar")
        .build();

    // 2. Initialize Layer Shell
    // This tells the compositor "I am a panel/overlay, not a normal window."
    window.init_layer_shell();

    // OnDemand allows us to take keyboard focus when clicked (needed for specific interactions),
    // but pass it back to the underlying app when ignored.
    window.set_keyboard_mode(gtk4_layer_shell::KeyboardMode::OnDemand);
    window.set_layer(Layer::Overlay);
    
    // Monitor Auto-Detection
    // Passing `None` tells the protocol to assign this window to the monitor 
    // containing the active mouse pointer. This solves multi-monitor focus issues natively.
    window.set_monitor(None);

    // --- HOVER GUARD (Sway Focus Fix) ---
    // In tiling WMs like Sway, clicking a button inside this window might momentarily
    // cause the window to report "lost focus" before the click registers.
    // We track the mouse position to ignore false-positive close events.
    let is_hovered = Rc::new(RefCell::new(false));
    let hover_controller = gtk4::EventControllerMotion::new();
    let is_hovered_enter = is_hovered.clone();
    let is_hovered_leave = is_hovered.clone();

    hover_controller.connect_enter(move |_, _, _| {
        *is_hovered_enter.borrow_mut() = true;
    });

    hover_controller.connect_leave(move |_| {
        *is_hovered_leave.borrow_mut() = false;
    });
    
    window.add_controller(hover_controller);

    // --- SMART CLOSE LOGIC ---
    let is_hovered_close = is_hovered.clone();
    let launch_time = std::time::Instant::now();
    
    // Track if we ever successfully grabbed focus
    let has_been_active = Rc::new(RefCell::new(false));
    let has_been_active_clone = has_been_active.clone();

    window.connect_is_active_notify(move |win| {
        if win.is_active() {
            // We got focus! Mark it.
            *has_been_active_clone.borrow_mut() = true;
        } else {
            // We LOST focus (or never had it). Should we close?
            
            // 1. Startup Grace Period: 
            // Sway needs more time than Hyprland. Bump to 1500ms.
            if launch_time.elapsed().as_millis() < 2000 { 
                return; 
            }


            // 3. Hover Guard:
            // If mouse is physically over the window, don't close.
            if *is_hovered_close.borrow() { 
                return; 
            }

            // Write "Death Note" for the toggle script
            let _ = std::process::Command::new("touch")
                .arg("/tmp/sidebar_just_closed")
                .output();
            
            win.close();
        }
    });

    // 3. Anchor it
    // Pin the window to the Right side, stretching from Top to Bottom.
    window.set_anchor(Edge::Right, true);
    window.set_anchor(Edge::Top, true);
    window.set_anchor(Edge::Bottom, true);
    window.set_width_request(final_width);

    // --- BUILD UI LAYOUT ---
    // Load external CSS for styling (transparency, rounded corners, colors).
    style::load_css();
    let main_box = gtk4::Box::new(gtk4::Orientation::Vertical, 10);
    main_box.set_margin_top(10);
    main_box.set_margin_bottom(10);
    main_box.set_margin_start(10);
    main_box.set_margin_end(10);

    // --- ZONE 1: TOP (Quick Toggles) ---
    let top_box = gtk4::Box::new(gtk4::Orientation::Vertical, 15);
    top_box.add_css_class("zone"); // Adds the semi-transparent background card

    // Row 1: Session Controls (Logout, Reboot, etc.)
    let row_session = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
    row_session.set_homogeneous(true);    // Force all buttons to equal width
    let btn_idle = helpers::make_squared_button("view-conceal-symbolic", "Idle Inhibit");
    let btn_suspend = helpers::make_squared_button("system-suspend-symbolic", "Suspend");
    let btn_lock = helpers::make_squared_button("system-lock-screen-symbolic", "Lock Screen");
    let btn_logout = helpers::make_squared_button("system-log-out-symbolic", "Logout");
    let btn_restart = helpers::make_squared_button("system-reboot-symbolic", "Reboot");
    let btn_power = helpers::make_squared_button("system-shutdown-symbolic", "Power Off");
    
    row_session.append(&btn_idle);
    row_session.append(&btn_suspend);
    row_session.append(&btn_lock);
    row_session.append(&btn_logout);
    row_session.append(&btn_restart);
    row_session.append(&btn_power);

    // Row 2: Feature Toggles
    let row_toggles = gtk4::Box::new(gtk4::Orientation::Horizontal, 15);
    row_toggles.set_homogeneous(true);

    let btn_radio = helpers::make_icon_button("multimedia-player-symbolic", "Internet Radio");
    // Returns a button AND its badge label so we can update the number later
    let (btn_update, lbl_update_badge) = helpers::make_badged_button("software-update-available-symbolic", "0", "Update System");
    let btn_air = helpers::make_icon_button("airplane-mode-symbolic", "Airplane Mode");
    let btn_dns = helpers::make_icon_button("weather-overcast-symbolic", "Cloudflare DNS");
    let btn_mute = helpers::make_icon_button("audio-volume-muted-symbolic", "Mute Audio");
    let btn_wall = helpers::make_icon_button("image-x-generic-symbolic", "Change Wallpaper");
    let btn_hint = helpers::make_icon_button("emoji-objects-symbolic", "Show Keyhints");
    
    row_toggles.append(&btn_radio);
    row_toggles.append(&btn_wall);
    row_toggles.append(&btn_dns);
    row_toggles.append(&btn_update);
    row_toggles.append(&btn_air);
    row_toggles.append(&btn_mute);
    row_toggles.append(&btn_hint);

    // Sliders (Brightness & Volume)
    // We use the helper to create the consistent UI row, but capture the `Scale` object
    // so we can attach logic to it below.    
    let (box_brightness, scale_brightness) = helpers::make_slider_row("display-brightness-symbolic");
    let (box_volume, scale_volume) = helpers::make_slider_row("audio-volume-high-symbolic");

    // --- INTERACTION GUARD ---
    // Tracks the last time the user manually moved a slider.
    // The background poller checks this timestamp; if it's recent (< 3s),
    // it skips updating the slider to prevent visual "fighting" or bouncing.
    let last_interaction = Rc::new(RefCell::new(std::time::Instant::now()));

    // BRIGHTNESS HANDLER
    let last_interaction_b = last_interaction.clone();
    scale_brightness.connect_value_changed(move |s| {
        let val = s.value() as i32;
        *last_interaction_b.borrow_mut() = std::time::Instant::now();
        helpers::run_cmd(&format!("brightnessctl s {}%", val));
    });

    // VOLUME HANDLER
    let last_interaction_v = last_interaction.clone();
    scale_volume.connect_value_changed(move |s| {
        let val = s.value() / 100.0; 
        *last_interaction_v.borrow_mut() = std::time::Instant::now();
        helpers::run_cmd(&format!("wpctl set-volume @DEFAULT_AUDIO_SINK@ {}", val));
    });

    top_box.append(&row_session);
    top_box.append(&row_toggles);
    top_box.append(&box_brightness);
    top_box.append(&box_volume);

    // --- ZONE 2: MIDDLE (Media & SysInfo) ---
    // This box expands to fill all available vertical space, pushing Top and Bottom zones apart.
    let middle_box = Box::builder()
        .orientation(Orientation::Vertical)
        .spacing(20)
        .valign(Align::Fill)
        .vexpand(true)
        .build();

    // Dynamic Media Player (Slides in via `media.rs` logic only when playing)
    let media_widget = media::build();
    middle_box.append(&media_widget);

    // Static System Information (Host, Kernel, Uptime)
    let sys_widget = sysinfo::build();
    middle_box.append(&sys_widget);
    
    // --- ZONE 3: FINANCE TICKER ---
    let finance_box = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    finance_box.add_css_class("zone");
    
    let finance_label = gtk4::Label::builder()
        .label("Loading Market Data...")
        .use_markup(true)
        .css_classes(vec!["finance-text".to_string()])
        .justify(gtk4::Justification::Center)
        .wrap(false)
        .build();

    let finance_hint = gtk4::Label::builder()
        .label("(Click to launch App)")
        .css_classes(vec!["hint-text".to_string()])
        .build();

    finance_box.append(&finance_label);
    finance_box.append(&finance_hint);

    // Make the ticker clickable to launch the TUI app
    let click_gesture = gtk4::GestureClick::new();
    finance_box.add_controller(click_gesture.clone());

    // --- ZONE 4: CALENDAR ---
    let calendar_height = 300; // Fixed height since we removed screen calculation
    let bottom_box = gtk4::Box::new(gtk4::Orientation::Vertical, 5);
    bottom_box.add_css_class("zone");
    bottom_box.set_height_request(calendar_height);
    bottom_box.set_vexpand(false);

    // Stack allows swapping between Month View (Grid) and Day View (List)
    let main_stack = gtk4::Stack::new();
    main_stack.set_vexpand(true);
    main_stack.set_valign(gtk4::Align::Fill);

    let stack_switcher = gtk4::StackSwitcher::builder()
        .stack(&main_stack)
        .halign(gtk4::Align::Center)
        .build();

    // View A: Month Grid
    let month_view_box = gtk4::Box::new(gtk4::Orientation::Vertical, 5);
    month_view_box.set_valign(gtk4::Align::Fill);
    
    // Month Nav Header (< Month >)
    let nav_box = gtk4::Box::new(gtk4::Orientation::Horizontal, 10);
    nav_box.set_halign(gtk4::Align::Center);
    nav_box.set_margin_bottom(10);
    nav_box.set_margin_top(10);

    let btn_prev = gtk4::Button::builder().icon_name("go-previous-symbolic").css_classes(vec!["flat".to_string()]).build();
    let btn_next = gtk4::Button::builder().icon_name("go-next-symbolic").css_classes(vec!["flat".to_string()]).build();
    let label_month = gtk4::Label::builder()
        .css_classes(vec!["calendar-title".to_string()])
        .build();

    nav_box.append(&btn_prev);
    nav_box.append(&label_month);
    nav_box.append(&btn_next);

    // The Grid Container (Cleared and rebuilt on month change)
    let grid_container = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    grid_container.set_valign(gtk4::Align::Fill);
    grid_container.set_vexpand(true);

    month_view_box.append(&nav_box);
    month_view_box.append(&grid_container);

    // Calendar Logic: State Management
    let current_view_date = Rc::new(RefCell::new(Local::now().date_naive()));
    let grid_container_weak = grid_container.clone();
    let label_month_weak = label_month.clone();
    let view_date_state = current_view_date.clone();

    // Grid Redraw Function
    let refresh_grid = move || {
        let date = *view_date_state.borrow();
        label_month_weak.set_label(&date.format("%B %Y").to_string());
        // Remove old rows
        while let Some(child) = grid_container_weak.first_child() {
            grid_container_weak.remove(&child);
        }
        // Build new rows via helper        
        let new_grid = helpers::build_calendar_grid(date.year(), date.month());
        grid_container_weak.append(&new_grid);
    };

    refresh_grid(); // Initial Draw

    // Calendar Navigation Handlers
    let view_date_prev = current_view_date.clone();
    let refresh_prev = refresh_grid.clone();
    btn_prev.connect_clicked(move |_| {
        let mut d = *view_date_prev.borrow();
        // Handle year rollback
        if d.month() == 1 {
            d = d.with_month(12).unwrap().with_year(d.year() - 1).unwrap();
        } else {
            d = d.with_month(d.month() - 1).unwrap();
        }
        *view_date_prev.borrow_mut() = d;
        refresh_prev();
    });

    let view_date_next = current_view_date.clone();
    let refresh_next = refresh_grid.clone();
    btn_next.connect_clicked(move |_| {
        let mut d = *view_date_next.borrow();
        // Handle year rollover
        if d.month() == 12 {
            d = d.with_month(1).unwrap().with_year(d.year() + 1).unwrap();
        } else {
            d = d.with_month(d.month() + 1).unwrap();
        }
        *view_date_next.borrow_mut() = d;
        refresh_next();
    });

    main_stack.add_titled(&month_view_box, Some("month_view"), "Month");

    // View B: Day View (Agenda placeholder)
    let day_view_box = gtk4::Box::new(gtk4::Orientation::Vertical, 10);
    day_view_box.set_margin_top(20);
    day_view_box.set_margin_start(10);

    let now = Local::now();
    let day_title = gtk4::Label::builder()
        .label(format!("Agenda: {}", now.format("%A, %d %b")))
        .css_classes(vec!["finance-text".to_string()])
        .halign(gtk4::Align::Start)
        .build();
        
    let no_appt_label = gtk4::Label::builder()
        .label("No appointments scheduled.")
        .css_classes(vec!["hint-text".to_string()])
        .halign(gtk4::Align::Start)
        .build();

    day_view_box.append(&day_title);
    day_view_box.append(&no_appt_label);
    main_stack.add_titled(&day_view_box, Some("day_view"), "Day");

    bottom_box.append(&stack_switcher);
    bottom_box.append(&main_stack);
    
    // Assemble Main Window
    main_box.append(&top_box);
    main_box.append(&middle_box);
    main_box.append(&finance_box);
    main_box.append(&bottom_box);

    let scroll_wrapper = gtk4::ScrolledWindow::builder()
        .child(&main_box)
        .hscrollbar_policy(gtk4::PolicyType::Never)
        .vscrollbar_policy(gtk4::PolicyType::Automatic)
        .build();
    window.set_child(Some(&scroll_wrapper));

    // --- BUTTON EVENT HANDLERS ---
    //
    // Power Management
    btn_power.connect_clicked(move |_| helpers::run_cmd("systemctl poweroff"));
    btn_restart.connect_clicked(move |_| helpers::run_cmd("systemctl reboot"));

    // Smart Logout: Detects the active session to run the correct exit command
    btn_logout.connect_clicked(move |_| {
        let desktop = std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default().to_lowercase();
        let cmd = if desktop.contains("niri") { "niri msg action quit" }
        else if desktop.contains("sway") { "swaymsg exit" }
        else if desktop.contains("hyprland") { "hyprctl dispatch exit" }
        else { "loginctl terminate-user $USER" };
        helpers::run_cmd(cmd);
    });

    // Security
    btn_suspend.connect_clicked(move |_| {
        // Ensure lock screen starts BEFORE system sleeps
        helpers::run_cmd("pidof hyprlock >/dev/null || hyprlock & sleep 0.5; systemctl suspend");
    });
    btn_lock.connect_clicked(move |_| { helpers::run_cmd(" pidof hyprlock || hyprlock &"); });

    // --- Idle Inhibit Persistence ---
    // Query procps-ng for the state of our idle daemons. 
    // A suspended process (SIGSTOP) will have a 'T' in its stat column.
    let idle_check = helpers::get_stdout("ps -o stat= -C swayidle,hypridle");
    
    // If the process exists and is stopped, visually mark the button active on boot
    if idle_check.contains('T') {
        btn_idle.add_css_class("active");
    }

    // Toggle logic
    btn_idle.connect_clicked(move |btn| {
        if btn.has_css_class("active") {
            // It's currently paused. Remove UI highlight and resume the daemon.
            btn.remove_css_class("active");
            helpers::run_cmd("pkill -CONT hypridle || pkill -CONT swayidle");
        } else {
            // It's running normally. Add UI highlight and pause the daemon.
            btn.add_css_class("active");
            helpers::run_cmd("pkill -STOP hypridle || pkill -STOP swayidle");
        }
    });

    // Launchers
    btn_wall.connect_clicked(move |_| helpers::run_cmd("$HOME/.cargo/bin/wp-select"));
    btn_hint.connect_clicked(move |_| helpers::run_cmd("$HOME/.cargo/bin/kb-launcher"));
    btn_radio.connect_clicked(move |_| helpers::run_cmd("$HOME/.cargo/bin/radio-menu"));

    // Cloudflare DNS Polling Logic
    // Toggling takes time (sudo, network restart). We poll status for 45s to update the badge.
    let btn_dns_poll = btn_dns.clone();
    btn_dns.connect_clicked(move |_| {
        helpers::run_cmd("$HOME/.cargo/bin/cf-toggle");
        let btn_target = btn_dns_poll.clone();
        let mut attempts = 0;
        glib::timeout_add_local(std::time::Duration::from_secs(1), move || {
            attempts += 1;
            if let Ok(out) = std::process::Command::new("sh").arg("-c").arg("$HOME/.cargo/bin/cf-status").output() {
                if let Ok(json) = serde_json::from_slice::<Value>(&out.stdout) {
                    if let Some(class) = json.get("class").and_then(|v| v.as_str()) {
                        if class == "on" { btn_target.add_css_class("active"); }
                        else { btn_target.remove_css_class("active"); }
                    }
                }
            }
            if attempts >= 45 { glib::ControlFlow::Break } else { glib::ControlFlow::Continue }
        });
    });

    // Update Logic
    // Updates run in a terminal (via updater), so we just launch it and hide the badge optimistically.
    let lbl_update_badge_clone = lbl_update_badge.clone();
    btn_update.connect_clicked(move |_| {
        helpers::run_cmd("$HOME/.cargo/bin/updater");
        lbl_update_badge_clone.set_visible(false);
    });

    // Update Checker (Background Thread)
    // Runs checkupdates/yay every 30 minutes to avoid spamming the CPU/Network.
    let (update_tx, update_rx) = std::sync::mpsc::channel();
    let lbl_update_target = lbl_update_badge.clone();
    std::thread::spawn(move || {
        loop {
            if let Ok(out) = std::process::Command::new("sh").arg("-c").arg("$HOME/.cargo/bin/update-check").output() {
                let _ = update_tx.send(out.stdout);
            }
            std::thread::sleep(std::time::Duration::from_secs(1800));
        }
    });

    // Update Checker (UI Receiver)
    glib::timeout_add_local(std::time::Duration::from_secs(1), move || {
        if let Ok(stdout) = update_rx.try_recv() {
            if let Ok(json) = serde_json::from_slice::<Value>(&stdout) {
                if let Some(text) = json.get("text").and_then(|v| v.as_str()) {
                     lbl_update_target.set_label(text);
                     lbl_update_target.set_visible(text != "0");
                }
            }
        }
        glib::ControlFlow::Continue
    });

    // Airplane Mode (Optimistic UI)
    let btn_air_clone = btn_air.clone();
    btn_air.connect_clicked(move |_| {
        helpers::run_cmd("$HOME/.cargo/bin/rfkill-manager --toggle");
        if btn_air_clone.has_css_class("active") { btn_air_clone.remove_css_class("active"); }
        else { btn_air_clone.add_css_class("active"); }
    });

    // Audio Mute (Optimistic UI)
    let btn_mute_clone = btn_mute.clone();
    btn_mute.connect_clicked(move |_| {
        helpers::run_cmd("wpctl set-mute @DEFAULT_AUDIO_SINK@ toggle");
        if btn_mute_clone.has_css_class("active") { btn_mute_clone.remove_css_class("active"); }
        else { btn_mute_clone.add_css_class("active"); }
    });

    // Finance Widget (Background Thread)
    // Runs the external fetcher script and pipes JSON back to the UI.
    click_gesture.connect_pressed(move |_, _, _, _| {
        helpers::run_cmd("ghostty --title=waybar-finance -e $HOME/.cargo/bin/waybar-finance --tui");
    });

    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let output = std::process::Command::new("sh").arg("-c").arg("$HOME/.cargo/bin/waybar-finance").output();
        let _ = sender.send(output);
    });

    let finance_label_update = finance_label.clone();
    glib::timeout_add_local(std::time::Duration::from_millis(100), move || {
        if let Ok(Ok(out)) = receiver.try_recv() {
            if let Ok(json) = serde_json::from_slice::<Value>(&out.stdout) {
                if let Some(text) = json.get("text").and_then(|v| v.as_str()) {
                    // Manual HTML/Pango parsing to format the grid 
                    // (The API returns raw HTML spans, we need to insert newlines every 4 items)
                    let raw_items: Vec<&str> = text.split("</span> ").collect();
                    let mut grid_text = String::new();
                    for (i, item) in raw_items.iter().enumerate() {
                        if item.trim().is_empty() { continue; }
                        grid_text.push_str(item);
                        if !item.ends_with("</span>") { grid_text.push_str("</span>"); }
                        if (i + 1) % 4 == 0 { grid_text.push('\n'); } else { grid_text.push_str("      "); }
                    }
                    finance_label_update.set_markup(&grid_text);
                    if let Some(tt) = json.get("tooltip").and_then(|v| v.as_str()) {
                        finance_label_update.set_tooltip_markup(Some(tt));
                    }
                }
            }
            glib::ControlFlow::Break
        } else {
            glib::ControlFlow::Continue
        }
    });

    // ================= MASTER STATUS LOADER =================
    // To ensure the sidebar opens INSTANTLY, we don't block the main thread checking states.
    // Instead, we spawn one worker thread to check DNS, Airplane, Mute, Volume, and Brightness
    // in parallel, then update the UI once the data arrives (approx 50-100ms later).
    let btn_dns_load = btn_dns.clone();
    let btn_air_load = btn_air.clone();
    let btn_mute_load = btn_mute.clone();
    let scale_bright_load = scale_brightness.clone();
    let scale_vol_load = scale_volume.clone();

    let (status_tx, status_rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let dns_o = std::process::Command::new("sh").arg("-c").arg("$HOME/.cargo/bin/cf-status").output().ok();
        let air_o = std::process::Command::new("rfkill").arg("list").arg("wlan").output().ok();
        let mute_o = std::process::Command::new("sh").arg("-c").arg("wpctl get-volume @DEFAULT_AUDIO_SINK@").output().ok();
        let bright_o = std::process::Command::new("brightnessctl").arg("i").arg("-m").output().ok();
        let _ = status_tx.send((dns_o, air_o, mute_o, bright_o));
    });

    glib::timeout_add_local(std::time::Duration::from_millis(100), move || {
        if let Ok((dns_o, air_o, mute_o, bright_o)) = status_rx.try_recv() {
            // Apply DNS State
            if let Some(out) = dns_o {
                if let Ok(json) = serde_json::from_slice::<Value>(&out.stdout) {
                    if json.get("class").and_then(|v| v.as_str()) == Some("on") { btn_dns_load.add_css_class("active"); }
                }
            }
            // Apply Airplane State
            if let Some(out) = air_o {
                if String::from_utf8_lossy(&out.stdout).contains("Soft blocked: yes") { btn_air_load.add_css_class("active"); }
            }
            // Apply Mute/Volume State
            if let Some(out) = mute_o {
                let s = String::from_utf8_lossy(&out.stdout);
                if s.contains("[MUTED]") { btn_mute_load.add_css_class("active"); }
                if let Some(vol_str) = s.split_whitespace().nth(1) {
                    if let Ok(vol) = vol_str.parse::<f64>() { scale_vol_load.set_value(vol * 100.0); }
                }
            }
            // Apply Brightness State
            if let Some(out) = bright_o {
                if let Some(p) = String::from_utf8_lossy(&out.stdout).split(',').nth(3) {
                     if let Ok(val) = p.replace("%", "").replace("\n", "").parse::<f64>() {
                         scale_bright_load.set_value(val);
                     }
                }
            }
            glib::ControlFlow::Break
        } else {
            glib::ControlFlow::Continue
        }
    });
    
    // ================= SLIDER WATCHER =================
    // Watches for EXTERNAL changes (e.g., hardware keys) to update the sliders.
    // Respects the "Interaction Guard" to avoid fighting the user.
    let scale_bright_watch = scale_brightness.clone();
    let scale_vol_watch = scale_volume.clone();
    let last_interaction_watch = last_interaction.clone();

    glib::timeout_add_local(std::time::Duration::from_secs(2), move || {
        let sb_inner = scale_bright_watch.clone();
        let sv_inner = scale_vol_watch.clone();
        let li_inner = last_interaction_watch.clone();
        glib::timeout_add_seconds_local(1, move || {
            // Guard: If user touched slider < 3 seconds ago, SKIP update.
            if li_inner.borrow().elapsed().as_secs() < 3 { return glib::ControlFlow::Continue; }
            
            // Check Brightness
            if let Ok(out) = std::process::Command::new("brightnessctl").arg("i").arg("-m").output() {
                if let Some(p) = String::from_utf8_lossy(&out.stdout).split(',').nth(3) {
                    if let Ok(sys_val) = p.replace("%", "").replace("\n", "").parse::<f64>() {
                         if (sb_inner.value() - sys_val).abs() > 1.0 { sb_inner.set_value(sys_val); }
                    }
                }
            }
            // Check Volume
            if let Ok(out) = std::process::Command::new("sh").arg("-c").arg("wpctl get-volume @DEFAULT_AUDIO_SINK@").output() {
                if let Some(vol_str) = String::from_utf8_lossy(&out.stdout).split_whitespace().nth(1) {
                    if let Ok(vol_float) = vol_str.parse::<f64>() {
                        let sys_val = vol_float * 100.0;
                        if (sv_inner.value() - sys_val).abs() > 1.0 { sv_inner.set_value(sys_val); }
                    }
                }
            }
            glib::ControlFlow::Continue
        });
        glib::ControlFlow::Break
    });

    window.present();
}
