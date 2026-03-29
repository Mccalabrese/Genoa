# Manual Install Guide (Current and Accurate)

This is the manual equivalent of what your tooling does today:
- `bootstrap.sh`
- `sysScripts/install-wizard/src/main.rs`
- `sysScripts/updater/src/main.rs`

If you just want the normal path, run the bootstrap script and let it handle everything.
This guide is for when you want to do it all by hand.

## Before You Start

- Run as a normal user, not root.
- Keep internet connected.
- Run commands in order.

Quick checks:

```bash
if [ "$EUID" -eq 0 ]; then echo "Do not run as root"; exit 1; fi
ping -c 1 archlinux.org
```

## 1. Match What bootstrap.sh Does

Sync keys/mirrors and update base system:

```bash
sudo pacman -Syu --noconfirm archlinux-keyring pacman-mirrorlist
```

Install bootstrap toolchain:

```bash
sudo pacman -S --needed --noconfirm base-devel rustup git pkgconf wget curl ca-certificates
```

Clone repo (if needed):

```bash
git clone https://github.com/Mccalabrese/rust-wayland-power.git
cd rust-wayland-power
```

Set Rust stable for this shell:

```bash
rustup default stable
export PATH="$HOME/.cargo/bin:$PATH"
```

Warm sudo cache:

```bash
sudo -v
```

## 2. Driver Pre-Step (Install Wizard Behavior)

The installer removes `jack2` early to avoid audio conflicts:

```bash
sudo pacman -Rdd --noconfirm jack2 || true
```

Detect GPU:

```bash
lspci -n | grep -Ei 'vga|3d|display'
```

### NVIDIA

If your card is GTX 16xx or RTX 20xx (Turing), the installer uses pinned legacy driver packages from Arch Archive.

Install legacy set:

```bash
sudo pacman -U --noconfirm \
  https://archive.archlinux.org/packages/n/nvidia-dkms/nvidia-dkms-580.119.02-1-x86_64.pkg.tar.zst \
  https://archive.archlinux.org/packages/n/nvidia-utils/nvidia-utils-580.119.02-1-x86_64.pkg.tar.zst \
  https://archive.archlinux.org/packages/n/nvidia-settings/nvidia-settings-580.119.02-1-x86_64.pkg.tar.zst
```

Pin packages in `/etc/pacman.conf` (`[options]` section):

```ini
IgnorePkg = nvidia-dkms nvidia-utils lib32-nvidia-utils nvidia-settings linux linux-headers linux-lts linux-lts-headers
```

If your NVIDIA card is newer (Ampere/Ada), install standard NVIDIA packages:

```bash
sudo pacman -S --needed --noconfirm nvidia-dkms nvidia-prime nvidia-settings libva-nvidia-driver
```

### AMD

```bash
sudo pacman -S --needed --noconfirm vulkan-radeon libva-mesa-driver xf86-video-amdgpu
```

### Intel

No extra vendor step needed here (covered in base package list below).

### Reboot checkpoint

The install wizard reboots after first GPU driver setup in a GUI session. If you are doing this manually, reboot now before continuing:

```bash
sudo reboot
```

## 3. Install Standard Packages (pkglist.txt)

These are the current package names from `pkglist.txt`:

Regenerate this block anytime with:

```bash
bash scripts/sync-manual-pkglist.sh
```

<!-- PKGLIST:START -->
```bash
sudo pacman -S --needed --noconfirm \
  base-devel git go rustup openssl pkgconf glibc wget curl jq \
  man-db man-pages unzip tree pciutils pacman-contrib bolt upower tlp bluez \
  bluez-utils blueman brightnessctl udiskie fwupd util-linux intel-media-driver libva-utils vulkan-intel sway \
  niri gnome hyprlock swayidle hypridle xdg-user-dirs-gtk greetd greetd-tuigreet xwayland-satellite qt5-wayland \
  qt6-wayland polkit-gnome geoclue xdg-desktop-portal-gnome xdg-desktop-portal-wlr xdg-desktop-portal-gtk wl-clipboard cliphist pipewire pipewire-pulse \
  pipewire-alsa pipewire-jack pavucontrol sof-firmware playerctl mpv-mpris thunar thunar-volman tumbler gvfs \
  gvfs-mtp gvfs-smb gvfs-gphoto2 file-roller gnome-disk-utility ufw timeshift seahorse gnome-keyring waybar \
  wofi rofi awww swaybg grim slurp mako papirus-icon-theme gnome-themes-extra adwaita-icon-theme \
  ttf-jetbrains-mono-nerd ttf-fira-code ttf-jetbrains-mono noto-fonts noto-fonts-emoji otf-font-awesome zsh starship ghostty tmux \
  fzf ripgrep bat btop fastfetch neovim networkmanager network-manager-applet discord tigervnc \
  mpv gparted simple-scan gnome-calculator cups system-config-printer cups-pdf zsh-autosuggestions zsh-syntax-highlighting dnscrypt-proxy \
  wireplumber
```
<!-- PKGLIST:END -->


