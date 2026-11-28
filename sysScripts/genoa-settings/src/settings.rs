use std::{cell::RefCell, env, path::Path, process::Command, rc::Rc, thread};

use genoa_settings::idle::{IdleProfile, IdleSettings};
use genoa_settings::wallpaper::{
    Wallpaper, configured_lock_wallpaper, load_wallpapers, save_lock_wallpaper, thumbnail_bytes,
    validate_wallpaper,
};
use gtk4::prelude::*;
use gtk4::{
    Align, Application, ApplicationWindow, Box as GtkBox, Button, DropDown, FlowBox, Label,
    Orientation, Picture, ScrolledWindow, SearchEntry, Stack, StackSidebar, StringList,
    StringObject, Switch, gdk, glib,
};

const MAX_VISIBLE_WALLPAPERS: usize = 72;
const TIMEOUT_MINUTES: &[u32] = &[
    1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 20, 25, 30, 45, 60, 90, 120, 180, 240,
];

pub fn build(application: &Application) {
    if let Some(settings) = gtk4::Settings::default() {
        settings.set_gtk_icon_theme_name(Some("Adwaita"));
        settings.set_gtk_application_prefer_dark_theme(true);
    }

    let pages = Stack::builder()
        .hexpand(true)
        .vexpand(true)
        .transition_type(gtk4::StackTransitionType::SlideLeftRight)
        .build();
    pages.add_titled(&overview_page(), Some("overview"), "Overview");
    pages.add_titled(&wallpaper_page(), Some("wallpaper"), "Wallpaper");
    let (idle_settings, load_warning) =
        IdleSettings::load_or_default(active_idle_profile().defaults());
    pages.add_titled(
        &lock_and_sleep_page(&idle_settings, load_warning.as_deref()),
        Some("lock-and-sleep"),
        "Lock and sleep",
    );
    pages.add_titled(
        &planned_page(
            "Optional packages",
            "Optional packages from Genoa's package list will be selectable here.",
            "Package installation and yay management are not connected yet.",
        ),
        Some("packages"),
        "Optional packages",
    );

    let navigation = StackSidebar::builder().stack(&pages).build();
    navigation.set_width_request(190);

    let root = GtkBox::builder()
        .orientation(Orientation::Horizontal)
        .spacing(24)
        .margin_top(24)
        .margin_bottom(24)
        .margin_start(24)
        .margin_end(24)
        .build();
    root.append(&navigation);
    root.append(&pages);

    let window = ApplicationWindow::builder()
        .application(application)
        .title("Genoa Settings")
        .default_width(960)
        .default_height(640)
        .child(&root)
        .build();
    window.present();
}

fn overview_page() -> GtkBox {
    let page = page_container();
    page.append(&heading(
        "Genoa Settings",
        "A home for Genoa's user-facing controls.",
    ));
    page.append(&section(
        "Foundation ready",
        "This standalone GTK4 application manages the lock and sleep policy used by Genoa's Niri and Sway sessions.",
    ));
    page.append(&section(
        "Incremental rollout",
        "Wallpaper and lock-and-sleep controls are ready. Optional packages will be added as a separate, reviewable change.",
    ));
    page
}

fn active_idle_profile() -> IdleProfile {
    if env::var_os("SWAYSOCK").is_some() {
        IdleProfile::Sway
    } else {
        IdleProfile::Niri
    }
}

fn planned_page(title: &str, description: &str, status: &str) -> GtkBox {
    let page = page_container();
    page.append(&heading(title, description));
    page.append(&section("Not connected yet", status));
    page
}

