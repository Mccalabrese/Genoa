//! Sidebar window composition and wiring.
//!
//! This module builds the GTK layout, connects controls, and coordinates background
//! polling so command I/O does not block the main loop.

use async_channel::unbounded;
use chrono::{Datelike, Local, NaiveDate};
use gtk4::prelude::*;
use gtk4::{Align, Application, ApplicationWindow, Box, Orientation};
use gtk4_layer_shell::{Edge, Layer, LayerShell};
use serde_json::Value;
use std::cell::RefCell;
use std::rc::Rc;

use crate::helpers;
use crate::media;
use crate::style;
use crate::sysinfo;

struct SliderSnapshot {
    brightness: Option<f64>,
    volume: Option<f64>,
}

struct CalendarMonthSnapshot {
    year: i32,
    month: u32,
    events: Vec<helpers::CalendarEvent>,
}

// Parse brightnessctl CSV output and return percentage in [0, 100].
fn parse_brightness_pct(out: &[u8]) -> Option<f64> {
    String::from_utf8_lossy(out)
        .split(',')
        .nth(3)
        .and_then(|p| p.replace('%', "").trim().parse::<f64>().ok())
}

// Parse wpctl output and normalize to percentage in [0, 100].
fn parse_volume_pct(out: &[u8]) -> Option<f64> {
    String::from_utf8_lossy(out)
        .split_whitespace()
        .nth(1)
        .and_then(|v| v.parse::<f64>().ok())
        .map(|v| v * 100.0)
}

