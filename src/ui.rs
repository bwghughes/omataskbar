//! Layout and drawing. Everything here works in landscape coordinates
//! (x along the bar, y across it); main.rs rotates for the portrait panel.

use crate::{
    config::{Config, ItemConfig},
    stats::{human_bytes, Stats},
    theme::{Rgb, Theme},
};
use cairo::{Context, FontFace, LinearGradient};
use std::f64::consts::PI;

const GAP: f64 = 8.0;
const PAD_Y: f64 = 5.0;
const RADIUS: f64 = 9.0;
const BORDER: f64 = 1.5;

// Nerd Font glyphs
const G_CPU: &str = "\u{F4BC}";
const G_MEM: &str = "\u{EFC5}";
const G_TOP: &str = "\u{F04C5}";
const G_DOWN: &str = "\u{F01DA}";
const G_UP: &str = "\u{F0552}";
const G_POWER: &str = "\u{F140B}";
const G_TEMP: &str = "\u{F050F}";
const G_FAN: &str = "\u{F0210}";
const G_LOAD: &str = "\u{F029A}";
const G_UPTIME: &str = "\u{F051F}";
const G_KBD: &str = "\u{F030C}";
const G_CHARGING: &str = "\u{F0084}";
const G_BATTERY: [&str; 10] = [
    "\u{F007A}", "\u{F007B}", "\u{F007C}", "\u{F007D}", "\u{F007E}",
    "\u{F007F}", "\u{F0080}", "\u{F0081}", "\u{F0082}", "\u{F0079}",
];
const OMARCHY_LOGO: &str = "\u{E900}";

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Widget {
    Cpu,
    Cores,
    Mem,
    Top,
    Net,
    Power,
    Temp,
    Fan,
    Battery,
    Clock,
    Load,
    Uptime,
    Kbd,
}

impl Widget {
    fn parse(s: &str) -> Option<Widget> {
        Some(match s {
            "cpu" => Widget::Cpu,
            "cores" => Widget::Cores,
            "mem" | "memory" => Widget::Mem,
            "top" | "procs" => Widget::Top,
            "net" | "network" => Widget::Net,
            "power" => Widget::Power,
            "temp" => Widget::Temp,
            "fan" => Widget::Fan,
            "battery" => Widget::Battery,
            "clock" | "time" => Widget::Clock,
            "load" => Widget::Load,
            "uptime" => Widget::Uptime,
            "kbd" | "keyboard" => Widget::Kbd,
            _ => return None,
        })
    }

    fn stretch(self) -> f64 {
        match self {
            Widget::Cpu => 2.4,
            Widget::Cores => 1.4,
            Widget::Mem => 2.0,
            Widget::Kbd => 3.0,
            Widget::Top => 2.2,
            Widget::Net => 1.4,
            Widget::Power => 1.3,
            Widget::Battery => 1.1,
            Widget::Clock => 1.2,
            Widget::Uptime => 1.2,
            Widget::Temp | Widget::Fan | Widget::Load => 1.0,
        }
    }
}

enum Kind {
    Widget(Widget),
    Glyph(String),
    Text(String),
    Logo,
    Spacer,
}

pub struct Item {
    kind: Kind,
    pub cfg: ItemConfig,
    pub x: f64,
    pub w: f64,
    pub pressed: bool,
}

impl Item {
    pub fn tappable(&self) -> bool {
        !self.cfg.action.is_empty() || self.cfg.layer.is_some()
    }
}

pub struct Layer {
    pub name: String,
    pub items: Vec<Item>,
}

impl Layer {
    pub fn build(name: &str, cfgs: &[ItemConfig], width: f64) -> Layer {
        let mut items: Vec<Item> = cfgs
            .iter()
            .map(|c| {
                let kind = if let Some(w) = &c.widget {
                    match Widget::parse(w) {
                        Some(w) => Kind::Widget(w),
                        None => Kind::Text(format!("?{w}")),
                    }
                } else if c.logo {
                    Kind::Logo
                } else if let Some(g) = &c.glyph {
                    Kind::Glyph(g.clone())
                } else if let Some(t) = &c.text {
                    Kind::Text(t.clone())
                } else {
                    Kind::Spacer
                };
                Item { kind, cfg: c.clone(), x: 0.0, w: 0.0, pressed: false }
            })
            .collect();
        let stretch = |i: &Item| {
            i.cfg.stretch.filter(|s| *s > 0.0).unwrap_or(match i.kind {
                Kind::Widget(w) => w.stretch(),
                _ => 1.0,
            })
        };
        let total: f64 = items.iter().map(stretch).sum();
        let unit = (width - GAP * (items.len() as f64 - 1.0)) / total;
        let mut x = 0.0f64;
        for it in &mut items {
            it.w = (stretch(it) * unit).floor();
            it.x = x.round();
            x += it.w + GAP;
        }
        // Give rounding leftovers to the last item so the right edge lines up.
        if let Some(last) = items.last_mut() {
            last.w = width - last.x;
        }
        Layer { name: name.into(), items }
    }