fn wallpaper_page() -> GtkBox {
    let page = page_container();
    page.append(&heading(
        "Wallpaper",
        "Choose an indexed image for the active desktop session or Genoa Lock.",
    ));

    let status = Label::builder().xalign(0.0).wrap(true).build();
    let selected_label = Label::builder()
        .xalign(0.0)
        .wrap(true)
        .label("Select an image below.")
        .build();
    let selected = Rc::new(RefCell::new(None::<Wallpaper>));
    let gallery = FlowBox::builder()
        .max_children_per_line(4)
        .homogeneous(true)
        .selection_mode(gtk4::SelectionMode::None)
        .row_spacing(10)
        .column_spacing(10)
        .build();
    let scroll = ScrolledWindow::builder()
        .vexpand(true)
        .min_content_height(340)
        .child(&gallery)
        .build();
    let search = SearchEntry::builder()
        .placeholder_text("Search indexed wallpapers")
        .build();

    match load_wallpapers() {
        Ok(wallpapers) if wallpapers.is_empty() => {
            status.set_label("No usable wallpapers were found in the index.");
        }
        Ok(wallpapers) => {
            populate_wallpaper_gallery(&gallery, &wallpapers, "", &selected, &selected_label);
            let selected = selected.clone();
            let selected_label = selected_label.clone();
            search.connect_search_changed(move |entry| {
                populate_wallpaper_gallery(
                    &gallery,
                    &wallpapers,
                    entry.text().as_str(),
                    &selected,
                    &selected_label,
                );
            });
        }
        Err(error) => status.set_label(&format!(
            "Wallpaper thumbnails are unavailable: {error}. The wallpaper indexer will create them after it starts."
        )),
    }

    let (display_row, button_row) = wallpaper_actions(&selected, &status);
    page.append(&section(
        "Current lock background",
        &configured_lock_wallpaper().map_or_else(
            || "Genoa Lock is using its built-in background.".to_string(),
            |path| format!("{}", path.display()),
        ),
    ));
    page.append(&search);
    page.append(&scroll);
    page.append(&selected_label);
    page.append(&display_row);
    page.append(&button_row);
    page.append(&status);
    page
}

fn wallpaper_actions(
    selected: &Rc<RefCell<Option<Wallpaper>>>,
    status: &Label,
) -> (GtkBox, GtkBox) {
    let output_selector = active_output_selector(status);
    let apply_session = Button::with_label("Apply to selected display");
    apply_session.set_halign(Align::Start);
    let selected_for_session = selected.clone();
    let status_for_session = status.clone();
    let output_for_session = output_selector.clone();
    apply_session.connect_clicked(move |_| {
        let Some(wallpaper) = selected_for_session.borrow().clone() else {
            status_for_session.set_label("Select a wallpaper first.");
            return;
        };
        let Some(output) = selected_output_name(&output_for_session) else {
            status_for_session.set_label("Choose an active display first.");
            return;
        };
        match apply_session_wallpaper(&wallpaper.path, &output) {
            Ok(()) => status_for_session.set_label("Wallpaper applied to the selected display."),
            Err(error) => status_for_session.set_label(&error),
        }
    });

    let set_lock = Button::with_label("Use for Genoa Lock");
    set_lock.add_css_class("suggested-action");
    set_lock.set_halign(Align::Start);
    let selected_for_lock = selected.clone();
    let status_for_lock = status.clone();
    set_lock.connect_clicked(move |_| {
        let Some(wallpaper) = selected_for_lock.borrow().clone() else {
            status_for_lock.set_label("Select a wallpaper first.");
            return;
        };
        match save_lock_wallpaper(&wallpaper.path) {
            Ok(()) => status_for_lock
                .set_label("Saved. Genoa Lock will use this wallpaper the next time it starts."),
            Err(error) => status_for_lock.set_label(&error),
        }
    });

    let display_row = GtkBox::builder()
        .orientation(Orientation::Horizontal)
        .spacing(12)
        .build();
    display_row.append(&Label::new(Some("Display")));
    display_row.append(&output_selector);
    let button_row = GtkBox::builder()
        .orientation(Orientation::Horizontal)
        .spacing(12)
        .build();
    button_row.append(&apply_session);
    button_row.append(&set_lock);
    (display_row, button_row)
}

fn active_output_selector(status: &Label) -> DropDown {
    let outputs = StringList::new(&[]);
    match active_output_names() {
        Ok(names) if names.is_empty() => {
            status.set_label("No active displays were reported by the compositor.");
        }
        Ok(names) => {
            for name in names {
                outputs.append(&name);
            }
        }
        Err(error) => status.set_label(&error),
    }
    let selector = DropDown::new(Some(outputs.clone()), None::<gtk4::Expression>);
    if outputs.n_items() > 0 {
        selector.set_selected(0);
    }
    selector
}

