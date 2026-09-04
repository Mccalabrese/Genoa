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
  before-sleep 'sh -c "flock -n /tmp/gtklock.lock gtklock &"'
