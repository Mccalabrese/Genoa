# Required Config Changes

My updater deliberately does not overwrite your dotfiles. It normally updates only the managed Rust tools in `sysScripts`, so you can keep your own setup and personalization. The GTKLock retirement below is the narrow exception: it rewrites only Genoa's former, known lock commands, and leaves GTKLock installed if it finds a custom active command it cannot safely migrate.

I will keep the important changes here. This is not an exhaustive changelog; the configs in this repository are always the complete, current examples.

## GNOME Files Replaces Thunar

**September 5, 2026 — applies to anyone who installed Genoa before this date.**

New Genoa installs now launch GNOME Files (`nautilus`) instead of Thunar. I am not removing Thunar, modifying your existing config, or changing your package state through the updater.

If you want your current install to use GNOME Files too, first make sure it is available:

```bash
sudo pacman -S --needed nautilus
```

Then make these small changes to your own config:

### Sway

In `~/.config/sway/UserConfigs/UserKeybinds.conf`, replace:

```conf
set $files thunar
```

with:

```conf
set $files nautilus
```

### Niri

In `~/.config/niri/config.kdl`, replace:

```kdl
Mod+E { spawn "thunar"; } // File Manager
```

with:

```kdl
Mod+E { spawn "nautilus"; } // File Manager
```

### Waybar

In `~/.config/waybar/ModulesCustom`, replace:

```jsonc
"on-click": "thunar &",
```

with:

```jsonc
"on-click": "nautilus &",
```

In `~/.config/waybar/ModulesWorkspaces`, replace:

```jsonc
"class<thunar|nemo>": "󰝰 ",
```

with:

```jsonc
"class<org.gnome.Nautilus>": "󰝰 ",
```

Restart your session, or reload the relevant compositor and Waybar, after making the changes.

Once GNOME Files is working for you, remove the retired file-manager packages:

```bash
sudo pacman -Rns thunar thunar-volman tumbler
```

---

## Previous Config Changes

### Genoa Lock Replaces GTKLock

GTKLock and its unmaintained runshell module are retired. During a normal
Genoa refresh, the installer builds and installs the root-owned
`/usr/local/bin/genoa-lock`, writes its dedicated PAM policy, migrates Genoa's
known Sway and Niri commands, and removes the GTKLock packages plus
`~/.config/gtklock`.

If it finds an active custom GTKLock command it cannot recognize, it leaves
GTKLock installed and prints the exact file that needs manual migration. That
is deliberate: an unknown customization is never a reason to leave a user
without a screen locker.

### Keybind Launcher Compositor Filters

**September 3, 2026 — applies to anyone who installed Genoa before this date.**

The keybind launcher now relies on a `compositor` field in the `[[kb_launcher.sheet]]` blocks of `.config/rust-dotfiles/config.toml`. They should look like this:

```toml
[[kb_launcher.sheet]]
name = "Niri"
file = "~/.config/niri/keybinds_niri.txt"
compositor = "niri"

[[kb_launcher.sheet]]
name = "Sway"
file = "~/.config/sway/keybinds_sway.txt"
compositor = "sway"
```

### Portal Backends

**August 31, 2026 — applies to anyone who installed Genoa before this date.**

The GNOME backend and the Niri/Sway backend need to talk to different compositors. Run:

```bash
sudo pacman -S xdg-desktop-portal-gnome
mkdir -p ~/.config/xdg-desktop-portal
printf '[preferred]\ndefault=wlr;gtk;\n' > ~/.config/xdg-desktop-portal/portals.conf
printf '[preferred]\ndefault=gnome;gtk;\n' > ~/.config/xdg-desktop-portal/gnome-portals.conf
systemctl --user restart xdg-desktop-portal
```

### Weather and Startup Cleanup

**July 24, 2026 — applies to anyone who installed Genoa before this date.**

The weather refactor removed the old OpenWeatherMap and Google Geolocation setup from the installer. The affected `.config` changes were:

```text
.config/niri/config.kdl                    -> start gsd-datetime with systemd-run at login
.config/rust-dotfiles/config.toml.template -> remove the waybar_weather block and OWM placeholder
.config/sway/UserConfigs/Startup_Apps.conf -> start gsd-datetime with systemd-run at login
.config/waybar/Modules                     -> remove the fixed timezone from the calendar module
```

Update the weather module in `.config/waybar/ModulesCustom` to:

```jsonc
"custom/weather": {
  "format": "{text}",
  "format-alt": "{alt}: {text}",
  "format-alt-click": "click",
  "return-type": "json",
  "exec": "$HOME/.cargo/bin/waybar-weather --unit c",
  "on-click": "$HOME/.cargo/bin/waybar-weather --prompt",
  "tooltip": true,
},
```

### `swww` Renamed to `awww`

**March 25, 2026 — applies to anyone who installed Genoa before this date.**

The Niri wallpaper-manager dependency was renamed. In your Niri config, change the startup command to:

```kdl
spawn-at-startup "awww-daemon" "--namespace" "niri"
```

The package is already installed and the wallpaper-management tooling was refactored through the updater.
