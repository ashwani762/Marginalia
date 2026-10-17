#!/usr/bin/env bash
# Installs the built node for the current user and sets it up to start at login.
# Build first: compute-node/build.sh (or build.sh --cpu-only). Run this from anywhere.
# Pass --models DIR as extra arguments by editing ExecStart in the installed unit if you keep
# the models somewhere other than ~/.local/share/marginalia/models.
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$here/../.." && pwd)"
bin="$root/target/release/marginalia-node"

if [[ ! -x "$bin" ]]; then
  echo "No binary at $bin. Run compute-node/build.sh first." >&2
  exit 1
fi

mkdir -p "$HOME/.local/bin" "$HOME/.config/systemd/user" "$HOME/.local/share/marginalia/models"
install -m 755 "$bin" "$HOME/.local/bin/marginalia-node"
install -m 644 "$here/marginalia-node.service" "$HOME/.config/systemd/user/marginalia-node.service"

systemctl --user daemon-reload
systemctl --user enable --now marginalia-node.service

echo
echo "Installed to ~/.local/bin/marginalia-node and started as a user service."
echo "Models go in ~/.local/share/marginalia/models (the node will not start without them)."
echo "Logs:    journalctl --user -u marginalia-node -f"
echo "Pair:    ~/.local/bin/marginalia-node pair"
echo
echo "A user service normally stops when you log out. To keep it running without a login session"
echo "(for example on a headless Pi), run once:"
echo "    sudo loginctl enable-linger $USER"
echo "Also allow UDP 47615 in your firewall (see compute-node/README.md)."
