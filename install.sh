#!/bin/bash
# Build omataskbar and swap it in for tiny-dfr on the Touch Bar.
#
#   ./install.sh              install / update
#   ./install.sh --uninstall  put tiny-dfr back (your /etc/omataskbar config is kept)

set -euo pipefail
cd "$(dirname "$0")"

BIN=/usr/local/bin/omataskbar
UNIT=/etc/systemd/system/omataskbar.service
RULES=/etc/udev/rules.d/99-omataskbar.rules
STATE=/var/lib/omataskbar
CFG_DIR=/etc/omataskbar
HOOK="${XDG_CONFIG_HOME:-$HOME/.config}/omarchy/hooks/theme-set.d/omataskbar"
BINDINGS="${XDG_CONFIG_HOME:-$HOME/.config}/hypr/bindings.lua"

# Run a bash snippet as root in one go: sudo when there's a terminal to ask
# for the password, otherwise pkexec (graphical prompt from the shell's agent).
as_root() {
  local script=$1
  shift
  if [[ -t 0 ]] || sudo -n true 2>/dev/null; then
    sudo bash -c "$script" omataskbar-install "$@"
  else
    pkexec bash -c "$script" omataskbar-install "$@"
  fi
}

if [[ ${1:-} == --uninstall ]]; then
  rm -f "$HOOK"
  as_root '
    systemctl disable --now omataskbar.service 2>/dev/null || true
    rm -f "$1" "$2" "$3"
    rm -rf "$4"
    systemctl unmask tiny-dfr.service
    systemctl daemon-reload
    udevadm control --reload
    systemctl restart tiny-dfr.service
  ' "$BIN" "$UNIT" "$RULES" "$STATE"
  echo "omataskbar removed, tiny-dfr is back. (Config left in $CFG_DIR; XF86Launch1 bind left in $BINDINGS)"
  exit 0
fi

cargo build --release
./target/release/omataskbar check

as_root '
  set -e
  install -Dm755 -o root -g root "$1" "$2"
  install -Dm644 "$8/dist/omataskbar.service" "$3"
  install -Dm644 "$8/dist/99-omataskbar.rules" "$4"
  install -d -m755 -o "$6" -g "$6" "$5"
  install -d -m755 "$7"
  if [[ ! -e "$7/config.toml" ]]; then
    printf "%s\n" \
      "# omataskbar overrides. The full defaults, with comments, are in" \
      "# $8/dist/config.toml - copy what you want to change here." \
      "# Saved changes apply to the Touch Bar immediately." \
      "" >"$7/config.toml"
    chmod 644 "$7/config.toml"
  fi
  systemctl daemon-reload
  udevadm control --reload
  systemctl mask tiny-dfr.service
  systemctl stop tiny-dfr.service || true
  systemctl enable omataskbar.service
  systemctl restart omataskbar.service
' "$PWD/target/release/omataskbar" "$BIN" "$UNIT" "$RULES" "$STATE" "$USER" "$CFG_DIR" "$PWD"

# Theme colours: copy now, and again on every `omarchy theme set`.
install -Dm755 dist/theme-hook.sh "$HOOK"
"$HOOK"

# Stat widgets send Prog1 (XF86Launch1): open btop.
if [[ -f $BINDINGS ]] && ! grep -q XF86Launch1 "$BINDINGS"; then
  cat >>"$BINDINGS" <<'EOF'

-- omataskbar: tapping a stat on the Touch Bar sends XF86Launch1.
o.bind("XF86Launch1", "Activity", { tui = "btop", focus = true })
EOF
  hyprctl reload >/dev/null 2>&1 || true
fi

echo "omataskbar installed. Logs: journalctl -u omataskbar -f"
