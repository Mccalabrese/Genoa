//! Shared helper utilities for sidebar widgets and command execution.

use async_channel::{Receiver, Sender, unbounded};
use chrono::{DateTime, Datelike, Local, NaiveDate, Utc};
use clepsydre_eds::Manager as EdsManager;
use clepsydre_rebind::prelude::*;
use clepsydre_rebind::{Event, Timeframe};
use gtk4::gio::prelude::ListModelExtManual;
use gtk4::prelude::*;
use std::collections::HashSet;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration as StdDuration;
use wait_timeout::ChildExt;

pub struct CalendarRequest {
    pub year: i32,
    pub month: u32,
}

pub struct CalendarResponse {
    pub year: i32,
    pub month: u32,
    pub events: Vec<CalendarEvent>,
}

#[derive(Debug, Clone)]
pub struct CalendarEvent {
    uid: String,
    summary: String,
    start_date: NaiveDate,
    end_date: NaiveDate,
    display_time: String,
    duration_minutes: i64,
    all_day: bool,
    sort_key: i64,
}

#[derive(Debug, Clone)]
pub struct DayAppointment {
    pub uid: String,
    pub summary: String,
    pub time: String,
    pub duration_minutes: i64,
    pub all_day: bool,
}

pub fn spawn_calendar_worker() -> (Sender<CalendarRequest>, Receiver<CalendarResponse>) {
    let (req_tx, req_rx) = unbounded::<CalendarRequest>();
    let (resp_tx, resp_rx) = unbounded::<CalendarResponse>();

    std::thread::spawn(move || {
        let ctx = glib::MainContext::new();
        ctx.with_thread_default(|| {
            let manager = EdsManager::new();

            while let Ok(request) = req_rx.recv_blocking() {
                let events = query_calendar_events_with(&manager, request.year, request.month);
                if resp_tx
                    .send_blocking(CalendarResponse {
                        year: request.year,
                        month: request.month,
                        events,
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .unwrap();
    });

    (req_tx, resp_rx)
}

fn query_calendar_events_with(manager: &EdsManager, year: i32, month: u32) -> Vec<CalendarEvent> {
    let start = first_day_of_month(year, month);
    let end = next_month_first_of(year, month);
    run_calendar_query(manager, start, end)
}
fn run_calendar_query(
    manager: &EdsManager,
    start_date: NaiveDate,
    end_date: NaiveDate,
) -> Vec<CalendarEvent> {
    let tz = glib::TimeZone::local();

    let (Ok(start_dt), Ok(end_dt)) = (
        glib::DateTime::new(
            &tz,
            start_date.year(),
            start_date.month() as i32,
            start_date.day() as i32,
            0,
            0,
            0.0,
        ),
        glib::DateTime::new(
            &tz,
            end_date.year(),
            end_date.month() as i32,
            end_date.day() as i32,
            0,
            0,
            0.0,
        ),
    ) else {
        return Vec::new();
    };

    let Ok(timeframe) = Timeframe::new(false, &start_dt, &end_dt) else {
        log_command_failure(
            "clepsydre_timeframe_failed",
            "clepsydre",
            &[],
            "bad timeframe bounds",
        );

        return Vec::new();
    };
    let subscription = match manager.new_subscription(&timeframe) {
        Ok(Some(sub)) => sub,
        Ok(None) => {
            log_command_failure(
                "clepsydre_subscription_none",
                "clepsydre",
                &[],
                "subscription returned None",
            );

            return Vec::new();
        }

        Err(e) => {
            log_command_failure(
                "clepsydre_subscription_failed",
                "clepsydre",
                &[],
                &e.to_string(),
            );

            return Vec::new();
        }
    };

    let ctx = glib::MainContext::thread_default().unwrap();

    // Give the data source time to publish its initial events. An empty
    // subscription may still be loading, so only debounce after data arrives.
    let mut last_count = subscription.n_items();
    let mut last_change = std::time::Instant::now();
    let deadline = last_change + std::time::Duration::from_millis(1500);
    let debounce = std::time::Duration::from_millis(150);
    let mut has_published_data = last_count > 0;

    loop {
        while ctx.iteration(false) {}

        let count = subscription.n_items();
        if count != last_count {
            last_count = count;
            last_change = std::time::Instant::now();
            has_published_data = true;
        }

        if (has_published_data && last_change.elapsed() >= debounce)
            || std::time::Instant::now() >= deadline
        {
            break;
        }

        // Avoid busy-spinning while the data source settles.
        std::thread::sleep(std::time::Duration::from_millis(20));
    }

    subscription
        .iter::<Event>()
        .filter_map(Result::ok)
        .filter_map(event_to_calendar_event)
        .collect()
}

fn event_to_calendar_event(event: Event) -> Option<CalendarEvent> {
    let tf = event.timeframe()?;
    let start_unix = tf.start_unix();
    let end_unix = tf.end_unix();
    let all_day = tf.is_all_day();

    /* Dummy timestamp at 00:00:00 UTC to prevent offseting with local time for all day events */
    let (start_date_reform, end_date_reform, display_time_reform) = if all_day {
        let start = DateTime::from_timestamp(start_unix, 0)?.with_timezone(&Utc);
        let end = DateTime::from_timestamp(end_unix, 0)?.with_timezone(&Utc);

        (start.date_naive(), end.date_naive(), "All day".to_string())
    } else {
        let start = DateTime::from_timestamp(start_unix, 0)?.with_timezone(&Local);
        let end = DateTime::from_timestamp(end_unix, 0)?.with_timezone(&Local);

        (
            start.date_naive(),
            end.date_naive(),
            start.format("%H:%M").to_string(),
        )
    };

    Some(CalendarEvent {
        uid: event.uri().map(|s| s.to_string()).unwrap_or_default(),
        summary: event.name().map(|s| s.to_string()).unwrap_or_default(),
        start_date: start_date_reform,
        end_date: end_date_reform,
        display_time: display_time_reform,
        duration_minutes: ((end_unix - start_unix) / 60).max(0),
        all_day,
        sort_key: start_unix,
    })
}

fn occurs_on(event: &CalendarEvent, target_date: NaiveDate) -> bool {
    if event.all_day {
        event.start_date <= target_date && target_date < event.end_date
    } else {
        event.start_date <= target_date && target_date <= event.end_date
    }
}

pub fn get_day_appointments_from_events(
    date: NaiveDate,
    events: &[CalendarEvent],
) -> Vec<DayAppointment> {
    let mut matches: Vec<CalendarEvent> = events
        .iter()
        .filter(|event| occurs_on(event, date))
        .cloned()
        .collect();

    matches.sort_by_key(|event| event.sort_key);

    matches
        .into_iter()
        .map(|event| DayAppointment {
            uid: event.uid,
            summary: event.summary,
            time: event.display_time,
            duration_minutes: event.duration_minutes,
            all_day: event.all_day,
        })
        .collect()
}

fn get_month_days_with_appointments_from_events(
    year: i32,
    month: u32,
    events: &[CalendarEvent],
) -> HashSet<u32> {
    let mut days = HashSet::new();

    let Some(first_day) = NaiveDate::from_ymd_opt(year, month, 1) else {
        return days;
    };

    let (next_year, next_month) = if month == 12 {
        (year + 1, 1)
    } else {
        (year, month + 1)
    };

    let Some(next_first) = NaiveDate::from_ymd_opt(next_year, next_month, 1) else {
        return days;
    };

    let days_in_month = next_first.signed_duration_since(first_day).num_days() as u32;

    for day_num in 1..=days_in_month {
        if let Some(date) = NaiveDate::from_ymd_opt(year, month, day_num)
            && events.iter().any(|event| occurs_on(event, date))
        {
            days.insert(day_num);
        }
    }

    days
}

fn first_day_of_month(year: i32, month: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(year, month, 1).unwrap_or_else(|| Local::now().date_naive())
}

fn next_month_first_of(year: i32, month: u32) -> NaiveDate {
    let (next_year, next_month) = if month == 12 {
        (year + 1, 1)
    } else {
        (year, month + 1)
    };
    NaiveDate::from_ymd_opt(next_year, next_month, 1).unwrap_or_else(|| Local::now().date_naive())
}

// --- Button factories ---

/// Creates a small square button for session controls.
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

/// Creates a larger circular button for feature toggles.
pub fn make_icon_button(icon_name: &str, tooltip: &str) -> gtk4::Button {
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
/// Creates a circular button with a notification badge.
pub fn make_badged_button(
    icon_name: &str,
    count: &str,
    tooltip: &str,
) -> (gtk4::Button, gtk4::Label) {
    let icon = gtk4::Image::builder()
        .icon_name(icon_name)
        .pixel_size(24)
        .build();

    let badge = gtk4::Label::builder()
        .label(count)
        .css_classes(vec!["badge".to_string()])
        .halign(gtk4::Align::End) // Align to Top-Right corner
        .valign(gtk4::Align::Start)
        .visible(count != "0") // Auto-hide if count is zero
        .build();

    let overlay = gtk4::Overlay::builder().child(&icon).build();
    overlay.add_overlay(&badge);

    let btn = gtk4::Button::builder()
        .child(&overlay)
        .css_classes(vec!["circular-btn".to_string()])
        .height_request(30)
        .tooltip_text(tooltip)
        .build();
    (btn, badge)
}

// --- Calendar rendering ---

pub fn build_calendar_grid_from_events(
    year: i32,
    month: u32,
    events: &[CalendarEvent],
) -> gtk4::Grid {
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

    let days = ["Su", "Mo", "Tu", "We", "Th", "Fr", "Sa"];
    for (i, day) in days.iter().enumerate() {
        let label = gtk4::Label::builder()
            .label(*day)
            .css_classes(vec!["calendar-header".to_string()])
            .hexpand(true)
            .build();
        grid.attach(&label, i as i32, 0, 1, 1);
    }

    let Some(first_day) = NaiveDate::from_ymd_opt(year, month, 1) else {
        return grid;
    };

    let start_offset = first_day.weekday().num_days_from_sunday();

    let next_month = if month == 12 { 1 } else { month + 1 };
    let next_year = if month == 12 { year + 1 } else { year };
    let Some(next_first) = NaiveDate::from_ymd_opt(next_year, next_month, 1) else {
        return grid;
    };
    let days_in_month = next_first.signed_duration_since(first_day).num_days();
    let appointment_days = get_month_days_with_appointments_from_events(year, month, events);

    let mut col = start_offset as i32;
    let mut row = 1;

    let today = Local::now().date_naive();

    for day_num in 1..=days_in_month {
        let vbox = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
        vbox.set_valign(gtk4::Align::Center);

        let num_label = gtk4::Label::builder()
            .label(day_num.to_string())
            .css_classes(vec!["calendar-day-num".to_string()])
            .build();

        let has_appointment = appointment_days.contains(&(day_num as u32));

        let dot_label = gtk4::Label::builder()
            .label("•")
            .css_classes(vec!["calendar-dot".to_string()])
            .visible(has_appointment)
            .build();

        vbox.append(&num_label);
        vbox.append(&dot_label);

        let mut btn_classes = vec!["calendar-day-btn".to_string()];

        if today.year() == year && today.month() == month && today.day() == day_num as u32 {
            btn_classes.push("today".to_string());
        }
        let btn = gtk4::Button::builder()
            .child(&vbox)
            .css_classes(btn_classes)
            .hexpand(true)
            .vexpand(true)
            .valign(gtk4::Align::Fill)
            .build();

        btn.connect_clicked(move |_| {
            let date_arg = format!("{:4}-{:02}-{:02}", year, month, day_num);
            run_command("gnome-calendar", &["--date", date_arg.as_str()]);
        });

        grid.attach(&btn, col, row, 1, 1);

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
    icon.add_css_class("slider-icon");

    let scale = gtk4::Scale::with_range(gtk4::Orientation::Horizontal, 0.0, 100.0, 1.0);
    scale.add_css_class("sidebar-slider");
    scale.set_hexpand(true);
    scale.set_draw_value(false); // Hide the number (we use visual feedback)

    box_row.append(&icon);
    box_row.append(&scale);

    (box_row, scale)
}

// --- System Utilities ---

fn cargo_bin_path(bin_name: &str) -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join(".cargo/bin").join(bin_name))
}

fn resolve_program(program: &str) -> String {
    if program.contains('/') {
        return program.to_string();
    }

    if let Some(path_var) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path_var) {
            let candidate = dir.join(program);
            if candidate.is_file() {
                return candidate.to_string_lossy().to_string();
            }
        }
    }

    for dir in ["/usr/bin", "/bin", "/usr/sbin", "/sbin"] {
        let candidate = Path::new(dir).join(program);
        if candidate.is_file() {
            return candidate.to_string_lossy().to_string();
        }
    }

    program.to_string()
}

// Shared command policy for external tools invoked by the sidebar.
const CMD_TIMEOUT_MS: u64 = 5000;
const CMD_RETRIES: usize = 2;
const RETRY_BACKOFF_MS: u64 = 120;

fn telemetry_path() -> PathBuf {
    if let Some(runtime) = std::env::var_os("XDG_RUNTIME_DIR") {
        return PathBuf::from(runtime).join("sidebar-telemetry.log");
    }
    PathBuf::from("/tmp/sidebar-telemetry.log")
}

pub fn log_command_failure(kind: &str, program: &str, args: &[&str], detail: &str) {
    let ts = Local::now().to_rfc3339();
    let arg_str = if args.is_empty() {
        "".to_string()
    } else {
        args.join(" ")
    };

    let line = format!("{} | {} | {} {} | {}\n", ts, kind, program, arg_str, detail);

    if let Ok(mut file) = OpenOptions::new()
        .create(true)
        .append(true)
        .open(telemetry_path())
    {
        let _ = file.write_all(line.as_bytes());
    }
}

fn run_output_with_retry_with_timeout(
    program: &str,
    args: &[&str],
    timeout_ms: u64,
) -> Option<std::process::Output> {
    let timeout = StdDuration::from_millis(timeout_ms);
    let resolved_program = resolve_program(program);

    for attempt in 1..=(CMD_RETRIES + 1) {
        let mut child = match Command::new(&resolved_program)
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
        {
            Ok(child) => child,
            Err(e) => {
                log_command_failure(
                    "spawn_failed",
                    &resolved_program,
                    args,
                    &format!("attempt={} error={}", attempt, e),
                );
                if attempt <= CMD_RETRIES {
                    std::thread::sleep(StdDuration::from_millis(RETRY_BACKOFF_MS * attempt as u64));
                    continue;
                }
                return None;
            }
        };

        // wait_timeout prevents command hangs from stalling call sites indefinitely.
        match child.wait_timeout(timeout) {
            Ok(Some(_)) => match child.wait_with_output() {
                Ok(output) if output.status.success() => return Some(output),
                Ok(output) => {
                    let stderr = String::from_utf8_lossy(&output.stderr).replace('\n', " ");
                    log_command_failure(
                        "non_zero_exit",
                        &resolved_program,
                        args,
                        &format!(
                            "attempt={} status={:?} stderr={}",
                            attempt,
                            output.status.code(),
                            stderr
                        ),
                    );
                    return None;
                }
                Err(e) => {
                    log_command_failure(
                        "wait_output_failed",
                        &resolved_program,
                        args,
                        &format!("attempt={} error={}", attempt, e),
                    );
                }
            },
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                log_command_failure(
                    "timeout",
                    &resolved_program,
                    args,
                    &format!("attempt={} timeout_ms={}", attempt, timeout_ms),
                );
            }
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                log_command_failure(
                    "wait_timeout_failed",
                    &resolved_program,
                    args,
                    &format!("attempt={} error={}", attempt, e),
                );
            }
        }

        if attempt <= CMD_RETRIES {
            std::thread::sleep(StdDuration::from_millis(RETRY_BACKOFF_MS * attempt as u64));
        }
    }

    None
}

fn run_output_with_retry(program: &str, args: &[&str]) -> Option<std::process::Output> {
    run_output_with_retry_with_timeout(program, args, CMD_TIMEOUT_MS)
}

pub fn run_command(program: &str, args: &[&str]) {
    let resolved = resolve_program(program);
    if let Err(e) = Command::new(&resolved).args(args).spawn() {
        log_command_failure("spawn_failed", &resolved, args, &e.to_string());
    }
}

pub fn run_home_bin(bin_name: &str, args: &[&str]) {
    if let Some(path) = cargo_bin_path(bin_name) {
        if let Err(e) = Command::new(&path).args(args).spawn() {
            log_command_failure(
                "spawn_failed",
                &path.display().to_string(),
                args,
                &e.to_string(),
            );
        }
    } else {
        log_command_failure("missing_bin", bin_name, args, "not found in ~/.cargo/bin");
    }
}

pub fn run_in_ghostty(title: &str, bin_name: &str, args: &[&str]) {
    let Some(path) = cargo_bin_path(bin_name) else {
        log_command_failure("missing_bin", bin_name, args, "not found in ~/.cargo/bin");
        return;
    };

    let mut cmd = Command::new("ghostty");
    cmd.arg(format!("--title={}", title)).arg("-e").arg(path);
    for arg in args {
        cmd.arg(arg);
    }
    if let Err(e) = cmd.spawn() {
        log_command_failure("spawn_failed", "ghostty", args, &e.to_string());
    }
}

pub fn get_output(program: &str, args: &[&str]) -> Option<Vec<u8>> {
    run_output_with_retry(program, args).map(|out| out.stdout)
}

pub fn get_output_home_bin(bin_name: &str, args: &[&str]) -> Option<Vec<u8>> {
    let path = cargo_bin_path(bin_name)?;
    let program = path.display().to_string();
    run_output_with_retry(&program, args).map(|out| out.stdout)
}

pub fn get_stdout(program: &str, args: &[&str]) -> String {
    match run_output_with_retry(program, args) {
        Some(o) => String::from_utf8_lossy(&o.stdout).trim().to_string(),
        None => "N/A".to_string(),
    }
}

pub fn pkg_count() -> String {
    match run_output_with_retry("pacman", &["-Q"]) {
        Some(o) => String::from_utf8_lossy(&o.stdout)
            .lines()
            .count()
            .to_string(),
        _ => "N/A".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn calendar_event(start_date: &str, end_date: &str, all_day: bool) -> CalendarEvent {
        CalendarEvent {
            uid: "uid-1".to_string(),
            summary: "Test".to_string(),
            start_date: NaiveDate::parse_from_str(start_date, "%Y-%m-%d").unwrap(),
            end_date: NaiveDate::parse_from_str(end_date, "%Y-%m-%d").unwrap(),
            display_time: "09:00".to_string(),
            duration_minutes: 30,
            all_day,
            sort_key: 0,
        }
    }

    #[test]
    fn timed_event_matches_same_day() {
        let date = NaiveDate::from_ymd_opt(2026, 8, 5).unwrap();
        assert!(occurs_on(
            &calendar_event("2026-08-05", "2026-08-05", false),
            date
        ));
    }

    #[test]
    fn all_day_event_keeps_exclusive_end() {
        let start_date = NaiveDate::from_ymd_opt(2026, 8, 5).unwrap();
        let end_date = NaiveDate::from_ymd_opt(2026, 8, 6).unwrap();
        assert!(occurs_on(
            &calendar_event("2026-08-05", "2026-08-06", true),
            start_date
        ));
        assert!(!occurs_on(
            &calendar_event("2026-08-05", "2026-08-06", true),
            end_date
        ));
    }
}
