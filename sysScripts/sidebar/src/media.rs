//! Media widget backed by playerctl.
//!
//! The card stays hidden when no MPRIS player is active and updates once per second.

use crate::helpers; // Shared helper for running shell commands
use gtk4::prelude::*;
use gtk4::{Align, Box, Button, Label, Orientation};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};

struct MediaSnapshot {
    status: String,
    title: String,
    artist: String,
}

fn parse_media_snapshot(out: &[u8]) -> Option<MediaSnapshot> {
    let raw = String::from_utf8_lossy(out);
    let parts: Vec<&str> = raw.trim().split(";;").collect();
    if parts.len() < 3 {
        return None;
    }

    Some(MediaSnapshot {
        status: parts[0].to_string(),
        title: parts[1].to_string(),
        artist: parts[2].to_string(),
    })
}

/// Builds the Media Player card.
pub fn build() -> Box {
    // 1. The Container (Hidden by Default)
    // We start with visibility set to FALSE. The polling loop will turn it TRUE
    // only if it successfully gets metadata from a player.
    // This ensures the sidebar doesn't show an empty/broken player.
    let container = Box::builder()
        .orientation(Orientation::Vertical)
        .css_classes(vec!["media-card"])
        .visible(false) // Start hidden
        .halign(Align::Fill)
        .build();

    // 2. Metadata Labels (Title & Artist)
    // We use ellipsize settings to ensure long song titles don't stretch the sidebar
    // or break the layout. They will show as "Song Title..." if too long.
    let title_label = Label::builder()
        .label("Unknown Title")
        .css_classes(vec!["media-title"])
        .wrap(true)
        .max_width_chars(25) // Approx width before wrapping/cutting off
        .ellipsize(gtk4::pango::EllipsizeMode::End)
        .halign(Align::Center)
        .build();

    let artist_label = Label::builder()
        .label("Unknown Artist")
        .css_classes(vec!["media-artist"])
        .wrap(true)
        .max_width_chars(25)
        .ellipsize(gtk4::pango::EllipsizeMode::End)
        .halign(Align::Center)
        .build();

    // 3. Playback Controls (Prev | Play/Pause | Next)
    let controls = Box::builder()
        .orientation(Orientation::Horizontal)
        .halign(Align::Center)
        .spacing(10)
        .margin_top(5)
        .build();

    let btn_prev = Button::builder()
        .label("⏮")
        .css_classes(vec!["media-btn"])
        .build();
    let btn_play = Button::builder()
        .label("⏸")
        .css_classes(vec!["media-btn", "play-btn"])
        .build();
    let btn_next = Button::builder()
        .label("⏭")
        .css_classes(vec!["media-btn"])
        .build();

    // --- Signal Handlers ---
    // These buttons simply fire-and-forget commands to playerctl.
    // We rely on the polling loop to update the UI state (e.g. changing Pause to Play icon).

    btn_prev.connect_clicked(|_| {
        helpers::run_command("playerctl", &["previous"]);
    });
    btn_next.connect_clicked(|_| {
        helpers::run_command("playerctl", &["next"]);
    });

    let btn_play_clone = btn_play.clone();
    btn_play.connect_clicked(move |_| {
        helpers::run_command("playerctl", &["play-pause"]);
        // Note: We don't manually change the icon here.
        // We let the next poll cycle (max 1s delay) detect the state change.
        // This prevents the UI from getting out of sync if the command fails.
    });

    controls.append(&btn_prev);
    controls.append(&btn_play);
    controls.append(&btn_next);

    container.append(&title_label);
    container.append(&artist_label);
    container.append(&controls);

    // Poll from the GTK loop, but run command I/O on a worker thread.
    let container_poll = container.clone();
    let title_poll = title_label.clone();
    let artist_poll = artist_label.clone();
    let play_btn_poll = btn_play_clone.clone();

    let (tx, rx) = mpsc::channel::<Option<MediaSnapshot>>();
    let in_flight = Arc::new(AtomicBool::new(false));

    // Runs every 1 second
    glib::timeout_add_seconds_local(1, move || {
        if let Ok(snapshot) = rx.try_recv() {
            match snapshot {
                Some(data) => {
                    container_poll.set_visible(true);
                    title_poll.set_label(&data.title);
                    artist_poll.set_label(&data.artist);
                    if data.status == "Playing" {
                        play_btn_poll.set_label("⏸");
                    } else {
                        play_btn_poll.set_label("▶");
                    }
                }
                None => {
                    container_poll.set_visible(false);
                }
            }
        }

        // Keep at most one fetch in flight to avoid thread pileups under slow/hung MPRIS.
        if !in_flight.swap(true, Ordering::AcqRel) {
            let tx_bg = tx.clone();
            let in_flight_bg = Arc::clone(&in_flight);
            std::thread::spawn(move || {
                let output = helpers::get_output(
                    "playerctl",
                    &["metadata", "--format", "{{status}};;{{title}};;{{artist}}"],
                );
                let parsed = output.as_deref().and_then(parse_media_snapshot);
                let _ = tx_bg.send(parsed);
                in_flight_bg.store(false, Ordering::Release);
            });
        }

        // Return Continue to keep the loop running
        glib::ControlFlow::Continue
    });

    container
}