fn selected_output_name(selector: &DropDown) -> Option<String> {
    selector
        .selected_item()
        .and_downcast::<StringObject>()
        .map(|output| output.string().to_string())
}

fn populate_wallpaper_gallery(
    gallery: &FlowBox,
    wallpapers: &[Wallpaper],
    query: &str,
    selected: &Rc<RefCell<Option<Wallpaper>>>,
    selected_label: &Label,
) {
    while let Some(child) = gallery.first_child() {
        gallery.remove(&child);
    }
    let query = query.trim().to_lowercase();
    let mut displayed = 0_usize;
    for wallpaper in wallpapers {
        if !query.is_empty() && !wallpaper.name.to_lowercase().contains(&query) {
            continue;
        }
        if displayed == MAX_VISIBLE_WALLPAPERS {
            break;
        }
        gallery.append(&wallpaper_button(
            wallpaper,
            Rc::clone(selected),
            selected_label.clone(),
        ));
        displayed = displayed.saturating_add(1);
    }
    if displayed == 0 {
        gallery.append(&Label::new(Some("No matching wallpaper.")));
    }
}

fn wallpaper_button(
    wallpaper: &Wallpaper,
    selected: Rc<RefCell<Option<Wallpaper>>>,
    selected_label: Label,
) -> Button {
    let content = GtkBox::builder()
        .orientation(Orientation::Vertical)
        .spacing(0)
        .width_request(160)
        .hexpand(false)
        .vexpand(false)
        .build();
    match thumbnail_bytes(wallpaper)
        .ok()
        .and_then(|bytes| gdk::Texture::from_bytes(&glib::Bytes::from_owned(bytes)).ok())
    {
        Some(texture) => {
            let picture = Picture::for_paintable(&texture);
            picture.set_size_request(160, 100);
            picture.set_can_shrink(true);
            picture.set_hexpand(false);
            picture.set_vexpand(false);
            content.append(&picture);
        }
        None => content.append(&Label::new(Some("Preview unavailable"))),
    }
    let button = Button::builder().child(&content).build();
    button.set_size_request(170, 110);
    let wallpaper = wallpaper.clone();
    button.connect_clicked(move |_| {
        selected.replace(Some(wallpaper.clone()));
        selected_label.set_label("Wallpaper selected.");
    });
    button
}

