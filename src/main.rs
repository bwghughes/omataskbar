//! omataskbar — an Omarchy-styled Touch Bar daemon for Apple Silicon Macs.
//!
//! The display, backlight and fontconfig plumbing come from tiny-dfr
//! (https://github.com/AsahiLinux/tiny-dfr, MIT); omataskbar replaces its
//! static buttons with live system stats and theme-driven drawing.

use anyhow::Result;
use cairo::{Context, Format, ImageSurface};
use drm::control::ClipRect;
use input::{
    event::{
        device::DeviceEvent,
        keyboard::{KeyState, KeyboardEvent, KeyboardEventTrait},
        touch::{TouchEvent, TouchEventPosition, TouchEventSlot},
        Event, EventTrait,
    },
    Device as InputDevice, Libinput, LibinputInterface,
};
use input_linux::{uinput::UInputHandle, EventKind, Key, SynchronizeKind};
use input_linux_sys::{input_event, input_id, timeval, uinput_setup};
use libc::{c_char, O_ACCMODE, O_RDONLY, O_RDWR, O_WRONLY};
use nix::{
    errno::Errno,
    sys::{
        epoll::{Epoll, EpollCreateFlags, EpollEvent, EpollFlags},
        inotify::{AddWatchFlags, InitFlags, Inotify},
    },
};
use privdrop::PrivDrop;
use std::{
    collections::HashMap,
    fs::{File, OpenOptions},
    os::{
        fd::AsFd,
        unix::{fs::OpenOptionsExt, io::OwnedFd},
    },
    path::Path,
    process::ExitCode,
    thread,
    time::{Duration, Instant},
};

mod backlight;
mod config;
mod display;
mod fonts;
mod stats;
mod theme;
mod ui;

use backlight::BacklightManager;
use config::Config;
use display::DrmBackend;
use stats::Stats;
use theme::Theme;
use ui::{Layer, Painter};

pub const TIMEOUT_MS: i32 = 10 * 1000;
const TICK: Duration = Duration::from_secs(1);

const USAGE: &str = "\
omataskbar - live system stats on the Touch Bar, Omarchy style

usage:
  omataskbar                         run the daemon (as root, from systemd)
  omataskbar preview [options]       render the bar to a PNG instead
      --out FILE                       default: omataskbar.png
      --layer NAME                     default: the configured DefaultLayer
      --config FILE                    default: /etc/omataskbar/config.toml
      --theme FILE                     colors.toml to use instead of the configured one
      --seconds N                      sample for N seconds first (fills graphs), default 3
      --press N                        draw item N as pressed
  omataskbar check [--config FILE]   validate a config file
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        None | Some("run") => daemon(),
        Some("preview") => preview(&args[1..]),
        Some("check") => {
            let path = flag(&args[1..], "--config");
            config::load(path.as_deref()).map(|c| {
                let names: Vec<_> = c.layers.iter().map(|l| l.name.as_str()).collect();
                println!("ok: layers {}", names.join(", "));
            })
        }
        Some("help" | "-h" | "--help") => {
            print!("{USAGE}");
            Ok(())
        }
        Some(other) => Err(anyhow::anyhow!("unknown command {other:?}\n\n{USAGE}")),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("omataskbar: {e:#}");
            ExitCode::FAILURE
        }
    }
}

fn flag(args: &[String], name: &str) -> Option<String> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned()
}

fn build_layers(cfg: &Config, width: f64) -> Vec<Layer> {
    cfg.layers.iter().map(|l| Layer::build(&l.name, &l.items, width)).collect()
}

/// Draw one layer in landscape coordinates onto `c`.
fn paint(c: &Context, cfg: &Config, theme: &Theme, stats: &Stats, layer: &Layer, width: f64, height: f64) {
    let p = Painter { c, cfg, theme, stats, height };
    p.background(width);
    for it in &layer.items {
        p.item(it);
    }
}