## 4. Install AUR Packages (Current List)

Bootstrap `yay` if missing:

```bash
if ! command -v yay >/dev/null 2>&1; then
  cd "$HOME"
  rm -rf yay-clone
  git clone https://aur.archlinux.org/yay.git yay-clone
  cd yay-clone
  makepkg -si --noconfirm
  cd "$HOME"
  rm -rf yay-clone
fi
```

Install current AUR set:

```bash
yay -S --needed --noconfirm \
  zoom slack-desktop ledger-live-bin visual-studio-code-bin pinta \
  ttf-victor-mono pear-desktop-bin librewolf-bin
```

## 5. Build and Sync Rust Apps to ~/.cargo/bin

Set stable (again, like installer does):

```bash
rustup default stable
```

Build each crate and copy release binaries to `~/.cargo/bin`:

```bash
cd "$HOME/rust-wayland-power/sysScripts"
for d in */; do
  if [ -f "$d/Cargo.toml" ]; then
    (cd "$d" && cargo build --release -q)
  fi
done

mkdir -p "$HOME/.cargo/bin"
for d in "$HOME/rust-wayland-power/sysScripts"/*/; do
  rel="$d/target/release"
  [ -d "$rel" ] || continue
  find "$rel" -maxdepth 1 -type f -perm /111 \
    ! -name '.*' ! -name '*.d' ! -name '*.rlib' ! -name '*.so' ! -name '*.a' \
    -exec cp -f {} "$HOME/.cargo/bin/" \;
done
```

## 6. System Configuration (Matches configure_system + optimize_pacman_config)

### 6.1 Clean known mkinitcpio corruption edge case

```bash
sudo sed -i '${/^o"$/d}' /etc/mkinitcpio.conf
sudo sed -i '${/^o”$/d}' /etc/mkinitcpio.conf
```

### 6.2 Enable core services

```bash
sudo systemctl enable geoclue.service
sudo systemctl enable bluetooth.service
sudo systemctl enable bolt.service
sudo systemctl enable --now paccache.timer
```

### 6.3 DNS: use dnscrypt-proxy (not cloudflared service)

```bash
sudo pacman -S --needed --noconfirm dnscrypt-proxy
sudo sed -i "s/^# server_names = \['cloudflare'\]/server_names = ['cloudflare']/" /etc/dnscrypt-proxy/dnscrypt-proxy.toml
sudo sed -i "s/^listen_addresses = \['127.0.0.1:53'\]/listen_addresses = ['127.0.0.1:53', '[::1]:53']/" /etc/dnscrypt-proxy/dnscrypt-proxy.toml
sudo systemctl enable --now dnscrypt-proxy

sudo systemctl disable --now cloudflared-dns 2>/dev/null || true
sudo rm -f /etc/systemd/system/cloudflared-dns.service
sudo systemctl daemon-reload
```

### 6.4 Session env path

```bash
mkdir -p ~/.config/environment.d
printf 'PATH=$HOME/.cargo/bin:$PATH\n' > ~/.config/environment.d/99-cargo-path.conf
```

### 6.5 logind + greetd

```bash
sudo sed -i 's/#KillUserProcesses=no/KillUserProcesses=yes/' /etc/systemd/logind.conf
sudo sed -i 's/KillUserProcesses=no/KillUserProcesses=yes/' /etc/systemd/logind.conf

cat > /tmp/greetd_config.toml <<'EOF'
[terminal]
vt = 1
[default_session]
command = "tuigreet --time --remember --sessions /usr/share/wayland-sessions:/usr/share/xsessions"
user = "greeter"
EOF
sudo mv /tmp/greetd_config.toml /etc/greetd/config.toml
sudo systemctl disable gdm sddm lightdm 2>/dev/null || true
sudo systemctl enable --force greetd.service
```