    pub fn hit(&self, x: f64) -> Option<usize> {
        // Gaps belong to the nearest item so fat fingers still land.
        self.items
            .iter()
            .position(|i| x >= i.x - GAP / 2.0 && x < i.x + i.w + GAP / 2.0)
            .filter(|i| self.items[*i].tappable())
    }
}

pub struct Painter<'a> {
    pub c: &'a Context,
    pub cfg: &'a Config,
    pub theme: &'a Theme,
    pub stats: &'a Stats,
    pub height: f64,
}

fn set(c: &Context, col: Rgb) {
    c.set_source_rgb(col.0, col.1, col.2);
}

fn set_a(c: &Context, col: Rgb, a: f64) {
    c.set_source_rgba(col.0, col.1, col.2, a);
}

fn rrect(c: &Context, x: f64, y: f64, w: f64, h: f64, r: f64) {
    let r = r.min(w / 2.0).min(h / 2.0);
    c.new_sub_path();
    c.arc(x + w - r, y + r, r, -PI / 2.0, 0.0);
    c.arc(x + w - r, y + h - r, r, 0.0, PI / 2.0);
    c.arc(x + r, y + h - r, r, PI / 2.0, PI);
    c.arc(x + r, y + r, r, PI, 1.5 * PI);
    c.close_path();
}

#[derive(Clone, Copy)]
enum Align {
    Left,
    Center,
    Right,
}

impl<'a> Painter<'a> {
    fn level(&self, v: f64) -> Rgb {
        if v >= 0.85 {
            self.theme.crit
        } else if v >= 0.6 {
            self.theme.warn
        } else {
            self.theme.accent
        }
    }

    /// Text vertically centred on `cy` by font metrics, so digits don't jump.
    fn text(&self, face: &FontFace, size: f64, s: &str, x: f64, cy: f64, align: Align) -> f64 {
        let c = self.c;
        c.set_font_face(face);
        c.set_font_size(size);
        let fe = c.font_extents().unwrap();
        let w = c.text_extents(s).unwrap().x_advance();
        let x = match align {
            Align::Left => x,
            Align::Center => x - w / 2.0,
            Align::Right => x - w,
        };
        c.move_to(x.round(), (cy + (fe.ascent() - fe.descent()) / 2.0).round());
        c.show_text(s).unwrap();
        w
    }

    /// Icon centred on its ink box, since Nerd Font glyphs sit unevenly.
    fn glyph(&self, face: &FontFace, size: f64, s: &str, cx: f64, cy: f64) -> f64 {
        let c = self.c;
        c.set_font_face(face);
        c.set_font_size(size);
        let e = c.text_extents(s).unwrap();
        c.move_to(
            (cx - e.x_bearing() - e.width() / 2.0).round(),
            (cy - e.y_bearing() - e.height() / 2.0).round(),
        );
        c.show_text(s).unwrap();
        e.x_advance()
    }

    fn width_of(&self, size: f64, s: &str) -> f64 {
        self.c.set_font_face(&self.cfg.font);
        self.c.set_font_size(size);
        self.c.text_extents(s).unwrap().x_advance()
    }

    pub fn background(&self, width: f64) {
        set(self.c, self.theme.bg);
        self.c.rectangle(0.0, 0.0, width, self.height);
        self.c.fill().unwrap();
    }