fn preview(args: &[String]) -> Result<()> {
    let cfg = config::load(flag(args, "--config").as_deref())?;
    let theme = Theme::load(&flag(args, "--theme").unwrap_or(cfg.theme_path.clone()));
    let (width, height) = (2170.0, 60.0);
    let mut layers = build_layers(&cfg, width);
    let idx = match flag(args, "--layer") {
        Some(n) => layers
            .iter()
            .position(|l| l.name == n)
            .ok_or_else(|| anyhow::anyhow!("no layer {n:?}"))?,
        None => cfg.default_layer,
    };
    if let Some(i) = flag(args, "--press").and_then(|v| v.parse::<usize>().ok()) {
        if let Some(it) = layers[idx].items.get_mut(i) {
            it.pressed = true;
        }
    }
    let secs: u64 = flag(args, "--seconds").and_then(|v| v.parse().ok()).unwrap_or(3);
    let mut stats = Stats::new();
    for _ in 0..secs.max(1) {
        thread::sleep(TICK);
        stats.sample();
    }
    let surface = ImageSurface::create(Format::ARgb32, width as i32, height as i32)?;
    let c = Context::new(&surface)?;
    paint(&c, &cfg, &theme, &stats, &layers[idx], width, height);
    drop(c);
    let out = flag(args, "--out").unwrap_or("omataskbar.png".into());
    surface.write_to_png(&mut File::create(&out)?)?;
    println!("wrote {out}");
    Ok(())
}

struct Interface;

impl LibinputInterface for Interface {
    fn open_restricted(&mut self, path: &Path, flags: i32) -> Result<OwnedFd, i32> {
        let mode = flags & O_ACCMODE;

        OpenOptions::new()
            .custom_flags(flags)
            .read(mode == O_RDONLY || mode == O_RDWR)
            .write(mode == O_WRONLY || mode == O_RDWR)
            .open(path)
            .map(|file| file.into())
            .map_err(|err| err.raw_os_error().unwrap())
    }
    fn close_restricted(&mut self, fd: OwnedFd) {
        _ = File::from(fd);
    }
}

fn emit(uinput: &mut UInputHandle<File>, ty: EventKind, code: u16, value: i32) {
    let ev = input_event { value, type_: ty as u16, code, time: timeval { tv_sec: 0, tv_usec: 0 } };
    if let Err(e) = uinput.write(&[ev]) {
        eprintln!("uinput write failed: {e}");
    }
}

fn toggle_keys(uinput: &mut UInputHandle<File>, codes: &[Key], value: i32) {
    if codes.is_empty() {
        return;
    }
    for kc in codes {
        emit(uinput, EventKind::Key, *kc as u16, value);
    }
    emit(uinput, EventKind::Synchronize, SynchronizeKind::Report as u16, 0);
}

fn make_uinput() -> Result<UInputHandle<File>> {
    let uinput = UInputHandle::new(OpenOptions::new().write(true).open("/dev/uinput")?);
    uinput.set_evbit(EventKind::Key)?;
    // Advertise every keyboard key up front so config reloads can use any of
    // them. Skip the BTN_* block so nothing mistakes us for a mouse.
    for code in (1..=0xffu16).chain(0x160..=0x2bf) {
        if let Ok(k) = Key::from_code(code) {
            let _ = uinput.set_keybit(k);
        }
    }
    let mut name = [0 as c_char; 80];
    for (i, b) in b"omataskbar Virtual Input Device".iter().enumerate() {
        name[i] = *b as c_char;
    }
    uinput.dev_setup(&uinput_setup {
        id: input_id { bustype: 0x19, vendor: 0x1209, product: 0x316E, version: 1 },
        ff_effects_max: 0,
        name,
    })?;
    uinput.dev_create()?;
    Ok(uinput)
}

/// Config (plus fonts) with a fallback to the built-in defaults, so a typo in
/// /etc never leaves the Mac without an Esc key.
fn load_config_or_default() -> Config {
    match config::load(None) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("config error, using built-in defaults: {e:#}");
            config::load(Some("/nonexistent")).expect("built-in config must load")
        }
    }
}