fn apply_session_wallpaper(path: &Path, output: &str) -> Result<(), String> {
    validate_wallpaper(path)?;
    let compositor = if env::var_os("NIRI_SOCKET").is_some() {
        "niri"
    } else if env::var_os("SWAYSOCK").is_some() {
        "sway"
    } else {
        return Err("open Genoa Settings from a Niri or Sway session".to_string());
    };
    let executable = current_user_binary("wp-apply")?;
    let status = Command::new(executable)
        .args([path, Path::new(compositor), Path::new(output)])
        .status()
        .map_err(|error| format!("could not start the wallpaper helper: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("the wallpaper helper exited with {status}"))
    }
}

fn active_output_names() -> Result<Vec<String>, String> {
    if env::var_os("NIRI_SOCKET").is_some() {
        let output = Command::new("/usr/bin/niri")
            .args(["msg", "--json", "outputs"])
            .output()
            .map_err(|error| format!("could not query Niri displays: {error}"))?;
        if !output.status.success() {
            return Err("Niri did not return its active displays".to_string());
        }
        let outputs = serde_json::from_slice::<serde_json::Value>(&output.stdout)
            .map_err(|error| format!("could not parse Niri displays: {error}"))?;
        let Some(outputs) = outputs.as_object() else {
            return Err("Niri returned an unexpected display list".to_string());
        };
        return Ok(outputs
            .iter()
            .filter(|(_, details)| {
                details
                    .get("current_mode")
                    .is_some_and(|mode| !mode.is_null())
                    && details
                        .get("logical")
                        .is_some_and(|logical| !logical.is_null())
            })
            .map(|(name, _)| name.clone())
            .collect());
    }
    if env::var_os("SWAYSOCK").is_some() {
        let output = Command::new("/usr/bin/swaymsg")
            .args(["-t", "get_outputs"])
            .output()
            .map_err(|error| format!("could not query Sway displays: {error}"))?;
        if !output.status.success() {
            return Err("Sway did not return its active displays".to_string());
        }
        let outputs = serde_json::from_slice::<Vec<serde_json::Value>>(&output.stdout)
            .map_err(|error| format!("could not parse Sway displays: {error}"))?;
        return Ok(outputs
            .into_iter()
            .filter(|output| {
                output.get("active").and_then(serde_json::Value::as_bool) == Some(true)
            })
            .filter_map(|output| {
                output
                    .get("name")
                    .and_then(serde_json::Value::as_str)
                    .map(ToOwned::to_owned)
            })
            .collect());
    }
    Err("open Genoa Settings from a Niri or Sway session".to_string())
}

fn current_user_binary(name: &str) -> Result<std::path::PathBuf, String> {
    let home = env::var_os("HOME")
        .map(std::path::PathBuf::from)
        .ok_or_else(|| "HOME is unavailable".to_string())?;
    let executable = home.join(".cargo/bin").join(name);
    executable
        .is_file()
        .then_some(executable)
        .ok_or_else(|| format!("{name} is not installed"))
}

fn lock_and_sleep_page(settings: &IdleSettings, load_warning: Option<&str>) -> GtkBox {
    let page = page_container();
    page.append(&heading(
        "Lock and sleep",
        "Set the idle sequence once for both Niri and Sway.",
    ));

    let dim = timeout_control("Dim display after", settings.dim_after_seconds);
    let lock = timeout_control("Lock session after", settings.lock_after_seconds);
    let display_off = timeout_control(
        "Turn displays off after",
        settings.display_off_after_seconds,
    );
    let suspend = timeout_control("Suspend after", settings.suspend_after_seconds);
    page.append(&dim.row);
    page.append(&lock.row);
    page.append(&display_off.row);
    page.append(&suspend.row);

    page.append(&section(
        "Fullscreen video",
        "Genoa honors native Wayland idle inhibition. Fullscreen video applications that request it pause the idle sequence automatically and release it when playback or fullscreen ends.",
    ));
    page.append(&section(
        "Builds and updates",
        "These controls prevent only system suspend. Your display may dim and the session may lock normally while a build or update runs.",
    ));
    let build_inhibit = suspend_inhibit_control(
        "Prevent suspend while compiling",
        settings.inhibit_suspend_while_building,
    );
    let update_inhibit = suspend_inhibit_control(
        "Prevent suspend during Genoa updates",
        settings.inhibit_suspend_while_updating,
    );
    page.append(&build_inhibit.row);
    page.append(&update_inhibit.row);

    let save = Button::with_label("Save lock and sleep settings");
    save.add_css_class("suggested-action");
    save.set_halign(Align::Start);
    let status = Label::builder().xalign(0.0).wrap(true).build();
    if let Some(warning) = load_warning {
        status.set_label(&format!(
            "The saved settings could not be used: {warning}. Showing safe defaults."
        ));
    }
    page.append(&save);
    page.append(&status);

    let dim_value = dim.selector;
    let lock_value = lock.selector;
    let display_off_value = display_off.selector;
    let suspend_value = suspend.selector;
    let build_inhibit_value = build_inhibit.switch;
    let update_inhibit_value = update_inhibit.switch;
    save.connect_clicked(move |_| {
        let settings = IdleSettings {
            dim_after_seconds: timeout_value(&dim_value),
            lock_after_seconds: timeout_value(&lock_value),
            display_off_after_seconds: timeout_value(&display_off_value),
            suspend_after_seconds: timeout_value(&suspend_value),
            inhibit_suspend_while_building: build_inhibit_value.is_active(),
            inhibit_suspend_while_updating: update_inhibit_value.is_active(),
        };
        match settings.save() {
            Ok(()) => match restart_active_idle_service() {
                Ok(service) => status.set_label(&format!("Saved. Restarting {service}.")),
                Err(error) => status.set_label(&format!(
                    "Saved, but could not restart idle management: {error}"
                )),
            },
            Err(error) => status.set_label(&error),
        }
    });

    page
}

struct TimeoutControl {
    row: GtkBox,
    selector: DropDown,
}

fn timeout_control(label: &str, seconds: u32) -> TimeoutControl {
    let row = GtkBox::builder()
        .orientation(Orientation::Horizontal)
        .spacing(18)
        .hexpand(true)
        .build();
    let label = Label::builder()
        .label(label)
        .xalign(0.0)
        .hexpand(true)
        .build();
    let choices = StringList::new(&[]);
    for minutes in TIMEOUT_MINUTES {
        choices.append(&format!(
            "{minutes} minute{}",
            if *minutes == 1 { "" } else { "s" }
        ));
    }
    let selector = DropDown::new(Some(choices), None::<gtk4::Expression>);
    let minutes = seconds.div_ceil(60);
    let selected = TIMEOUT_MINUTES
        .iter()
        .position(|candidate| *candidate >= minutes)
        .unwrap_or(TIMEOUT_MINUTES.len().saturating_sub(1));
    selector.set_selected(u32::try_from(selected).unwrap_or(0));
    row.append(&label);
    row.append(&selector);
    TimeoutControl { row, selector }
}

fn timeout_value(control: &DropDown) -> u32 {
    let selected = usize::try_from(control.selected()).unwrap_or(0);
    TIMEOUT_MINUTES
        .get(selected)
        .copied()
        .unwrap_or(TIMEOUT_MINUTES[0])
        .saturating_mul(60)
}

struct SuspendInhibitControl {
    row: GtkBox,
    switch: Switch,
}

fn suspend_inhibit_control(label: &str, active: bool) -> SuspendInhibitControl {
    let row = GtkBox::builder()
        .orientation(Orientation::Horizontal)
        .spacing(18)
        .hexpand(true)
        .build();
    let label = Label::builder()
        .label(label)
        .xalign(0.0)
        .hexpand(true)
        .build();
    let switch = Switch::builder().active(active).build();
    row.append(&label);
    row.append(&switch);
    SuspendInhibitControl { row, switch }
}

fn restart_active_idle_service() -> Result<&'static str, String> {
    let service = if env::var_os("NIRI_SOCKET").is_some() {
        "genoa-niri-idle.service"
    } else if env::var_os("SWAYSOCK").is_some() {
        "genoa-sway-idle.service"
    } else {
        return Err("open Genoa Settings from a Niri or Sway session".to_string());
    };
    let service_for_worker = service.to_string();
    thread::Builder::new()
        .name("genoa-idle-restart".to_string())
        .spawn(move || {
            match Command::new("/usr/bin/systemctl")
                .args(["--user", "restart", &service_for_worker])
                .status()
            {
                Ok(status) if status.success() => {}
                Ok(status) => {
                    eprintln!("genoa-settings: {service_for_worker} exited with {status}");
                }
                Err(error) => {
                    eprintln!("genoa-settings: could not restart {service_for_worker}: {error}");
                }
            }
        })
        .map_err(|error| format!("could not start the idle-service restart: {error}"))?;
    Ok(service)
}

