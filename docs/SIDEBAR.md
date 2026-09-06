# Sidebar

The sidebar is a small GTK4 layer-shell control panel for my Niri, Sway, and Hyprland sessions. It is opened from the sidebar button in Waybar and closes when it loses focus, unless the pointer is still over the window.

The goal is simple: keep the controls I use most often in one place without turning the sidebar into another large desktop application.

## What it includes

### Session controls

The top row contains controls for:

- idle inhibition
- suspend
- locking the screen
- logging out
- rebooting
- powering off

The idle button pauses or resumes `swayidle` and `hypridle` with `SIGSTOP`/`SIGCONT`, and restores its state when the sidebar opens.

### System controls

The second row provides quick access to:

- radio-menu
- system updates
- airplane mode
- Cloudflare DNS over HTTPS
- audio mute
- wallpaper selection
- the keybind launcher

The DNS badge is refreshed for a short period after toggling. The update badge is checked every 30 minutes. Both checks run outside the GTK thread.

### Brightness and volume

The sliders control `brightnessctl` and the default PipeWire sink through `wpctl`. The sidebar also checks for changes made outside the window, such as hardware keys.

The current implementation deliberately keeps the slider write path simple. A drag can issue several quick commands, so slider responsiveness is still an area I may revisit separately.

### Media

The media card uses `playerctl` to read the active MPRIS player. It stays hidden when no player is available and shows the current title, artist, and playback state when one appears.

The previous, play/pause, and next buttons are fire-and-forget `playerctl` commands. The next metadata refresh updates the displayed state.

### System information

The system card displays a startup snapshot of:

- kernel version
- login shell
- current desktop/session
- installed package count
- uptime

These values are intentionally fetched once instead of being polled.

### Finance

The finance row displays the JSON output from `waybar-finance`. Clicking it opens the full finance TUI in Ghostty.

### Calendar

The calendar has month and day views. Calendar events are read through the experimental `clepsydre-eds` and `clepsydre-rebind` dependencies instead of the old standalone C query helper.

Calendar queries run on a dedicated worker with its own GLib context. Results return to the GTK thread through an async channel, so opening or changing months does not block the sidebar while Evolution Data Server responds.

The month view marks days containing appointments. The day view lists the appointments for the selected date and can open an event in GNOME Calendar.

## Opening the sidebar

The Waybar module is defined in `.config/waybar/ModulesCustom`:

```jsonc
"custom/sidebar_toggle": {
  "format": "  ",
  "on-click": "$HOME/Genoa/sidebar-toggle"
}
```

The `sidebar-toggle` script launches `~/.cargo/bin/sidebar` when it is closed and terminates the running process when it is open. It also uses `/tmp/sidebar_just_closed` to avoid immediately reopening the window after the sidebar closes itself because focus moved elsewhere.

## Building it manually

From the repository root:

```bash
cargo build --release --manifest-path sysScripts/sidebar/Cargo.toml
cp sysScripts/sidebar/target/release/sidebar ~/.cargo/bin/sidebar
```

The normal updater builds the Rust tools for me, so this manual build is mainly useful when working on the sidebar itself.

The build needs the GTK4 and layer-shell development libraries available through the system package manager. Calendar support also needs the Evolution Data Server libraries required by the `clepsydre` crates. Runtime features depend on the local tools being installed:

- `playerctl` for media
- `brightnessctl` for brightness
- `wpctl` for volume and mute
- `gnome-calendar` for opening calendar events
- `cf-status` and `cf-toggle` for DNS status and toggling. The toggle requires a running NetworkManager and manages a dedicated `/etc/NetworkManager/conf.d/90-dnscrypt-proxy.conf` override instead of editing `/etc/resolv.conf`. When off, it uses direct Cloudflare DNS (`1.1.1.1` and `1.0.0.1`).
- `update-check` and `updater` for system updates
- `waybar-finance` for the finance row
- `wp-select`, `kb-launcher`, and `radio-menu` for their launch buttons

### The packaged clepsydre dependency

The calendar currently depends on a locally packaged WIP build of clepsydre. The install wizard downloads this package, verifies its SHA-256 checksum, and installs it before compiling the Rust tools. The local-package operation is `pacman -U` with an uppercase `U`.

If you are building the sidebar manually, install the same package first:

```bash
clepsydre_pkg="$HOME/.cache/genoa/clepsydre-git-r.head-1-x86_64.pkg.tar.zst"
mkdir -p "$(dirname "$clepsydre_pkg")"
curl --fail --location --retry 3 --retry-delay 2 \
  --proto '=https' --tlsv1.2 \
  --output "$clepsydre_pkg" \
  "https://github.com/Mccalabrese/Genoa/releases/download/v0.1.0/clepsydre-git-r.head-1-x86_64.pkg.tar.zst"
echo "fb17aa2066ec7d3a2e9ebb7b066b4547c9a22ab76e687ad45e9cc64541369852  $clepsydre_pkg" \
  | sha256sum --check
sudo pacman -U --needed --noconfirm "$clepsydre_pkg"
```

## Source layout

- `src/main.rs` sets the GTK environment and starts the application.
- `src/ui.rs` builds the window, controls, calendar views, and background status tasks.
- `src/helpers.rs` contains command helpers, calendar rendering, calendar worker code, and shared widget factories.
- `src/media.rs` owns the playerctl card.
- `src/sysinfo.rs` builds the one-time system information card.
- `src/style.rs` contains the embedded GTK CSS.

The GTK thread owns widgets and rendering. Anything that waits on a subprocess or calendar service is kept off that thread, with results delivered back through the GLib main context.

## Troubleshooting

If a command fails, the sidebar records a short failure line in:

```text
$XDG_RUNTIME_DIR/sidebar-telemetry.log
```

If `XDG_RUNTIME_DIR` is unavailable, it falls back to `/tmp/sidebar-telemetry.log`.

For a missing control, first check that the corresponding command exists in `PATH` or in `~/.cargo/bin`. For calendar problems, check that the Evolution Data Server libraries are installed and that a calendar account is available to the desktop session.