### 6.6 Set shell and tmux plugin manager

```bash
sudo chsh -s /usr/bin/zsh "$USER"
if [ ! -d "$HOME/.tmux/plugins/tpm" ]; then
  git clone https://github.com/tmux-plugins/tpm "$HOME/.tmux/plugins/tpm"
fi
```

### 6.7 Remove old GNOME session desktop files and old NoExtract session rules

```bash
sudo rm -f \
  /usr/share/wayland-sessions/gnome.desktop \
  /usr/share/wayland-sessions/gnome-classic.desktop \
  /usr/share/wayland-sessions/gnome-classic-wayland.desktop

sudo sed -i '/wayland-sessions/d' /etc/pacman.conf
```

## 7. NVIDIA Runtime Config (Only If NVIDIA)

### 7.1 Turing safety kernel policy

For GTX 16xx / RTX 20xx:

```bash
sudo pacman -S --needed --noconfirm linux-lts linux-lts-headers
if pacman -Q linux >/dev/null 2>&1; then
  sudo pacman -Rdd --noconfirm linux linux-headers
fi
sudo grub-mkconfig -o /boot/grub/grub.cfg
```

### 7.2 Modprobe, udev, grub, initramfs

Use firmware value `0` for Turing, `1` for newer cards.

```bash
FIRMWARE_VAL=1   # set to 0 for Turing

echo "options nvidia NVreg_EnableGpuFirmware=$FIRMWARE_VAL NVreg_DynamicPowerManagement=0x02 NVreg_EnableS0ixPowerManagement=1" | \
  sudo tee /etc/modprobe.d/nvidia.conf >/dev/null

echo "blacklist nvidia_uvm" | \
  sudo tee /etc/modprobe.d/99-nvidia-uvm-blacklist.conf >/dev/null

echo 'SUBSYSTEM=="pci", ATTR{vendor}=="0x10de", ATTR{power/control}="auto"' | \
  sudo tee /etc/udev/rules.d/90-nvidia-pm.rules >/dev/null

sudo sed -i 's/GRUB_CMDLINE_LINUX_DEFAULT="[^\"]*/& nvidia_drm.modeset=1/' /etc/default/grub
```

For modern NVIDIA (non-Turing), also add modules to mkinitcpio:

```bash
sudo sed -i 's/^MODULES=(\(.*\))/MODULES=(\1 nvidia nvidia_modeset nvidia_uvm nvidia_drm)/' /etc/mkinitcpio.conf
```

### 7.3 Generate sway-hybrid wrapper

This mirrors the installer intent (choose the correct iGPU card path yourself if needed):

```bash
sudo tee /usr/local/bin/sway-hybrid >/dev/null <<'EOF'
#!/bin/sh
export __GLX_VENDOR_LIBRARY_NAME=mesa
export VK_ICD_FILENAMES=/usr/share/vulkan/icd.d/intel_icd.x86_64.json
export __EGL_VENDOR_LIBRARY_FILENAMES=/usr/share/glvnd/egl_vendor.d/50_mesa.json
export WLR_DRM_DEVICES=/dev/dri/card0
exec sway
EOF
sudo chmod 755 /usr/local/bin/sway-hybrid
```

Then rebuild:

```bash
sudo mkinitcpio -P
sudo grub-mkconfig -o /boot/grub/grub.cfg
```

## 8. Session Ordering (Greetd List)

Rename and retitle sessions:

```bash
sudo mv -f /usr/share/wayland-sessions/niri.desktop /usr/share/wayland-sessions/10-niri.desktop 2>/dev/null || true
sudo mv -f /usr/share/wayland-sessions/sway.desktop /usr/share/wayland-sessions/20-sway.desktop 2>/dev/null || true
sudo mv -f /usr/share/wayland-sessions/gnome.desktop /usr/share/wayland-sessions/40-gnome.desktop 2>/dev/null || true
sudo mv -f /usr/share/wayland-sessions/gnome-wayland.desktop /usr/share/wayland-sessions/40-gnome-wayland.desktop 2>/dev/null || true

sudo sed -i 's/^Name=.*/Name=1. Niri/' /usr/share/wayland-sessions/10-niri.desktop 2>/dev/null || true
sudo sed -i 's/^Name=.*/Name=2. Sway (Battery)/' /usr/share/wayland-sessions/20-sway.desktop 2>/dev/null || true
sudo sed -i 's/^Name=.*/Name=3. Gnome/' /usr/share/wayland-sessions/40-gnome.desktop 2>/dev/null || true
sudo sed -i 's/^Name=.*/Name=3. Gnome-wayland/' /usr/share/wayland-sessions/40-gnome-wayland.desktop 2>/dev/null || true
```

