//! Shared helper utilities for sidebar widgets and command execution.

use async_channel::{Receiver, Sender, unbounded};
use chrono::{DateTime, Datelike, Local, NaiveDate, Utc};
use clepsydre_eds::Manager as EdsManager;
use clepsydre_rebind::prelude::*;
use clepsydre_rebind::Event;
use gtk4::gio::prelude::ListModelExtManual;
use gtk4::prelude::*;
use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::os::fd::AsRawFd;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
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

    let subscription = match manager.new_subscription(&start_dt, &end_dt) {
        Ok(sub) => sub,
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

fn resolve_program(program: &str) -> Option<PathBuf> {
    if program.contains('/') {
        let path = PathBuf::from(program);
        return is_executable(&path).then_some(path);
    }

    for dir in ["/usr/bin", "/bin", "/usr/sbin", "/sbin"] {
        let candidate = Path::new(dir).join(program);
        if is_executable(&candidate) {
            return Some(candidate);
        }
    }

    None
}

fn is_executable(path: &Path) -> bool {
    fs::metadata(path)
        .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
}

// Shared command policy for external tools invoked by the sidebar.
const CMD_TIMEOUT_MS: u64 = 5000;
const CMD_RETRIES: usize = 2;
const RETRY_BACKOFF_MS: u64 = 120;
const TELEMETRY_FILE_NAME: &str = "sidebar-telemetry.log";
const TELEMETRY_ROTATED_FILE_NAME: &str = "sidebar-telemetry.log.1";
const TELEMETRY_MAX_BYTES: u64 = 64 * 1024;
const TELEMETRY_DETAIL_MAX_BYTES: usize = 512;
const GTKLOCK: &str = "/usr/bin/gtklock";
const GTKLOCK_SUSPEND_COMMAND: &str = "/usr/bin/systemctl suspend";
const LOCK_EXCLUSIVE: std::ffi::c_int = 2;
const LOCK_NONBLOCKING: std::ffi::c_int = 4;

unsafe extern "C" {
    fn flock(fd: std::ffi::c_int, operation: std::ffi::c_int) -> std::ffi::c_int;
    fn geteuid() -> u32;
}

fn telemetry_path() -> Result<PathBuf, String> {
    Ok(genoa_runtime_directory()?.join(TELEMETRY_FILE_NAME))
}

pub fn log_command_failure(kind: &str, program: &str, args: &[&str], detail: &str) {
    let Ok(path) = telemetry_path() else {
        // Never fall back to /tmp: a missing per-user runtime directory means
        // there is no safe place to preserve diagnostic data.
        return;
    };
    let program = Path::new(program)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("unknown");
    let line = telemetry_line(kind, program, args.len(), detail);
    let Ok(mut file) = open_telemetry_file(&path, line.len() as u64) else {
        return;
    };
    let _ = file.write_all(line.as_bytes());
}

fn telemetry_line(kind: &str, program: &str, argument_count: usize, detail: &str) -> String {
    format!(
        "{} | {} | {} | args={} | {}\n",
        Local::now().to_rfc3339(),
        telemetry_text(kind),
        telemetry_text(program),
        argument_count,
        telemetry_text(detail),
    )
}

fn open_telemetry_file(path: &Path, next_entry_bytes: u64) -> Result<File, std::io::Error> {
    rotate_telemetry_if_needed(path, next_entry_bytes)?;
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)?;

    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.uid() != unsafe { geteuid() } {
        return Err(std::io::Error::other(
            "telemetry file is not a private regular file",
        ));
    }
    if metadata.permissions().mode() & 0o077 != 0 {
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
    }
    Ok(file)
}

fn rotate_telemetry_if_needed(path: &Path, next_entry_bytes: u64) -> Result<(), std::io::Error> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(std::io::Error::other(
            "telemetry path is not a regular file",
        ));
    }
    if metadata.len().saturating_add(next_entry_bytes) > TELEMETRY_MAX_BYTES {
        fs::rename(path, path.with_file_name(TELEMETRY_ROTATED_FILE_NAME))?;
    }
    Ok(())
}

fn telemetry_text(value: &str) -> String {
    let mut output = String::new();
    for character in value.chars() {
        let character = if character.is_control() {
            ' '
        } else {
            character
        };
        if output.len() + character.len_utf8() > TELEMETRY_DETAIL_MAX_BYTES - 3 {
            output.push('…');
            break;
        }
        output.push(character);
    }
    output
}