    pub fn item(&self, it: &Item) {
        let c = self.c;
        let (x, y, w, h) = (it.x, PAD_Y, it.w, self.height - 2.0 * PAD_Y);
        let t = self.theme;
        match &it.kind {
            Kind::Spacer => {}
            Kind::Widget(wd) => {
                rrect(c, x, y, w, h, RADIUS);
                set(c, t.surface.mix(t.bg, 0.45));
                c.fill_preserve().unwrap();
                set_a(c, if it.pressed { t.accent } else { t.muted }, if it.pressed { 1.0 } else { 0.8 });
                c.set_line_width(BORDER);
                c.stroke().unwrap();
                c.save().unwrap();
                rrect(c, x, y, w, h, RADIUS);
                c.clip();
                self.widget(*wd, x, y, w, h);
                c.restore().unwrap();
            }
            kind => {
                rrect(c, x, y, w, h, RADIUS);
                set(c, if it.pressed { t.accent } else { t.surface });
                c.fill_preserve().unwrap();
                set_a(c, if it.pressed { t.accent } else { t.muted }, 0.9);
                c.set_line_width(BORDER);
                c.stroke().unwrap();
                let ink = if it.pressed { t.bg } else if it.cfg.layer.is_some() { t.accent } else { t.fg };
                set(c, ink);
                let (cx, cy) = (x + w / 2.0, y + h / 2.0);
                match kind {
                    Kind::Glyph(g) => {
                        self.glyph(&self.cfg.font, 26.0, g, cx, cy);
                    }
                    Kind::Logo => {
                        self.glyph(&self.cfg.logo_font, 24.0, OMARCHY_LOGO, cx, cy);
                    }
                    Kind::Text(s) => {
                        self.text(&self.cfg.font, 20.0, s, cx, cy, Align::Center);
                    }
                    _ => {}
                }
            }
        }
    }

    fn icon(&self, g: &str, x: f64, cy: f64, col: Rgb) -> f64 {
        set(self.c, col);
        self.glyph(&self.cfg.font, 22.0, g, x + 14.0, cy);
        x + 30.0
    }

    /// Filled line graph, newest sample at the right edge. `max` fixes the
    /// scale; None autoscales to the data.
    #[allow(clippy::too_many_arguments)]
    fn sparkline(&self, data: &[f64], x: f64, y: f64, w: f64, h: f64, col: Rgb, fill_a: f64, max: Option<f64>) {
        if data.len() < 2 {
            return;
        }
        let c = self.c;
        let max = max.unwrap_or_else(|| data.iter().copied().fold(0.0, f64::max)).max(1e-9);
        let n = data.len();
        let step = w / (crate::stats::HISTORY - 1) as f64;
        let px = |i: usize| x + w - (n - 1 - i) as f64 * step;
        let py = |v: f64| y + h - (v / max).clamp(0.0, 1.0) * h;
        c.move_to(px(0), py(data[0]));
        for (i, v) in data.iter().enumerate().skip(1) {
            c.line_to(px(i), py(*v));
        }
        let line = c.copy_path().unwrap();
        c.line_to(px(n - 1), y + h);
        c.line_to(px(0), y + h);
        c.close_path();
        let g = LinearGradient::new(0.0, y, 0.0, y + h);
        g.add_color_stop_rgba(0.0, col.0, col.1, col.2, fill_a);
        g.add_color_stop_rgba(1.0, col.0, col.1, col.2, 0.0);
        c.set_source(&g).unwrap();
        c.fill().unwrap();
        c.new_path();
        c.append_path(&line);
        set(c, col);
        c.set_line_width(1.8);
        c.set_line_join(cairo::LineJoin::Round);
        c.stroke().unwrap();
    }

    fn bar(&self, x: f64, y: f64, w: f64, h: f64, v: f64, col: Rgb) {
        let c = self.c;
        rrect(c, x, y, w, h, h / 2.0);
        set_a(c, self.theme.muted, 0.55);
        c.fill().unwrap();
        let fw = (w * v.clamp(0.0, 1.0)).max(h);
        rrect(c, x, y, fw, h, h / 2.0);
        let g = LinearGradient::new(x, 0.0, x + fw, 0.0);
        let dim = col.mix(self.theme.bg, 0.45);
        g.add_color_stop_rgb(0.0, dim.0, dim.1, dim.2);
        g.add_color_stop_rgb(1.0, col.0, col.1, col.2);
        c.set_source(&g).unwrap();
        c.fill().unwrap();
    }