Set Sway launch command:

```bash
# NVIDIA
sudo sed -i 's|^Exec=.*|Exec=/usr/local/bin/sway-hybrid|' /usr/share/wayland-sessions/20-sway.desktop

# Non-NVIDIA
# sudo sed -i 's|^Exec=.*|Exec=sway|' /usr/share/wayland-sessions/20-sway.desktop
# sudo rm -f /usr/local/bin/sway-hybrid
```

## 9. Dotfiles, Wallpapers, and User Services

Link managed files/directories:

```bash
cd "$HOME/rust-wayland-power"

ln -sf "$PWD/.tmux.conf" "$HOME/.tmux.conf"
ln -sf "$PWD/.profile" "$HOME/.profile"
ln -sf "$PWD/.zshrc" "$HOME/.zshrc"

ln -sfn "$PWD/.config/waybar" "$HOME/.config/waybar"
ln -sfn "$PWD/.config/sway" "$HOME/.config/sway"
ln -sfn "$PWD/.config/hypr" "$HOME/.config/hypr"
ln -sfn "$PWD/.config/niri" "$HOME/.config/niri"
ln -sfn "$PWD/.config/rofi" "$HOME/.config/rofi"
ln -sfn "$PWD/.config/ghostty" "$HOME/.config/ghostty"
ln -sfn "$PWD/.config/fastfetch" "$HOME/.config/fastfetch"
ln -sfn "$PWD/.config/gtk-3.0" "$HOME/.config/gtk-3.0"
ln -sfn "$PWD/.config/gtk-4.0" "$HOME/.config/gtk-4.0"
ln -sfn "$PWD/.config/environment.d" "$HOME/.config/environment.d"
ln -sfn "$PWD/.config/mako" "$HOME/.config/mako"

if [ ! -e "$HOME/.config/nvim" ]; then
  ln -sfn "$PWD/.config/nvim" "$HOME/.config/nvim"
fi

sudo ln -sf "$PWD/tlp.conf" /etc/tlp.conf
sudo systemctl enable tlp.service

mkdir -p "$HOME/Pictures/Wallpapers"
cp -n "$PWD"/wallpapers/* "$HOME/Pictures/Wallpapers/" 2>/dev/null || true
xdg-user-dirs-update
```

Waybar templates:

```bash
mkdir -p "$HOME/.config/waybar"
cp -n "$HOME/.config/waybar/hyprConfig.jsonc.template" "$HOME/.config/waybar/hyprConfig.jsonc" 2>/dev/null || true
cp -n "$HOME/.config/waybar/swayConfig.jsonc.template" "$HOME/.config/waybar/swayConfig.jsonc" 2>/dev/null || true
cp -n "$HOME/.config/waybar/niriConfig.jsonc.template" "$HOME/.config/waybar/niriConfig.jsonc" 2>/dev/null || true
```

Battery daemon user service:

```bash
mkdir -p "$HOME/.config/systemd/user"
cp "$HOME/rust-wayland-power/sysScripts/battery-daemon/battery-daemon.service" "$HOME/.config/systemd/user/battery-daemon.service"
systemctl --user daemon-reload
systemctl --user enable --now battery-daemon.service
```

## 10. Secrets + Geoclue + Generated config.toml

The installer asks for:
- OpenWeatherMap API key
- Finnhub API key
- Google Geolocation API key (optional but recommended)
- Preferred terminal (`ghostty`, `alacritty`, or `kitty`)

Create secure config dir/file:

```bash
mkdir -p "$HOME/.config/rust-dotfiles"
chmod 700 "$HOME/.config/rust-dotfiles"
umask 077
```

If you want to match the installer exactly, run the installer once for the prompt flow:

```bash
cd "$HOME/rust-wayland-power/sysScripts/install-wizard"
cargo build --release
"$HOME/rust-wayland-power/sysScripts/install-wizard/target/release/install-wizard"
```

