# ~/.profile

# Toolkit Backends
# export GDK_BACKEND=wayland,x11,*
# export QT_QPA_PLATFORM=wayland;xcb
# export CLUTTER_BACKEND=wayland

# XDG Specs
export XDG_MENU_PREFIX=arch-
export XCURSOR_SIZE=24
export XCURSOR_THEME=Adwaita

# QT Variables
export QT_AUTO_SCREEN_SCALE_FACTOR=1
export QT_WAYLAND_DISABLE_WINDOWDECORATION=1
export QT_QPA_PLATFORMTHEME=qt5ct

# Firefox Wayland
export MOZ_ENABLE_WAYLAND=1

# Electron Wayland Hint
export ELECTRON_OZONE_PLATFORM_HINT=auto
export PATH="$HOME/.cargo/bin:$PATH"
