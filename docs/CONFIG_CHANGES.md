## Required Config Changes

My updater will not update your configs, it will only update my rust tools in /sysScripts. Because of this, breaking changes will periodically occur in configs that users must manually fix. I believe this is the best choice for user customization and personalization. That said, I will start maintaining a list of breaking config changes at the top of this readme. This list is not exhaustive and users can always refer back to my configs on this repo for complete and up to date config examples.

- **swww**: (the Niri wallpaper manager dependency) has renamed to awww. Users will need to change line 83 of their niri configs to `spawn-at-startup "awww-daemon" "--namespace" "niri"` to have the correct dependency start when logging into Niri. The package will already be installed and the wallpaper management tooling is already refactored via the updater.
- **weather and startup config cleanup**: The weather refactor removed the old OpenWeatherMap and Google Geolocation setup from the installer. The affected `.config` changes from the last commit were:

  ```text
  .config/niri/config.kdl                    -> start gsd-datetime with systemd-run at login
  .config/rust-dotfiles/config.toml.template -> remove the waybar_weather block and OWM placeholder
  .config/sway/UserConfigs/Startup_Apps.conf -> start gsd-datetime with systemd-run at login
  .config/waybar/Modules                     -> remove the fixed timezone from the calendar module
  .config/waybar/ModulesCustom               -> change to:
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

- **compositor args in .config/rust-dotfiles**: In the rust-dotfiles config, the keybind launcher script now relies on arguments for `compositor`. The sheet blocks should look like this:

```
[[kb_launcher.sheet]]
name = "Niri"
file = "~/.config/niri/keybinds_niri.txt"
compositor = "sway"

[[kb_launcher.sheet]]
name = "Sway"
file = "~/.config/sway/keybinds_sway.txt"
compositor = "niri"
```

- **Portal Changes:** The GNOME backend and Niri/Sway backend require talking to different compositors. To do this:

```
sudo pacman -S xdg-desktop-portal-gnome
mkdir -p ~/.config/xdg-desktop-portal
echo -e "[preferred]\ndefault=wlr;gtk;" > ~/.config/xdg-desktop-portal/portals.conf
echo -e "[preferred]\ndefault=gnome;gtk;" > ~/.config/xdg-desktop-portal/gnome-portals.conf
systemctl --user restart xdg-desktop-portal

```
- **Idle & Lock Changes:** The old hypr dependencies are not being kept, and are causing some issues on waking from suspend. 

1. Copy the `gtklock` directory from this repo's .config into your local `~/.config`.

2. In `~/.config/niri/config`:
Replace:

```
spawn-sh-at-startup "iDIR=\"$HOME/.config/swaync/images/ja.png\"; export iDIR; swayidle -w timeout 200 'brightnessctl set 0%' resume 'brightnessctl set 60%' timeout 600 'pidof hyprlock || hyprlock &' timeout 630 'wlr-randr --output eDP-1 --off; wlr-randr --output DP-2 --off; wlr-randr --output DP-3 --off; wlr-randr --output DP-4 --off' resume 'wlr-randr --output eDP-1 --on; wlr-randr --output DP-2 --on; wlr-randr --output DP-3 --on; wlr-randr --output DP-4 --on' timeout 1200 'systemctl suspend' before-sleep 'pidof hyprlock || hyprlock &'"
```

with:

```
spawn-sh-at-startup "swayidle -w timeout 200 'brightnessctl set 0%' resume 'brightnessctl set 60%' timeout 600 'flock -n /tmp/gtklock.lock gtklock -d' timeout 630 'NIRI_SOCKET=\"$NIRI_SOCKET\" niri msg action power-off-monitors' resume 'NIRI_SOCKET=\"$NIRI_SOCKET\" niri msg action power-on-monitors' timeout 1200 'systemctl suspend' before-sleep 'flock -n /tmp/gtklock.lock gtklock -d'"

```

**AND** replace:

```
    //suspend
    Mod+Alt+S hotkey-overlay-title="Lock and Suspend" {
        spawn-sh "pidof hyprlock >/dev/null || hyprlock & sleep 0.5; systemctl suspend"
    }
    Mod+Alt+K { spawn-sh "pkill swaync && swaync"; }

```

with:

```
    //suspend
    Mod+Alt+S hotkey-overlay-title="Lock and Suspend" {
        spawn-sh "flock -n /tmp/gtklock.lock gtklock -d; systemctl suspend"
    }
    Mod+Alt+K { spawn-sh "pkill swaync && swaync"; }
```

**AND** replace:

```
    Super+Alt+L hotkey-overlay-title="Lock the Screen: hyprlock" { spawn-sh "pidof hyprlock || hyprlock &"; }
```

with:

```
    Super+Alt+L hotkey-overlay-title="Lock the Screen: gtklock" { spawn-sh "flock -n /tmp/gtklock.lock gtklock -d"; }
```

3. In `~/.config/sway/`:

Replace everything in `` with:

```
#!/usr/bin/env bash

iDIR="$HOME/.config/swaync/images/ja.png"

# kill any existing instance (in case of reload)
pkill swayidle 2>/dev/null

swayidle -w \
  timeout 60 'brightnessctl set 0%' \
  resume 'brightnessctl set 15%' \
  timeout 240 'flock -n /tmp/gtklock.lock gtklock -d' \
  timeout 270 'swaymsg "output * dpms off"' \
  resume 'swaymsg "output * dpms on"' \
  timeout 600 'systemctl suspend' \
  before-sleep 'flock -n /tmp/gtklock.lock gtklock -d'

```

**AND** in `UserConfigs/UserKeybinds.conf` replace:

```
bindsym $mainMod+Alt+s exec sh -c 'pidof hyprlock >/dev/null || (hyprlock & sleep 0.5); systemctl suspend'

```

with:

```
bindsym $mainMod+Alt+s exec sh -c 'flock -n /tmp/gtklock.lock gtklock -d; systemctl suspend'

```

**AND** replace:

```
bindsym Control+Mod1+l exec pidof hyprlock || hyprlock &
```

with:

```
bindsym Control+Mod1+l exec sh -c 'flock -n /tmp/gtklock.lock gtklock -d'

```


**After running these steps you should run `sudo pacman -Rns hypridle hyprlock` and `rm -rf ~/.config/hypr`**