fn run_output_with_retry_with_timeout(
    program: &str,
    args: &[&str],
    timeout_ms: u64,
) -> Option<std::process::Output> {
    let timeout = StdDuration::from_millis(timeout_ms);
    let Some(resolved_program) = resolve_program(program) else {
        log_command_failure(
            "missing_system_binary",
            program,
            args,
            "not in trusted system paths",
        );
        return None;
    };
    let resolved_program_label = resolved_program.to_string_lossy();

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
                    &resolved_program_label,
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
                        &resolved_program_label,
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
                        &resolved_program_label,
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
                    &resolved_program_label,
                    args,
                    &format!("attempt={} timeout_ms={}", attempt, timeout_ms),
                );
            }
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                log_command_failure(
                    "wait_timeout_failed",
                    &resolved_program_label,
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
    let Some(resolved) = resolve_program(program) else {
        log_command_failure(
            "missing_system_binary",
            program,
            args,
            "not in trusted system paths",
        );
        return;
    };
    if let Err(e) = Command::new(&resolved).args(args).spawn() {
        log_command_failure(
            "spawn_failed",
            &resolved.display().to_string(),
            args,
            &e.to_string(),
        );
    }
}

/// Starts gtklock while holding a lock that belongs to this login session.
/// For suspend, gtklock itself invokes the fixed systemctl command only after
/// the Wayland session lock is active; a failed or not-yet-ready lock can
/// therefore never be followed by suspend.
pub fn lock_screen(suspend_after_lock: bool) {
    std::thread::spawn(move || {
        if let Err(error) = lock_screen_inner(suspend_after_lock) {
            log_command_failure(
                "lock_screen_failed",
                GTKLOCK,
                if suspend_after_lock {
                    &["--lock-command", GTKLOCK_SUSPEND_COMMAND]
                } else {
                    &[]
                },
                &error,
            );
        }
    });
}

fn lock_screen_inner(suspend_after_lock: bool) -> Result<(), String> {
    let lock_path = gtklock_runtime_lock_path()?;
    let lock_file = open_runtime_lock(&lock_path)?;
    try_lock_exclusive(&lock_file)
        .map_err(|error| format!("another lock request is already running: {error}"))?;

    let status = Command::new(GTKLOCK)
        .args(gtklock_args(suspend_after_lock))
        .status()
        .map_err(|error| format!("could not start gtklock: {error}"))?;
    if !status.success() {
        return Err(format!(
            "gtklock exited before it could lock the session: {status}"
        ));
    }
    Ok(())
}

fn open_runtime_lock(lock_path: &Path) -> Result<File, String> {
    OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .mode(0o600)
        .open(lock_path)
        .map_err(|error| format!("could not open {}: {error}", lock_path.display()))
}

fn gtklock_runtime_lock_path() -> Result<PathBuf, String> {
    Ok(genoa_runtime_directory()?.join("gtklock.lock"))
}

fn genoa_runtime_directory() -> Result<PathBuf, String> {
    let runtime = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .ok_or_else(|| {
            "XDG_RUNTIME_DIR is unavailable; refusing to use /tmp for lock state".to_string()
        })?;
    genoa_runtime_directory_in(&runtime)
}

#[cfg(test)]
fn gtklock_runtime_lock_path_in(runtime: &Path) -> Result<PathBuf, String> {
    Ok(genoa_runtime_directory_in(runtime)?.join("gtklock.lock"))
}

fn genoa_runtime_directory_in(runtime: &Path) -> Result<PathBuf, String> {
    ensure_private_runtime_directory(runtime)?;

    let genoa_runtime = runtime.join("genoa");
    let mut builder = fs::DirBuilder::new();
    builder.mode(0o700);
    match builder.create(&genoa_runtime) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => {
            return Err(format!(
                "could not create {}: {error}",
                genoa_runtime.display()
            ));
        }
    }
    ensure_private_runtime_directory(&genoa_runtime)?;
    Ok(genoa_runtime)
}

fn ensure_private_runtime_directory(path: &Path) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("could not inspect {}: {error}", path.display()))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(format!("{} is not a real directory", path.display()));
    }
    if metadata.uid() != unsafe { geteuid() } {
        return Err(format!(
            "{} is not owned by the active user",
            path.display()
        ));
    }
    if metadata.permissions().mode() & 0o077 != 0 {
        return Err(format!("{} is accessible by another user", path.display()));
    }
    Ok(())
}

