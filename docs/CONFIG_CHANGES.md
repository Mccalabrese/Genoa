# Required Config Changes

My updater deliberately does not overwrite your dotfiles. It only updates the managed Rust tools in `sysScripts`, so you can keep your own setup and personalization. That also means that when a config change is required, you will need to apply it yourself.

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

### Idle and Lock Changes

**September 5, 2026 — applies to anyone who installed Genoa before this date.**

The old Hyprland idle and lock dependencies are no longer kept and can cause issues after waking from suspend.

1. Copy this repository's `.config/gtklock` directory into `~/.config`.

2. In `~/.config/niri/config.kdl`, replace:

   ```kdl
   spawn-sh-at-startup "iDIR=\"$HOME/.config/swaync/images/ja.png\"; export iDIR; swayidle -w timeout 200 'brightnessctl set 0%' resume 'brightnessctl set 60%' timeout 600 'pidof hyprlock || hyprlock &' timeout 630 'wlr-randr --output eDP-1 --off; wlr-randr --output DP-2 --off; wlr-randr --output DP-3 --off; wlr-randr --output DP-4 --off' resume 'wlr-randr --output eDP-1 --on; wlr-randr --output DP-2 --on; wlr-randr --output DP-3 --on; wlr-randr --output DP-4 --on' timeout 1200 'systemctl suspend' before-sleep 'pidof hyprlock || hyprlock &'"
   ```

   with:

   ```kdl
   spawn-sh-at-startup "swayidle -w timeout 200 'brightnessctl set 0%' resume 'brightnessctl set 60%' timeout 600 'flock -n /tmp/gtklock.lock gtklock -d' timeout 630 'NIRI_SOCKET=\"$NIRI_SOCKET\" niri msg action power-off-monitors' resume 'NIRI_SOCKET=\"$NIRI_SOCKET\" niri msg action power-on-monitors' timeout 1200 'systemctl suspend' before-sleep 'sh -c \"flock -n /tmp/gtklock.lock gtklock &\"'"
   ```

   Also replace:

   ```kdl
   //suspend
   Mod+Alt+S hotkey-overlay-title="Lock and Suspend" {
       spawn-sh "pidof hyprlock >/dev/null || hyprlock & sleep 0.5; systemctl suspend"
   }
   Mod+Alt+K { spawn-sh "pkill swaync && swaync"; }
   ```

   with:

   ```kdl
   //suspend
   Mod+Alt+S hotkey-overlay-title="Lock and Suspend" {
       spawn-sh "flock -n /tmp/gtklock.lock gtklock -d; systemctl suspend"
   }
   Mod+Alt+K { spawn-sh "pkill swaync && swaync"; }
   ```

   Finally, replace:

   ```kdl
   Super+Alt+L hotkey-overlay-title="Lock the Screen: hyprlock" { spawn-sh "pidof hyprlock || hyprlock &"; }
   ```

   with:

   ```kdl
   Super+Alt+L hotkey-overlay-title="Lock the Screen: gtklock" { spawn-sh "flock -n /tmp/gtklock.lock gtklock -d"; }
   ```

3. Replace the contents of `~/.config/sway/swayidle.sh` with:

   ```bash
   #!/usr/bin/env bash

   iDIR="$HOME/.config/swaync/images/ja.png"

   # Kill any existing instance before starting a fresh one.
   pkill swayidle 2>/dev/null

   swayidle -w \
     timeout 60 'brightnessctl set 0%' \
     resume 'brightnessctl set 15%' \
     timeout 240 'flock -n /tmp/gtklock.lock gtklock -d' \
     timeout 270 'swaymsg "output * dpms off"' \
     resume 'swaymsg "output * dpms on"' \
     timeout 600 'systemctl suspend' \
     before-sleep 'sh -c "flock -n /tmp/gtklock.lock gtklock &"'
   ```

   In `~/.config/sway/UserConfigs/UserKeybinds.conf`, replace:

   ```conf
   bindsym $mainMod+Alt+s exec sh -c 'pidof hyprlock >/dev/null || (hyprlock & sleep 0.5); systemctl suspend'
   ```

   with:

   ```conf
   bindsym $mainMod+Alt+s exec sh -c 'flock -n /tmp/gtklock.lock gtklock -d; systemctl suspend'
   ```

   Then replace:

   ```conf
   bindsym Control+Mod1+l exec pidof hyprlock || hyprlock &
   ```

   with:

   ```conf
   bindsym Control+Mod1+l exec sh -c 'flock -n /tmp/gtklock.lock gtklock -d'
   ```

Afterward, remove the retired Hyprland packages and config:

```bash
sudo pacman -Rns hypridle hyprlock
rm -rf ~/.config/hypr
```

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