fn daemon() -> Result<()> {
    let mut drm = DrmBackend::open_card()?;
    let (height, width) = drm.mode().size();
    let (db_width, db_height) = drm.fb_info()?.size();
    let (wf, hf) = (width as f64, height as f64);

    let mut uinput = make_uinput()?;
    let mut backlight = BacklightManager::new();
    let mut cfg = load_config_or_default();
    let mut theme = Theme::load(&cfg.theme_path);
    let mut layers = build_layers(&cfg, wf);

    let inotify = Inotify::init(InitFlags::IN_NONBLOCK)?;
    let watch_flags = AddWatchFlags::IN_CLOSE_WRITE | AddWatchFlags::IN_MOVED_TO | AddWatchFlags::IN_CREATE;
    let theme_dir = Path::new(&cfg.theme_path).parent().map(Path::to_path_buf);
    for dir in [Some(Path::new(config::USER_CFG_DIR).to_path_buf()), theme_dir].into_iter().flatten() {
        if let Err(e) = inotify.add_watch(&dir, watch_flags) {
            eprintln!("not watching {}: {e}", dir.display());
        }
    }

    PrivDrop::default()
        .user("nobody")
        .group_list(&["input", "video"])
        .apply()
        .map_err(|e| anyhow::anyhow!("failed to drop privileges: {e}"))?;

    let mut surface = ImageSurface::create(Format::ARgb32, db_width as i32, db_height as i32)?;

    let mut input_tb = Libinput::new_with_udev(Interface);
    let mut input_main = Libinput::new_with_udev(Interface);
    input_tb.udev_assign_seat("seat-touchbar").map_err(|_| anyhow::anyhow!("seat-touchbar"))?;
    input_main.udev_assign_seat("seat0").map_err(|_| anyhow::anyhow!("seat0"))?;
    let epoll = Epoll::new(EpollCreateFlags::empty())?;
    epoll.add(input_main.as_fd(), EpollEvent::new(EpollFlags::EPOLLIN, 0))?;
    epoll.add(input_tb.as_fd(), EpollEvent::new(EpollFlags::EPOLLIN, 1))?;
    epoll.add(inotify.as_fd(), EpollEvent::new(EpollFlags::EPOLLIN, 2))?;

    let mut stats = Stats::new();
    let mut next_tick = Instant::now() + TICK;
    let mut digitizer: Option<InputDevice> = None;
    let mut touches: HashMap<u32, (usize, usize)> = HashMap::new();
    let mut base_layer = cfg.default_layer;
    let mut fn_held = false;
    let mut last_touch = Instant::now();
    let mut needs_redraw = true;
    let mut last_bl = backlight.current_bl();
    // Brightness keys go through Hyprland, so re-read levels shortly after a tap.
    let mut levels_at: Option<Instant> = None;

    loop {
        let now = Instant::now();
        if now >= next_tick {
            stats.sample();
            next_tick += TICK;
            if next_tick < now {
                next_tick = now + TICK;
            }
            needs_redraw = true;
        }
        if levels_at.is_some_and(|t| now >= t) {
            stats.sample_levels();
            levels_at = None;
            needs_redraw = true;
        }
        if base_layer != cfg.default_layer
            && touches.is_empty()
            && now.duration_since(last_touch).as_secs() >= cfg.layer_timeout_s
        {
            base_layer = cfg.default_layer;
            needs_redraw = true;
        }
        let active = if fn_held { cfg.fn_layer } else { base_layer };

        // Nothing to see with the backlight off; keep sampling so the graphs
        // are warm when it comes back.
        let bl = backlight.current_bl();
        if bl != 0 && (needs_redraw || last_bl == 0) {
            let c = Context::new(&surface)?;
            c.translate(hf, 0.0);
            c.rotate(90f64.to_radians());
            paint(&c, &cfg, &theme, &stats, &layers[active], wf, hf);
            drop(c);
            surface.flush();
            let data = surface.data().map_err(|e| anyhow::anyhow!("surface: {e}"))?;
            drm.map()?.as_mut()[..data.len()].copy_from_slice(&data);
            drm.dirty(&[ClipRect::new(0, 0, height, width)])?;
            needs_redraw = false;
        }
        last_bl = bl;

        let wake = levels_at.map_or(next_tick, |t| t.min(next_tick));
        let wait = wake.saturating_duration_since(Instant::now()).as_millis().min(u16::MAX as u128) as u16;
        match epoll.wait(&mut [EpollEvent::new(EpollFlags::EPOLLIN, 0)], wait) {
            Err(Errno::EINTR) | Ok(_) => {}
            Err(e) => return Err(e.into()),
        }

        if let Ok(events) = inotify.read_events() {
            if !events.is_empty() {
                thread::sleep(Duration::from_millis(50)); // let editors finish writing
                while inotify.read_events().is_ok_and(|e| !e.is_empty()) {}
                match config::load(None) {
                    Ok(new) => {
                        release_all(&mut layers, &mut touches, &mut uinput);
                        cfg = new;
                        layers = build_layers(&cfg, wf);
                        base_layer = cfg.default_layer;
                    }
                    Err(e) => eprintln!("config error, keeping the previous one: {e:#}"),
                }
                theme = Theme::load(&cfg.theme_path);
                needs_redraw = true;
            }
        }

        input_tb.dispatch()?;
        input_main.dispatch()?;
        for event in &mut input_tb.clone().chain(input_main.clone()) {
            backlight.process_event(&event);
            match event {
                Event::Device(DeviceEvent::Added(evt)) => {
                    let dev = evt.device();
                    if dev.name().contains(" Touch Bar") {
                        digitizer = Some(dev);
                    }
                }
                Event::Keyboard(KeyboardEvent::Key(key)) if key.key() == Key::Fn as u32 => {
                    let held = key.key_state() == KeyState::Pressed;
                    if held != fn_held {
                        fn_held = held;
                        needs_redraw = true;
                    }
                }
                Event::Touch(te) => {
                    if Some(te.device()) != digitizer || backlight.current_bl() == 0 {
                        continue;
                    }
                    last_touch = Instant::now();
                    let active = if fn_held { cfg.fn_layer } else { base_layer };
                    match te {
                        TouchEvent::Down(dn) => {
                            let x = dn.x_transformed(width as u32);
                            if let Some(i) = layers[active].hit(x) {
                                touches.insert(dn.seat_slot(), (active, i));
                                let it = &mut layers[active].items[i];
                                it.pressed = true;
                                toggle_keys(&mut uinput, &it.cfg.action, 1);
                                needs_redraw = true;
                            }
                        }
                        TouchEvent::Motion(mtn) => {
                            let Some(&(l, i)) = touches.get(&mtn.seat_slot()) else { continue };
                            let x = mtn.x_transformed(width as u32);
                            let inside = layers[l].hit(x) == Some(i);
                            let it = &mut layers[l].items[i];
                            if it.pressed != inside {
                                it.pressed = inside;
                                toggle_keys(&mut uinput, &it.cfg.action, inside as i32);
                                needs_redraw = true;
                            }
                        }
                        TouchEvent::Up(up) => {
                            let Some((l, i)) = touches.remove(&up.seat_slot()) else { continue };
                            let it = &mut layers[l].items[i];
                            if it.pressed {
                                it.pressed = false;
                                toggle_keys(&mut uinput, &it.cfg.action, 0);
                                if !it.cfg.action.is_empty() {
                                    levels_at = Some(Instant::now() + Duration::from_millis(250));
                                }
                                if let Some(target) = &it.cfg.layer {
                                    let t = cfg.layers.iter().position(|x| &x.name == target).unwrap_or(cfg.default_layer);
                                    base_layer = if base_layer == t { cfg.default_layer } else { t };
                                }
                            }
                            needs_redraw = true;
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        }
        backlight.update_backlight(&cfg);
    }
}

fn release_all(layers: &mut [Layer], touches: &mut HashMap<u32, (usize, usize)>, uinput: &mut UInputHandle<File>) {
    for (_, (l, i)) in touches.drain() {
        let it = &mut layers[l].items[i];
        if it.pressed {
            it.pressed = false;
            toggle_keys(uinput, &it.cfg.action, 0);
        }
    }
}