fn try_lock_exclusive(file: &File) -> std::io::Result<()> {
    // SAFETY: flock operates only on this valid, open file descriptor. The
    // operation is non-blocking, so the sidebar UI can never hang waiting for
    // another concurrent lock request.
    let result = unsafe { flock(file.as_raw_fd(), LOCK_EXCLUSIVE | LOCK_NONBLOCKING) };
    if result == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

fn gtklock_args(suspend_after_lock: bool) -> Vec<&'static str> {
    let mut args = Vec::new();
    if suspend_after_lock {
        args.extend(["--lock-command", GTKLOCK_SUSPEND_COMMAND]);
    }
    args
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

    let Some(ghostty) = resolve_program("ghostty") else {
        log_command_failure(
            "missing_system_binary",
            "ghostty",
            args,
            "not in trusted system paths",
        );
        return;
    };

    let mut cmd = Command::new(ghostty);
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

    #[test]
    fn suspend_is_a_fixed_post_lock_command() {
        assert_eq!(gtklock_args(false), Vec::<&str>::new());
        assert_eq!(
            gtklock_args(true),
            ["--lock-command", "/usr/bin/systemctl suspend"]
        );
    }

    #[test]
    fn runtime_lock_requires_a_private_user_directory() {
        let unique = format!(
            "genoa-sidebar-runtime-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let runtime = std::env::temp_dir().join(unique);
        fs::create_dir(&runtime).unwrap();
        fs::set_permissions(&runtime, fs::Permissions::from_mode(0o700)).unwrap();

        let lock_path = gtklock_runtime_lock_path_in(&runtime).unwrap();
        assert_eq!(lock_path, runtime.join("genoa/gtklock.lock"));
        assert_eq!(
            fs::metadata(runtime.join("genoa"))
                .unwrap()
                .permissions()
                .mode()
                & 0o077,
            0
        );
        let lock = open_runtime_lock(&lock_path).unwrap();
        assert_eq!(lock.metadata().unwrap().permissions().mode() & 0o077, 0);

        fs::set_permissions(&runtime, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(gtklock_runtime_lock_path_in(&runtime).is_err());

        fs::set_permissions(&runtime, fs::Permissions::from_mode(0o700)).unwrap();
        fs::remove_dir_all(runtime).unwrap();
    }

    #[test]
    fn telemetry_omits_arguments_and_bounds_error_details() {
        let secret = "api-token-that-must-not-be-recorded";
        let line = telemetry_line("command_failed", "systemctl", 2, &"x".repeat(2048));

        assert!(line.contains("args=2"));
        assert!(!line.contains(secret));
        assert!(line.len() < 700);
        assert!(telemetry_text("first\nsecond\0third").contains("first second third"));
    }

    #[test]
    fn telemetry_rotation_and_opening_remain_private() {
        let unique = format!(
            "genoa-sidebar-telemetry-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let runtime = std::env::temp_dir().join(unique);
        fs::create_dir(&runtime).unwrap();
        fs::set_permissions(&runtime, fs::Permissions::from_mode(0o700)).unwrap();

        let telemetry_path = genoa_runtime_directory_in(&runtime)
            .unwrap()
            .join(TELEMETRY_FILE_NAME);
        let telemetry_file = File::create(&telemetry_path).unwrap();
        telemetry_file.set_len(TELEMETRY_MAX_BYTES).unwrap();

        rotate_telemetry_if_needed(&telemetry_path, 1).unwrap();
        assert!(!telemetry_path.exists());
        assert!(
            telemetry_path
                .with_file_name(TELEMETRY_ROTATED_FILE_NAME)
                .exists()
        );

        let new_file = open_telemetry_file(&telemetry_path, 1).unwrap();
        assert_eq!(new_file.metadata().unwrap().permissions().mode() & 0o077, 0);

        fs::remove_dir_all(runtime).unwrap();
    }

    #[test]
    fn telemetry_refuses_symlinks() {
        let unique = format!(
            "genoa-sidebar-telemetry-symlink-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let runtime = std::env::temp_dir().join(unique);
        fs::create_dir(&runtime).unwrap();
        fs::set_permissions(&runtime, fs::Permissions::from_mode(0o700)).unwrap();

        let telemetry_path = genoa_runtime_directory_in(&runtime)
            .unwrap()
            .join(TELEMETRY_FILE_NAME);
        std::os::unix::fs::symlink("/dev/null", &telemetry_path).unwrap();
        assert!(open_telemetry_file(&telemetry_path, 1).is_err());

        fs::remove_dir_all(runtime).unwrap();
    }
}
