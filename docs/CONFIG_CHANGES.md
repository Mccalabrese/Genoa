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

- **Window Rules for Cal-Tui**: The calendar tui requires window rules in niri and sway's configs to float in the center of the screen. These should be added:

### Niri (~/.config/niri/config.kdl)

```

window-rule {
    match app-id="ghostty" title="calendar-tui"
    open-floating true
    default-column-width { fixed 1000; }
    default-window-height { fixed 600; }
}
```

### Sway (~/.config/sway/UserConfigs/WindowRules.conf)

```
for_window [title="^calendar-tui$"] floating enable, resize set 1000 600, move position center
```

- **Portal Changes: The GNOME backend and Niri/Sway backend require talking to different compositors. To do this:

```
sudo pacman -S xdg-desktop-portal-gnome
mkdir -p ~/.config/xdg-desktop-portal
echo -e "[preferred]\ndefault=wlr;gtk;" > ~/.config/xdg-desktop-portal/portals.conf
echo -e "[preferred]\ndefault=gnome;gtk;" > ~/.config/xdg-desktop-portal/gnome-portals.conf
systemctl --user restart xdg-desktop-portal

```
