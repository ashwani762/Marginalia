#!/usr/bin/env bash
# Installs the built node for the current user and sets it up to start at login.
# Build first: scripts/node/build.sh (or build.sh --cpu-only). Run this from anywhere.
# Pass --models DIR as extra arguments by editing ExecStart in the installed unit if you keep
# the models somewhere other than ~/.local/share/marginalia/models.
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$here/../.." && pwd)"
pack="$root/compute-node/packaging"
bin="$root/target/release/marginalia-node"
tray="$root/target/release/marginalia-node-tray"

if [[ ! -x "$bin" ]]; then
  echo "No binary at $bin. Run scripts/node/build.sh first." >&2
  exit 1
fi

# The tray icon is small and quick to build (pure Rust, no system packages). Build it when it is missing.
if [[ ! -x "$tray" ]] && command -v cargo >/dev/null; then
  (cd "$root" && cargo build --release -p marginalia-node-tray) || echo "Could not build the tray icon, installing the node without it." >&2
fi

mkdir -p "$HOME/.local/bin" "$HOME/.config/systemd/user" "$HOME/.local/share/marginalia/models"
install -m 755 "$bin" "$HOME/.local/bin/marginalia-node"
# The optional speaker helper goes next to the node, which looks for it there. Download its models with: marginalia-node models download --only speakers
if [[ -x "$root/target/release/marginalia-node-speakers" ]]; then
  install -m 755 "$root/target/release/marginalia-node-speakers" "$HOME/.local/bin/marginalia-node-speakers"
fi
if [[ -x "$tray" ]]; then
  mkdir -p "$HOME/.config/autostart"
  install -m 755 "$tray" "$HOME/.local/bin/marginalia-node-tray"
  sed "s|@HOME@|$HOME|g" "$pack/marginalia-node-tray.desktop" > "$HOME/.config/autostart/marginalia-node-tray.desktop"
  chmod 644 "$HOME/.config/autostart/marginalia-node-tray.desktop"
fi
install -m 644 "$pack/marginalia-node.service" "$HOME/.config/systemd/user/marginalia-node.service"

systemctl --user daemon-reload
systemctl --user enable --now marginalia-node.service

echo
echo "Installed to ~/.local/bin/marginalia-node and started as a user service."
if [[ -x "$HOME/.local/bin/marginalia-node-tray" ]]; then
  echo "The tray icon is installed and starts at login (~/.config/autostart/marginalia-node-tray.desktop)."
  echo "Start it now with: ~/.local/bin/marginalia-node-tray &"
  echo "On Ubuntu it shows in the top bar through the AppIndicator extension, which is on by default."
  echo "If you do not see it: sudo apt install gnome-shell-extension-appindicator, then log out and in."
fi
echo "Models go in ~/.local/share/marginalia/models (the node will not start without them)."
echo "Logs:    journalctl --user -u marginalia-node -f"
echo "Pair:    ~/.local/bin/marginalia-node pair"
echo
echo "A user service normally stops when you log out. To keep it running without a login session"
echo "(for example on a headless Pi), run once:"
echo "    sudo loginctl enable-linger $USER"
echo "Also allow UDP 47615 in your firewall (see compute-node/README.md)."