    fn widget(&self, wd: Widget, x: f64, y: f64, w: f64, h: f64) {
        let s = self.stats;
        let t = self.theme;
        let f = &self.cfg.font;
        let cy = y + h / 2.0;
        let (top, bot) = (y + h * 0.3, y + h * 0.72);
        let right = x + w - 10.0;
        match wd {
            Widget::Cpu => {
                let col = self.level(s.cpu);
                let gx = self.icon(G_CPU, x, cy, col);
                let label = format!("{:>3.0}%", s.cpu * 100.0);
                let tw = self.width_of(20.0, " 100%");
                set(self.c, t.fg);
                self.text(f, 20.0, &label, right, cy, Align::Right);
                let hist: Vec<f64> = s.cpu_history.iter().copied().collect();
                let (sx, sw) = (gx + 2.0, right - tw - gx - 4.0);
                self.sparkline(&hist, sx, y + 7.0, sw, h - 12.0, col, 0.45, Some(1.0));
            }
            Widget::Cores => {
                let n = s.cores.len().max(1);
                let inner = w - 20.0;
                let gap = 4.0;
                let bw = (inner - gap * (n as f64 - 1.0)) / n as f64;
                let (by, bh) = (y + 8.0, h - 16.0);
                for (i, v) in s.cores.iter().enumerate() {
                    let bx = x + 10.0 + i as f64 * (bw + gap);
                    rrect(self.c, bx, by, bw, bh, 2.5);
                    set_a(self.c, t.muted, 0.45);
                    self.c.fill().unwrap();
                    let fh = (bh * v).max(3.0);
                    rrect(self.c, bx, by + bh - fh, bw, fh, 2.5);
                    set(self.c, self.level(*v));
                    self.c.fill().unwrap();
                }
            }
            Widget::Mem => {
                let frac = if s.mem_total > 0 { s.mem_used as f64 / s.mem_total as f64 } else { 0.0 };
                let col = self.level(frac);
                let gx = self.icon(G_MEM, x, cy, col);
                set(self.c, t.fg);
                let used = format!("{}/{}", human_bytes(s.mem_used as f64), human_bytes(s.mem_total as f64));
                self.text(f, 15.0, &used, gx + 2.0, top, Align::Left);
                set(self.c, t.dim);
                self.text(f, 15.0, &format!("{:.0}%", frac * 100.0), right, top, Align::Right);
                self.bar(gx + 2.0, bot - 4.0, right - gx - 2.0, 8.0, frac, col);
            }
            Widget::Top => {
                let gx = self.icon(G_TOP, x, cy, t.accent);
                let rows = [top, bot];
                for (i, p) in s.top.iter().take(2).enumerate() {
                    let pct = format!("{:.0}%", p.cpu);
                    set(self.c, if i == 0 { self.level(p.cpu / 100.0) } else { t.dim });
                    let pw = self.text(f, 15.0, &pct, right, rows[i], Align::Right);
                    set(self.c, if i == 0 { t.fg } else { t.dim });
                    let room = right - pw - 8.0 - gx;
                    let name = self.ellipsize(&p.name, 15.0, room);
                    self.text(f, 15.0, &name, gx + 2.0, rows[i], Align::Left);
                }
            }
            Widget::Net => {
                let hist: Vec<f64> = s.net_history.iter().copied().collect();
                self.sparkline(&hist, x, y + 10.0, w, h - 10.0, t.muted, 0.5, None);
                let gx = x + 8.0;
                set(self.c, t.accent);
                self.glyph(f, 15.0, G_DOWN, gx + 8.0, top);
                set(self.c, t.fg);
                self.text(f, 15.0, &format!("{}/s", human_bytes(s.rx)), right, top, Align::Right);
                set(self.c, t.dim);
                self.glyph(f, 15.0, G_UP, gx + 8.0, bot);
                self.text(f, 15.0, &format!("{}/s", human_bytes(s.tx)), right, bot, Align::Right);
            }
            Widget::Power => {
                let watts = s.power_w.map(|p| format!("{p:.1}W")).unwrap_or("--".into());
                set(self.c, t.accent);
                self.glyph(f, 15.0, G_POWER, x + 16.0, top);
                set(self.c, t.fg);
                self.text(f, 16.0, &watts, right, top, Align::Right);
                let temp = s.temp_c.map(|c| format!("{c:.0}°")).unwrap_or("--".into());
                let tcol = s.temp_c.map(|c| self.level((c - 30.0) / 60.0)).unwrap_or(t.dim);
                set(self.c, tcol);
                self.glyph(f, 15.0, G_TEMP, x + 16.0, bot);
                set(self.c, t.dim);
                self.text(f, 15.0, &temp, right, bot, Align::Right);
            }
            Widget::Temp => {
                let v = s.temp_c.unwrap_or(0.0);
                let gx = self.icon(G_TEMP, x, cy, self.level((v - 30.0) / 60.0));
                set(self.c, t.fg);
                self.text(f, 19.0, &s.temp_c.map(|c| format!("{c:.0}°")).unwrap_or("--".into()), (gx + right) / 2.0, cy, Align::Center);
            }
            Widget::Fan => {
                let spinning = s.fan_rpm.is_some_and(|r| r > 0);
                let gx = self.icon(G_FAN, x, cy, if spinning { t.accent } else { t.dim });
                let label = match s.fan_rpm {
                    Some(0) => "off".into(),
                    Some(r) => format!("{r}"),
                    None => "--".into(),
                };
                set(self.c, t.fg);
                self.text(f, 17.0, &label, (gx + right) / 2.0, cy, Align::Center);
            }
            Widget::Load => {
                let gx = self.icon(G_LOAD, x, cy, t.accent);
                set(self.c, t.fg);
                self.text(f, 18.0, &format!("{:.2}", s.load), (gx + right) / 2.0, cy, Align::Center);
            }
            Widget::Uptime => {
                let gx = self.icon(G_UPTIME, x, cy, t.accent);
                let (d, hr, m) = (s.uptime_s / 86400, s.uptime_s / 3600 % 24, s.uptime_s / 60 % 60);
                let label = if d > 0 { format!("{d}d{hr}h") } else { format!("{hr}h{m:02}m") };
                set(self.c, t.fg);
                self.text(f, 17.0, &label, (gx + right) / 2.0, cy, Align::Center);
            }
            Widget::Kbd => {
                let Some(v) = s.kbd else {
                    set(self.c, t.dim);
                    self.text(f, 16.0, "no keyboard backlight", x + w / 2.0, cy, Align::Center);
                    return;
                };
                let gx = self.icon(G_KBD, x, cy, t.accent);
                set(self.c, t.fg);
                let pw = self.text(f, 17.0, &format!("{:.0}%", v * 100.0), right, cy, Align::Right);
                self.bar(gx + 6.0, cy - 5.0, right - pw - gx - 18.0, 10.0, v, t.accent);
            }
            Widget::Battery => {
                let Some(b) = s.battery else {
                    set(self.c, t.dim);
                    self.text(f, 16.0, "AC", x + w / 2.0, cy, Align::Center);
                    return;
                };
                let plugged = b.charging || b.full;
                let col = if plugged {
                    t.good
                } else if b.percent <= 15 {
                    t.crit
                } else if b.percent <= 30 {
                    t.warn
                } else {
                    t.accent
                };
                let g = if b.charging { G_CHARGING } else { G_BATTERY[((b.percent as usize).saturating_sub(1) / 10).min(9)] };
                let gx = self.icon(g, x, cy, col);
                set(self.c, t.fg);
                self.text(f, 19.0, &format!("{}%", b.percent), (gx + right) / 2.0, cy, Align::Center);
            }
            Widget::Clock => {
                let (hm, date) = local_time();
                set(self.c, t.fg);
                self.text(f, 21.0, &hm, x + w / 2.0, y + h * 0.36, Align::Center);
                set(self.c, t.accent);
                self.text(f, 13.0, &date, x + w / 2.0, y + h * 0.78, Align::Center);
            }
        }
    }

    fn ellipsize(&self, s: &str, size: f64, room: f64) -> String {
        if self.width_of(size, s) <= room {
            return s.to_string();
        }
        let mut out: String = s.to_string();
        while !out.is_empty() {
            out.pop();
            let candidate = format!("{out}…");
            if self.width_of(size, &candidate) <= room {
                return candidate;
            }
        }
        String::new()
    }
}

fn local_time() -> (String, String) {
    // SAFETY: time/localtime_r/strftime are called with valid, owned buffers.
    unsafe {
        let now = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&now, &mut tm);
        let fmt = |f: &[u8]| {
            let mut buf = [0u8; 32];
            let n = libc::strftime(buf.as_mut_ptr() as *mut _, buf.len(), f.as_ptr() as *const _, &tm);
            String::from_utf8_lossy(&buf[..n]).into_owned()
        };
        (fmt(b"%H:%M\0"), fmt(b"%a %-d %b\0"))
    }
}
