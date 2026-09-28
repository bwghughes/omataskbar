# omataskbar

Live system stats on the Apple Silicon Touch Bar, drawn in your Omarchy theme. It is written in Rust and replaces `tiny-dfr`.

![Stats layer](docs/stats.png)

<sub>Controls layer (tap the Omarchy logo)</sub>

![Controls layer](docs/controls.png)

<sub>Holding Fn: keyboard backlight</sub>

![Keyboard backlight](docs/keyboard.png)

<sub>These are real frames from `omataskbar preview`, drawn at the Touch Bar's native 2170×60 in the Hackerman theme.</sub>

- **Stats layer (default).** It shows:
  - screen brightness down/up buttons
  - CPU %, with a 60-second sparkline
  - per-core bars
  - memory
  - the busiest two apps (helper processes are merged into their app)
  - network rates
  - system power and temperature from the SMC
  - battery and clock
  - play/pause and volume buttons

  Tap any stat to open btop (the stats send `XF86Launch1`).
- **Omarchy logo.** Opens the **controls** layer, which has everything the old media layer had: brightness, mic, Omarchy menu, lock, screenshot, night light, media and volume. The bar falls back to stats after 10 s.
- **Hold Fn.** Adjust the keyboard backlight, with a live level bar. Esc is here too, because the M1 has no physical Esc key.
- **Theme.** Colors follow `omarchy theme set` through a theme-set hook. The hook copies `colors.toml` to `/var/lib/omataskbar/`, and the daemon recolors on the next tick.

## Install

```sh
./install.sh              # build, install to /usr/local/bin, mask tiny-dfr, start omataskbar
./install.sh --uninstall  # put tiny-dfr back
```

## Configure

Edit `/etc/omataskbar/config.toml`. Changes apply live, and a broken file is ignored while the daemon keeps running. `dist/config.toml` has the defaults and documents every option. For example, to put load and fan on the stats layer, copy the whole `stats = [...]` block into `[Layers]` and add `{ Widget = "load" }`.

Widgets: `cpu cores mem top net power temp fan battery clock load uptime kbd`.

```sh
omataskbar check                                 # validate the config
omataskbar preview --out bar.png --layer stats   # render to a PNG without the hardware
journalctl -u omataskbar -f
```

## Credits

The DRM, backlight and fontconfig code is adapted from [tiny-dfr](https://github.com/AsahiLinux/tiny-dfr) (MIT, see `dist/LICENSE.tiny-dfr`).
