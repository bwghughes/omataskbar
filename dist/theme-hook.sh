#!/bin/bash
# omataskbar: hand the new theme's colours to the Touch Bar daemon, which
# can't see /home. It watches this file and recolours instantly.
src="${XDG_STATE_HOME:-$HOME/.local/state}/omarchy/current/theme/colors.toml"
[[ -f $src && -w /var/lib/omataskbar ]] || exit 0
cp "$src" /var/lib/omataskbar/colors.toml.tmp && mv /var/lib/omataskbar/colors.toml.tmp /var/lib/omataskbar/colors.toml