pub fn build_ui(app: &Application) {
    // The compositor places the layer on the active monitor, so use a stable width.
    let final_width = 400;
    let (calendar_req_tx, calendar_worker_rx) = helpers::spawn_calendar_worker();
    let window = ApplicationWindow::builder()
        .application(app)
        .default_width(final_width)
        .title("My Sidebar")
        .build();

    window.init_layer_shell();

    window.set_keyboard_mode(gtk4_layer_shell::KeyboardMode::OnDemand);
    window.set_layer(Layer::Overlay);

    window.set_monitor(None);

    // Keep the sidebar open while the pointer is over it. This avoids false close
    // events when a compositor briefly reports lost focus during a click.
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

    let is_hovered_close = is_hovered.clone();
    let launch_time = std::time::Instant::now();

    window.connect_is_active_notify(move |win| {
        if !win.is_active() {
            if launch_time.elapsed().as_millis() < 2000 {
                return;
            }

            if *is_hovered_close.borrow() {
                return;
            }

            let _ = std::process::Command::new("touch")
                .arg("/tmp/sidebar_just_closed")
                .output();

            win.close();
        }
    });

    window.set_anchor(Edge::Right, true);
    window.set_anchor(Edge::Top, true);
    window.set_anchor(Edge::Bottom, true);
    window.set_width_request(final_width);

    style::load_css();
    let main_box = gtk4::Box::new(gtk4::Orientation::Vertical, 10);
    main_box.set_margin_top(10);
    main_box.set_margin_bottom(10);
    main_box.set_margin_start(10);
    main_box.set_margin_end(10);

    let top_box = gtk4::Box::new(gtk4::Orientation::Vertical, 15);
    top_box.add_css_class("zone");

    let row_session = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
    row_session.set_homogeneous(true); // Force all buttons to equal width
    let btn_idle = helpers::make_squared_button("view-conceal-symbolic", "Idle Inhibit");
    let btn_suspend = helpers::make_squared_button("weather-clear-night-symbolic", "Suspend");
    let btn_lock = helpers::make_squared_button("system-lock-screen-symbolic", "Lock Screen");
    let btn_logout = helpers::make_squared_button("system-log-out-symbolic", "Logout");
    let btn_restart = helpers::make_squared_button("view-refresh-symbolic", "Reboot");
    let btn_power = helpers::make_squared_button("system-shutdown-symbolic", "Power Off");

    row_session.append(&btn_idle);
    row_session.append(&btn_suspend);
    row_session.append(&btn_lock);
    row_session.append(&btn_logout);
    row_session.append(&btn_restart);
    row_session.append(&btn_power);

    let row_toggles = gtk4::Box::new(gtk4::Orientation::Horizontal, 15);
    row_toggles.set_homogeneous(true);

    let btn_radio = helpers::make_icon_button("multimedia-player-symbolic", "Internet Radio");
    let (btn_update, lbl_update_badge) =
        helpers::make_badged_button("software-update-available-symbolic", "0", "Update System");
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
    let (box_brightness, scale_brightness) =
        helpers::make_slider_row("display-brightness-symbolic");
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
        let level = format!("{}%", val);
        helpers::run_command("brightnessctl", &["s", level.as_str()]);
    });

    // VOLUME HANDLER
    let last_interaction_v = last_interaction.clone();
    scale_volume.connect_value_changed(move |s| {
        let val = s.value() / 100.0;
        *last_interaction_v.borrow_mut() = std::time::Instant::now();
        let level = val.to_string();
        helpers::run_command(
            "wpctl",
            &["set-volume", "@DEFAULT_AUDIO_SINK@", level.as_str()],
        );
    });

    top_box.append(&row_session);
    top_box.append(&row_toggles);
    top_box.append(&box_brightness);
    top_box.append(&box_volume);

    let middle_box = Box::builder()
        .orientation(Orientation::Vertical)
        .spacing(10)
        .valign(Align::Fill)
        .vexpand(true)
        .build();

    let media_widget = media::build();
    middle_box.append(&media_widget);

    let sys_widget = sysinfo::build();
    middle_box.append(&sys_widget);

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

    let click_gesture = gtk4::GestureClick::new();
    finance_box.add_controller(click_gesture.clone());

    let calendar_height = 300;
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
    stack_switcher.add_css_class("calendar-switcher");

    // View A: Month Grid
    let month_view_box = gtk4::Box::new(gtk4::Orientation::Vertical, 5);
    month_view_box.set_valign(gtk4::Align::Fill);

    // Month Nav Header (< Month >)
    let nav_box = gtk4::Box::new(gtk4::Orientation::Horizontal, 10);
    nav_box.set_halign(gtk4::Align::Center);
    nav_box.set_margin_bottom(10);
    nav_box.set_margin_top(10);

    let btn_prev = gtk4::Button::builder()
        .icon_name("go-previous-symbolic")
        .css_classes(vec!["flat".to_string(), "calendar-nav-btn".to_string()])
        .build();
    let btn_next = gtk4::Button::builder()
        .icon_name("go-next-symbolic")
        .css_classes(vec!["flat".to_string(), "calendar-nav-btn".to_string()])
        .build();
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

    let current_view_date = Rc::new(RefCell::new(Local::now().date_naive()));
    let calendar_cache: Rc<RefCell<Option<CalendarMonthSnapshot>>> = Rc::new(RefCell::new(None));

    // Keep one cached month and one request in flight at a time.
    let loaded_month = Rc::new(RefCell::new(None::<(i32, u32)>));
    let is_in_flight = Rc::new(RefCell::new(false));

    let request_calendar_month = {
        let loaded_month_req = loaded_month.clone();
        let is_in_flight_req = is_in_flight.clone();
        let calendar_req_tx = calendar_req_tx.clone();

        Rc::new(move |date: NaiveDate| {
            let month_key = (date.year(), date.month());

            // Drop duplicate or overlapping requests
            if *loaded_month_req.borrow() == Some(month_key) || *is_in_flight_req.borrow() {
                return;
            }

            *is_in_flight_req.borrow_mut() = true;
            let _ = calendar_req_tx.send_blocking(helpers::CalendarRequest {
                year: month_key.0,
                month: month_key.1,
            });
        })
    };
    let grid_container = grid_container.clone();
    let label_month = label_month.clone();
    let view_date_state = current_view_date.clone();
    let calendar_cache_for_grid = calendar_cache.clone();
    let request_calendar_month_for_grid = request_calendar_month.clone();

    let refresh_grid = move || {
        let date = *view_date_state.borrow();
        label_month.set_label(&date.format("%B %Y").to_string());
        while let Some(child) = grid_container.first_child() {
            grid_container.remove(&child);
        }
        let cache = calendar_cache_for_grid.borrow();
        let empty_events: &[helpers::CalendarEvent] = &[];
        let events = cache
            .as_ref()
            .filter(|snapshot| snapshot.year == date.year() && snapshot.month == date.month())
            .map(|snapshot| snapshot.events.as_slice())
            .unwrap_or(empty_events);

        if events.is_empty() {
            request_calendar_month_for_grid(date);
        }

        let new_grid = helpers::build_calendar_grid_from_events(date.year(), date.month(), events);
        grid_container.append(&new_grid);
    };

    refresh_grid();

    let shift_month = |date: chrono::NaiveDate, delta: i32| -> chrono::NaiveDate {
        let mut year = date.year();
        let mut month = date.month() as i32 + delta;

        while month < 1 {
            month += 12;
            year -= 1;
        }
        while month > 12 {
            month -= 12;
            year += 1;
        }

        let target_month = month as u32;
        let mut day = date.day();
        while day > 1 {
            if let Some(valid_date) = chrono::NaiveDate::from_ymd_opt(year, target_month, day) {
                return valid_date;
            }
            day -= 1;
        }

        chrono::NaiveDate::from_ymd_opt(year, target_month, 1).unwrap_or(date)
    };

    let view_date_prev = current_view_date.clone();
    let refresh_prev = refresh_grid.clone();
    btn_prev.connect_clicked(move |_| {
        let d = *view_date_prev.borrow();
        let d = shift_month(d, -1);
        *view_date_prev.borrow_mut() = d;
        refresh_prev();
    });

    let view_date_next = current_view_date.clone();
    let refresh_next = refresh_grid.clone();
    btn_next.connect_clicked(move |_| {
        let d = *view_date_next.borrow();
        let d = shift_month(d, 1);
        *view_date_next.borrow_mut() = d;
        refresh_next();
    });

    main_stack.add_titled(&month_view_box, Some("month_view"), "Month");

    let day_view_box = gtk4::Box::new(gtk4::Orientation::Vertical, 10);
    day_view_box.set_margin_top(20);
    day_view_box.set_margin_start(10);

    let day_nav = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
    let btn_day_prev = gtk4::Button::builder()
        .icon_name("go-previous-symbolic")
        .css_classes(vec!["flat".to_string(), "calendar-nav-btn".to_string()])
        .build();
    let btn_day_next = gtk4::Button::builder()
        .icon_name("go-next-symbolic")
        .css_classes(vec!["flat".to_string(), "calendar-nav-btn".to_string()])
        .build();

    let day_title = gtk4::Label::builder()
        .label("Agenda")
        .css_classes(vec![
            "finance-text".to_string(),
            "calendar-day-title".to_string(),
        ])
        .halign(gtk4::Align::Start)
        .hexpand(true)
        .build();
    day_nav.append(&btn_day_prev);
    day_nav.append(&day_title);
    day_nav.append(&btn_day_next);

    let agenda_list = gtk4::Box::new(gtk4::Orientation::Vertical, 6);

    let day_state = current_view_date.clone();
    let day_title_clone = day_title.clone();
    let agenda_list_clone = agenda_list.clone();
    let calendar_cache_for_day = calendar_cache.clone();
    let request_calendar_month_for_day = request_calendar_month.clone();
    let refresh_day_view: Rc<dyn Fn()> = Rc::new(move || {
        let date = *day_state.borrow();
        day_title_clone.set_label(&format!("Agenda: {}", date.format("%A, %d %b")));

        while let Some(child) = agenda_list_clone.first_child() {
            agenda_list_clone.remove(&child);
        }

        let cache = calendar_cache_for_day.borrow();
        let empty_events: &[helpers::CalendarEvent] = &[];
        let events = cache
            .as_ref()
            .filter(|snapshot| snapshot.year == date.year() && snapshot.month == date.month())
            .map(|snapshot| snapshot.events.as_slice())
            .unwrap_or(empty_events);

        if events.is_empty() {
            request_calendar_month_for_day(date);
        }

        let appointments = helpers::get_day_appointments_from_events(date, events);
        if appointments.is_empty() {
            let empty = gtk4::Label::builder()
                .label("No appointments scheduled.")
                .css_classes(vec!["hint-text".to_string()])
                .halign(gtk4::Align::Start)
                .build();
            agenda_list_clone.append(&empty);
            return;
        }

        for appt in appointments.into_iter().take(8) {
            let text = if appt.all_day {
                format!("All day  {}", appt.summary)
            } else {
                format!(
                    "{}  {} ({}m)",
                    appt.time, appt.summary, appt.duration_minutes
                )
            };
            let button = gtk4::Button::builder()
                .label(&text)
                .halign(gtk4::Align::Fill)
                .css_classes(vec!["flat".to_string(), "agenda-event".to_string()])
                .tooltip_text("Open in GNOME Calendar")
                .build();

            let event_uid = appt.uid.clone();
            button.connect_clicked(move |_| {
                helpers::run_command("gnome-calendar", &["--uuid", event_uid.as_str()]);
            });
            agenda_list_clone.append(&button);
        }
    });

    refresh_day_view();

    let calendar_cache_update = calendar_cache.clone();
    let refresh_grid_update = refresh_grid.clone();
    let refresh_day_update = refresh_day_view.clone();
    let loaded_month_resp = loaded_month.clone();
    let is_in_flight_resp = is_in_flight.clone();

    // The async channel wakes this task when the worker has finished.
    glib::MainContext::default().spawn_local(async move {
        while let Ok(response) = calendar_worker_rx.recv().await {
            *is_in_flight_resp.borrow_mut() = false;

            *loaded_month_resp.borrow_mut() = Some((response.year, response.month));
            *calendar_cache_update.borrow_mut() = Some(CalendarMonthSnapshot {
                year: response.year,
                month: response.month,
                events: response.events,
            });

            refresh_grid_update();
            refresh_day_update();
        }
    });

    let day_prev_state = current_view_date.clone();
    let day_prev_refresh = refresh_day_view.clone();
    btn_day_prev.connect_clicked(move |_| {
        let current = *day_prev_state.borrow();
        *day_prev_state.borrow_mut() = current - chrono::Duration::days(1);
        day_prev_refresh();
    });

    let day_next_state = current_view_date.clone();
    let day_next_refresh = refresh_day_view.clone();
    btn_day_next.connect_clicked(move |_| {
        let current = *day_next_state.borrow();
        *day_next_state.borrow_mut() = current + chrono::Duration::days(1);
        day_next_refresh();
    });

    let day_refresh_from_month_prev = refresh_day_view.clone();
    btn_prev.connect_clicked(move |_| day_refresh_from_month_prev());

    let day_refresh_from_month_next = refresh_day_view.clone();
    btn_next.connect_clicked(move |_| day_refresh_from_month_next());

    day_view_box.append(&day_nav);
    day_view_box.append(&agenda_list);
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
    btn_power.connect_clicked(move |_| helpers::run_command("systemctl", &["poweroff"]));
    btn_restart.connect_clicked(move |_| helpers::run_command("systemctl", &["reboot"]));

    // Smart Logout: Detects the active session to run the correct exit command
    btn_logout.connect_clicked(move |_| {
        let desktop = std::env::var("XDG_CURRENT_DESKTOP")
            .unwrap_or_default()
            .to_lowercase();
        if desktop.contains("niri") {
            helpers::run_command("niri", &["msg", "action", "quit"]);
        } else if desktop.contains("sway") {
            helpers::run_command("swaymsg", &["exit"]);
        } else if desktop.contains("hyprland") {
            helpers::run_command("hyprctl", &["dispatch", "exit"]);
        } else if let Ok(user) = std::env::var("USER") {
            helpers::run_command("loginctl", &["terminate-user", user.as_str()]);
        }
    });

    // Security
    btn_suspend.connect_clicked(move |_| {
        if !helpers::is_process_running("hyprlock") {
            helpers::run_command("hyprlock", &[]);
            std::thread::sleep(std::time::Duration::from_millis(500));
        }
        helpers::run_command("systemctl", &["suspend"]);
    });
    btn_lock.connect_clicked(move |_| {
        if !helpers::is_process_running("hyprlock") {
            helpers::run_command("hyprlock", &[]);
        }
    });

    // --- Idle Inhibit Persistence ---
    // Query procps-ng for the state of our idle daemons.
    // A suspended process (SIGSTOP) will have a 'T' in its stat column.
    let idle_check = helpers::get_stdout("ps", &["-o", "stat=", "-C", "swayidle,hypridle"]);

    // If the process exists and is stopped, visually mark the button active on boot
    if idle_check.contains('T') {
        btn_idle.add_css_class("active");
    }

    // Toggle logic
    btn_idle.connect_clicked(move |btn| {
        if btn.has_css_class("active") {
            // It's currently paused. Remove UI highlight and resume the daemon.
            btn.remove_css_class("active");
            helpers::run_command("pkill", &["-CONT", "hypridle"]);
            helpers::run_command("pkill", &["-CONT", "swayidle"]);
        } else {
            // It's running normally. Add UI highlight and pause the daemon.
            btn.add_css_class("active");
            helpers::run_command("pkill", &["-STOP", "hypridle"]);
            helpers::run_command("pkill", &["-STOP", "swayidle"]);
        }
    });

    // Launchers
    btn_wall.connect_clicked(move |_| helpers::run_home_bin("wp-select", &[]));
    btn_hint.connect_clicked(move |_| helpers::run_home_bin("kb-launcher", &[]));
    btn_radio.connect_clicked(move |_| helpers::run_home_bin("radio-menu", &[]));

    // Cloudflare DNS Polling Logic
    // Toggling takes time (sudo, network restart). We poll status for 45s to update the badge.
    let btn_dns_poll = btn_dns.clone();
    btn_dns.connect_clicked(move |_| {
        helpers::run_home_bin("cf-toggle", &[]);
        let btn_target = btn_dns_poll.clone();
        let (dns_tx, dns_rx) = unbounded::<Option<Vec<u8>>>();
        std::thread::spawn(move || {
            for _ in 0..45 {
                std::thread::sleep(std::time::Duration::from_secs(1));
                if dns_tx
                    .send_blocking(helpers::get_output_home_bin("cf-status", &[]))
                    .is_err()
                {
                    break;
                }
            }
        });

        glib::MainContext::default().spawn_local(async move {
            while let Ok(status) = dns_rx.recv().await {
                let Some(stdout) = status else {
                    continue;
                };

                if let Ok(json) = serde_json::from_slice::<Value>(&stdout)
                    && let Some(class) = json.get("class").and_then(|v| v.as_str())
                {
                    if class == "on" {
                        btn_target.add_css_class("active");
                    } else {
                        btn_target.remove_css_class("active");
                    }
                }
            }
        });
    });

    // Update Logic
    // Updates run in a terminal (via updater), so we just launch it and hide the badge optimistically.
    let lbl_update_badge_clone = lbl_update_badge.clone();
    btn_update.connect_clicked(move |_| {
        helpers::run_home_bin("updater", &[]);
        lbl_update_badge_clone.set_visible(false);
    });

    // Update Checker (Background Thread)
    // Runs checkupdates/yay every 30 minutes to avoid spamming the CPU/Network.
    let (update_tx, update_rx) = unbounded::<Vec<u8>>();
    std::thread::spawn(move || {
        loop {
            if let Some(stdout) = helpers::get_output_home_bin("update-check", &[])
                && update_tx.send_blocking(stdout).is_err()
            {
                break;
            }
            std::thread::sleep(std::time::Duration::from_secs(1800));
        }
    });

    // Update Checker (GLib async receiver)
    let lbl_update_target = lbl_update_badge.clone();
    glib::MainContext::default().spawn_local(async move {
        while let Ok(stdout) = update_rx.recv().await {
            if let Ok(json) = serde_json::from_slice::<Value>(&stdout)
                && let Some(text) = json.get("text").and_then(|v| v.as_str())
            {
                lbl_update_target.set_label(text);
                lbl_update_target.set_visible(text != "0");
            }
        }
    });

    // Airplane Mode (Optimistic UI)
    let btn_air_clone = btn_air.clone();
    btn_air.connect_clicked(move |_| {
        helpers::run_home_bin("rfkill-manager", &["--toggle"]);
        if btn_air_clone.has_css_class("active") {
            btn_air_clone.remove_css_class("active");
        } else {
            btn_air_clone.add_css_class("active");
        }
    });

    // Audio Mute (Optimistic UI)
    let btn_mute_clone = btn_mute.clone();
    btn_mute.connect_clicked(move |_| {
        helpers::run_command("wpctl", &["set-mute", "@DEFAULT_AUDIO_SINK@", "toggle"]);
        if btn_mute_clone.has_css_class("active") {
            btn_mute_clone.remove_css_class("active");
        } else {
            btn_mute_clone.add_css_class("active");
        }
    });

    // Finance Widget (Background Thread)
    // Runs the external fetcher script and pipes JSON back to the UI.
    click_gesture.connect_pressed(move |_, _, _, _| {
        helpers::run_in_ghostty("waybar-finance", "waybar-finance", &["--tui"]);
    });

    let (sender, receiver) = unbounded::<Option<Vec<u8>>>();
    std::thread::spawn(move || {
        let output = helpers::get_output_home_bin("waybar-finance", &[]);
        let _ = sender.send_blocking(output);
    });

    let finance_label_update = finance_label.clone();
    glib::MainContext::default().spawn_local(async move {
        if let Ok(Some(stdout)) = receiver.recv().await
            && let Ok(json) = serde_json::from_slice::<Value>(&stdout)
            && let Some(text) = json.get("text").and_then(|v| v.as_str())
        {
            // Format the returned spans into rows for the narrow sidebar.
            let raw_items: Vec<&str> = text.split("</span> ").collect();
            let mut grid_text = String::new();
            for (i, item) in raw_items.iter().enumerate() {
                if item.trim().is_empty() {
                    continue;
                }
                grid_text.push_str(item);
                if !item.ends_with("</span>") {
                    grid_text.push_str("</span>");
                }
                if (i + 1) % 3 == 0 {
                    grid_text.push('\n');
                } else {
                    grid_text.push_str("      ");
                }
            }
            finance_label_update.set_markup(&grid_text);
            if let Some(tt) = json.get("tooltip").and_then(|v| v.as_str()) {
                finance_label_update.set_tooltip_markup(Some(tt));
            }
        }
    });

    // Load the initial status without blocking the GTK thread.
    let btn_dns_load = btn_dns.clone();
    let btn_air_load = btn_air.clone();
    let btn_mute_load = btn_mute.clone();
    let scale_bright_load = scale_brightness.clone();
    let scale_vol_load = scale_volume.clone();

    let (status_tx, status_rx) = unbounded();
    std::thread::spawn(move || {
        let dns_o = helpers::get_output_home_bin("cf-status", &[]);
        let air_o = helpers::get_output("rfkill", &["list", "wlan"]);
        let mute_o = helpers::get_output("wpctl", &["get-volume", "@DEFAULT_AUDIO_SINK@"]);
        let bright_o = helpers::get_output("brightnessctl", &["i", "-m"]);
        let _ = status_tx.send_blocking((dns_o, air_o, mute_o, bright_o));
    });

    glib::MainContext::default().spawn_local(async move {
        if let Ok((dns_o, air_o, mute_o, bright_o)) = status_rx.recv().await {
            // Apply DNS State
            if let Some(out) = dns_o
                && let Ok(json) = serde_json::from_slice::<Value>(&out)
                && json.get("class").and_then(|v| v.as_str()) == Some("on")
            {
                btn_dns_load.add_css_class("active");
            }
            // Apply Airplane State
            if let Some(out) = air_o
                && String::from_utf8_lossy(&out).contains("Soft blocked: yes")
            {
                btn_air_load.add_css_class("active");
            }
            // Apply Mute/Volume State
            if let Some(out) = mute_o {
                let s = String::from_utf8_lossy(&out);
                if s.contains("[MUTED]") {
                    btn_mute_load.add_css_class("active");
                }
                if let Some(vol_str) = s.split_whitespace().nth(1)
                    && let Ok(vol) = vol_str.parse::<f64>()
                {
                    scale_vol_load.set_value(vol * 100.0);
                }
            }
            // Apply Brightness State
            if let Some(out) = bright_o
                && let Some(p) = String::from_utf8_lossy(&out).split(',').nth(3)
                && let Ok(val) = p.replace("%", "").replace("\n", "").parse::<f64>()
            {
                scale_bright_load.set_value(val);
            }
        }
    });

    // Refresh slider values changed outside the sidebar, while respecting the
    // interaction guard so a drag is not overwritten.
    let scale_bright_watch = scale_brightness.clone();
    let scale_vol_watch = scale_volume.clone();
    let last_interaction_watch = last_interaction.clone();

    glib::MainContext::default().spawn_local(async move {
        loop {
            glib::timeout_future_seconds(1).await;

            // Guard: If user touched slider < 3 seconds ago, skip external refresh.
            if last_interaction_watch.borrow().elapsed().as_secs() < 3 {
                continue;
            }

            // Run command I/O off-thread and await its result from the GLib loop.
            let (slider_tx, slider_rx) = unbounded::<SliderSnapshot>();
            std::thread::spawn(move || {
                let brightness = helpers::get_output("brightnessctl", &["i", "-m"])
                    .as_deref()
                    .and_then(parse_brightness_pct);
                let volume = helpers::get_output("wpctl", &["get-volume", "@DEFAULT_AUDIO_SINK@"])
                    .as_deref()
                    .and_then(parse_volume_pct);

                let _ = slider_tx.send_blocking(SliderSnapshot { brightness, volume });
            });

            if let Ok(snapshot) = slider_rx.recv().await {
                if let Some(sys_val) = snapshot.brightness
                    && (scale_bright_watch.value() - sys_val).abs() > 1.0
                {
                    scale_bright_watch.set_value(sys_val);
                }
                if let Some(sys_val) = snapshot.volume
                    && (scale_vol_watch.value() - sys_val).abs() > 1.0
                {
                    scale_vol_watch.set_value(sys_val);
                }
            }
        }
    });

    window.present();
}