fn page_container() -> GtkBox {
    GtkBox::builder()
        .orientation(Orientation::Vertical)
        .spacing(18)
        .hexpand(true)
        .vexpand(true)
        .build()
}

fn heading(title: &str, subtitle: &str) -> GtkBox {
    let container = GtkBox::builder()
        .orientation(Orientation::Vertical)
        .spacing(8)
        .build();
    let title = Label::builder()
        .label(title)
        .xalign(0.0)
        .halign(Align::Start)
        .css_classes(["title-1"])
        .build();
    let subtitle = Label::builder()
        .label(subtitle)
        .xalign(0.0)
        .halign(Align::Start)
        .wrap(true)
        .css_classes(["dim-label"])
        .build();
    container.append(&title);
    container.append(&subtitle);
    container
}

fn section(title: &str, body: &str) -> GtkBox {
    let container = GtkBox::builder()
        .orientation(Orientation::Vertical)
        .spacing(8)
        .margin_top(12)
        .margin_bottom(12)
        .margin_start(16)
        .margin_end(16)
        .build();
    let title = Label::builder()
        .label(title)
        .xalign(0.0)
        .css_classes(["heading"])
        .build();
    let body = Label::builder().label(body).xalign(0.0).wrap(true).build();
    container.append(&title);
    container.append(&body);
    container
}