Or create `~/.config/rust-dotfiles/config.toml` yourself and set mode `600`:

```bash
chmod 600 "$HOME/.config/rust-dotfiles/config.toml"
```

Geoclue key patch (same pattern as installer):

```bash
KEY="YOUR_GOOGLE_KEY"
sudo sed -i 's/^.*enable=true/enable=true/' /etc/geoclue/geoclue.conf
sudo sed -i "s|^.*googleapis.com.*|url=https://www.googleapis.com/geolocation/v1/geolocate?key=$KEY|" /etc/geoclue/geoclue.conf
sudo systemctl restart geoclue.service
```

LibreWolf defaults:

```bash
mkdir -p "$HOME/.librewolf"
cat > "$HOME/.librewolf/librewolf.overrides.cfg" <<'EOF'
defaultPref("network.captive-portal-service.enabled", true);
defaultPref("privacy.resistFingerprinting.letterboxing", false);
defaultPref("privacy.resistFingerprinting", false);
defaultPref("webgl.disabled", false);
defaultPref("privacy.clearOnShutdown.history", false);
defaultPref("privacy.clearOnShutdown.cookies", false);
EOF

xdg-settings set default-web-browser librewolf.desktop
xdg-mime default librewolf.desktop x-scheme-handler/http
xdg-mime default librewolf.desktop x-scheme-handler/https
```

## 11. Finalization Steps

Install tmux plugins:

```bash
if [ -x "$HOME/.tmux/plugins/tpm/bin/install_plugins" ]; then
  "$HOME/.tmux/plugins/tpm/bin/install_plugins"
fi
```

Sync Neovim plugins (if config installed):

```bash
if [ -f "$HOME/.config/nvim/init.lua" ]; then
  nvim --headless "+Lazy! sync" "+qa"
fi
```

Reboot when done:

```bash
sudo reboot
```

## 12. Manual Equivalent of updater

`updater` does more than `yay -Syu`. It runs this sequence:

1. Open your configured terminal and run update command from config (`[updater].update_command`, default `yay -Syu`).
2. If update succeeds, run firmware check/update with `fwupdmgr`.
3. If `$HOME/rust-wayland-power/.git` exists:
   - `git fetch origin main`
   - If `sysScripts` or `pkglist.txt` differ from `origin/main`, force-sync those paths:
     - `git checkout origin/main -- sysScripts pkglist.txt`
4. Build install wizard and refresh machine state:
   - `cd ~/rust-wayland-power/sysScripts/install-wizard`
   - `cargo build --release -q`
   - Copy newer binary to `~/.cargo/bin/install-wizard`
   - Run `~/.cargo/bin/install-wizard --refresh-configs`
5. Send desktop notification for success/failure.

If you want to replicate that manually:

```bash
# 1) Main update
yay -Syu

# 2) Firmware
if command -v fwupdmgr >/dev/null 2>&1; then
  sudo fwupdmgr refresh >/dev/null
  fwupdmgr get-updates || true
  sudo fwupdmgr update || true
fi

# 3) Repo surgical sync
if [ -d "$HOME/rust-wayland-power/.git" ]; then
  cd "$HOME/rust-wayland-power"
  git fetch origin main
  SCRIPTS_DIFF=0
  git diff --quiet origin/main -- sysScripts || SCRIPTS_DIFF=1
  git diff --quiet origin/main -- pkglist.txt || SCRIPTS_DIFF=1
  if [ "$SCRIPTS_DIFF" -eq 1 ]; then
    git checkout origin/main -- sysScripts pkglist.txt
  fi
fi

# 4) Rebuild installer and refresh configs
cd "$HOME/rust-wayland-power/sysScripts/install-wizard"
cargo build --release -q
mkdir -p "$HOME/.cargo/bin"
if [ target/release/install-wizard -nt "$HOME/.cargo/bin/install-wizard" ]; then
  cp target/release/install-wizard "$HOME/.cargo/bin/"
fi
"$HOME/.cargo/bin/install-wizard" --refresh-configs
```

## 13. Fast Path (Recommended)

If you are not debugging anything and just want a working setup:

```bash
cd "$HOME"
git clone https://github.com/Mccalabrese/rust-wayland-power.git
cd rust-wayland-power
bash bootstrap.sh
```

That is still the easiest path, and this manual now matches what your current code actually does.
