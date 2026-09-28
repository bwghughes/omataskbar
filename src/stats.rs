//! System sampling straight from /proc and /sys. One `sample()` per tick;
//! rates are computed against the previous tick.

use std::{
    collections::{HashMap, VecDeque},
    fs,
    time::Instant,
};

pub const HISTORY: usize = 60;

#[derive(Default, Clone)]
pub struct Proc {
    pub name: String,
    pub cpu: f64, // percent of one core, like top
}

#[derive(Default, Clone, Copy)]
pub struct Battery {
    pub percent: u32,
    pub charging: bool,
    pub full: bool,
}

#[derive(Default)]
pub struct Stats {
    pub cpu: f64,
    pub cpu_history: VecDeque<f64>,
    pub cores: Vec<f64>,
    pub mem_used: u64,
    pub mem_total: u64,
    pub swap_used: u64,
    pub mem_history: VecDeque<f64>,
    pub top: Vec<Proc>,
    pub procs: usize,
    pub rx: f64,
    pub tx: f64,
    pub net_history: VecDeque<f64>,
    pub power_w: Option<f64>,
    pub temp_c: Option<f64>,
    pub fan_rpm: Option<u32>,
    pub battery: Option<Battery>,
    pub load: f64,
    pub uptime_s: u64,

    prev_cpu: Vec<(u64, u64)>,
    prev_ticks: HashMap<u32, u64>,
    prev_net: Option<(u64, u64)>,
    prev_at: Option<Instant>,
    clk_tck: f64,
    sensors: Sensors,
}

#[derive(Default)]
struct Sensors {
    power: Option<String>,
    temps: Vec<String>,
    fan: Option<String>,
    battery: Option<String>,
}

fn read(path: &str) -> Option<String> {
    fs::read_to_string(path).ok()
}

fn read_num(path: &str) -> Option<f64> {
    read(path)?.trim().parse().ok()
}

fn push(h: &mut VecDeque<f64>, v: f64) {
    if h.len() == HISTORY {
        h.pop_front();
    }
    h.push_back(v);
}

impl Sensors {
    fn discover() -> Sensors {
        let mut s = Sensors::default();
        let Ok(dirs) = fs::read_dir("/sys/class/hwmon") else { return s };
        for d in dirs.flatten() {
            let dir = d.path();
            let dir = dir.to_string_lossy();
            let name = read(&format!("{dir}/name")).unwrap_or_default();
            let Ok(files) = fs::read_dir(&*dir) else { continue };
            for f in files.flatten() {
                let file = f.file_name().to_string_lossy().into_owned();
                let Some(stem) = file.strip_suffix("_label") else { continue };
                let label = read(&format!("{dir}/{file}")).unwrap_or_default();
                let input = format!("{dir}/{stem}_input");
                match label.trim() {
                    "Total System Power" => s.power = Some(input),
                    "Fan" => s.fan = Some(input),
                    _ if stem.starts_with("temp") && name.trim() == "macsmc_hwmon" => s.temps.push(input),
                    _ => {}
                }
            }
        }
        if s.temps.is_empty() {
            s.temps.push("/sys/class/thermal/thermal_zone0/temp".into());
        }
        if let Ok(ps) = fs::read_dir("/sys/class/power_supply") {
            for p in ps.flatten() {
                let path = p.path().to_string_lossy().into_owned();
                if read(&format!("{path}/type")).is_some_and(|t| t.trim() == "Battery") {
                    s.battery = Some(path);
                    break;
                }
            }
        }
        s
    }
}

impl Stats {
    pub fn new() -> Stats {
        // SAFETY: sysconf has no preconditions.
        let clk = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
        let mut s = Stats {
            clk_tck: if clk > 0 { clk as f64 } else { 100.0 },
            sensors: Sensors::discover(),
            ..Default::default()
        };
        s.sample();
        s
    }

    pub fn sample(&mut self) {
        let now = Instant::now();
        let dt = self.prev_at.map(|t| (now - t).as_secs_f64()).filter(|d| *d > 0.05);
        self.prev_at = Some(now);

        self.sample_cpu();
        self.sample_mem();
        self.sample_procs(dt);
        self.sample_net(dt);
        self.sample_sensors();
        if let Some(l) = read("/proc/loadavg") {
            self.load = l.split_whitespace().next().and_then(|v| v.parse().ok()).unwrap_or(0.0);
        }
        if let Some(u) = read("/proc/uptime") {
            self.uptime_s = u.split('.').next().and_then(|v| v.parse().ok()).unwrap_or(0);
        }
    }

    fn sample_cpu(&mut self) {
        let Some(stat) = read("/proc/stat") else { return };
        let rows: Vec<(u64, u64)> = stat
            .lines()
            .take_while(|l| l.starts_with("cpu"))
            .map(|l| {
                let v: Vec<u64> = l.split_whitespace().skip(1).filter_map(|x| x.parse().ok()).collect();
                let idle = v.get(3).unwrap_or(&0) + v.get(4).unwrap_or(&0);
                (v.iter().take(8).sum(), idle)
            })
            .collect();
        let usage = |cur: (u64, u64), prev: (u64, u64)| {
            let total = cur.0.saturating_sub(prev.0) as f64;
            let idle = cur.1.saturating_sub(prev.1) as f64;
            if total > 0.0 { (1.0 - idle / total).clamp(0.0, 1.0) } else { 0.0 }
        };
        if self.prev_cpu.len() == rows.len() {
            self.cpu = usage(rows[0], self.prev_cpu[0]);
            self.cores = rows[1..].iter().zip(&self.prev_cpu[1..]).map(|(c, p)| usage(*c, *p)).collect();
            push(&mut self.cpu_history, self.cpu);
        } else {
            self.cores = vec![0.0; rows.len().saturating_sub(1)];
        }
        self.prev_cpu = rows;
    }

    fn sample_mem(&mut self) {
        let Some(info) = read("/proc/meminfo") else { return };
        let mut f = HashMap::new();
        for l in info.lines() {
            let mut it = l.split_whitespace();
            if let (Some(k), Some(v)) = (it.next(), it.next()) {
                f.insert(k.trim_end_matches(':'), v.parse::<u64>().unwrap_or(0) * 1024);
            }
        }
        let g = |k| f.get(k).copied().unwrap_or(0);
        self.mem_total = g("MemTotal");
        self.mem_used = self.mem_total.saturating_sub(g("MemAvailable"));
        self.swap_used = g("SwapTotal").saturating_sub(g("SwapFree"));
        if self.mem_total > 0 {
            push(&mut self.mem_history, self.mem_used as f64 / self.mem_total as f64);
        }
    }

    fn sample_procs(&mut self, dt: Option<f64>) {
        let Ok(dir) = fs::read_dir("/proc") else { return };
        let mut ticks = HashMap::with_capacity(self.prev_ticks.len());
        let mut by_name: HashMap<String, f64> = HashMap::new();
        for e in dir.flatten() {
            let Some(pid) = e.file_name().to_str().and_then(|s| s.parse::<u32>().ok()) else { continue };
            let Some(stat) = read(&format!("/proc/{pid}/stat")) else { continue };
            // comm can contain spaces and parens; it runs to the last ')'.
            let (Some(open), Some(close)) = (stat.find('('), stat.rfind(')')) else { continue };
            let name = &stat[open + 1..close];
            let rest: Vec<&str> = stat[close + 2..].split(' ').collect();
            let t = rest.get(11).and_then(|v| v.parse::<u64>().ok()).unwrap_or(0)
                + rest.get(12).and_then(|v| v.parse::<u64>().ok()).unwrap_or(0);
            ticks.insert(pid, t);
            if let (Some(dt), Some(prev)) = (dt, self.prev_ticks.get(&pid)) {
                let pct = t.saturating_sub(*prev) as f64 / self.clk_tck / dt * 100.0;
                if pct > 0.0 {
                    *by_name.entry(pretty_name(name)).or_default() += pct;
                }
            }
        }
        self.procs = ticks.len();
        self.prev_ticks = ticks;
        if dt.is_some() {
            let mut top: Vec<Proc> = by_name.into_iter().map(|(name, cpu)| Proc { name, cpu }).collect();
            top.sort_by(|a, b| b.cpu.total_cmp(&a.cpu));
            top.truncate(3);
            self.top = top;
        }
    }

    fn sample_net(&mut self, dt: Option<f64>) {
        let Some(dev) = read("/proc/net/dev") else { return };
        let (mut rx, mut tx) = (0u64, 0u64);
        for l in dev.lines().skip(2) {
            let Some((iface, data)) = l.split_once(':') else { continue };
            let iface = iface.trim();
            if iface == "lo" || iface.starts_with("veth") || iface.starts_with("docker") || iface.starts_with("br-") {
                continue;
            }
            let v: Vec<u64> = data.split_whitespace().filter_map(|x| x.parse().ok()).collect();
            rx += v.first().unwrap_or(&0);
            tx += v.get(8).unwrap_or(&0);
        }
        if let (Some(dt), Some((prx, ptx))) = (dt, self.prev_net) {
            self.rx = rx.saturating_sub(prx) as f64 / dt;
            self.tx = tx.saturating_sub(ptx) as f64 / dt;
            push(&mut self.net_history, self.rx + self.tx);
        }
        self.prev_net = Some((rx, tx));
    }

    fn sample_sensors(&mut self) {
        let s = &self.sensors;
        self.power_w = s.power.as_deref().and_then(read_num).map(|uw| uw / 1e6);
        self.fan_rpm = s.fan.as_deref().and_then(read_num).map(|r| r as u32);
        self.temp_c = s
            .temps
            .iter()
            .filter_map(|p| read_num(p))
            .map(|m| m / 1000.0)
            .filter(|c| *c > 0.0 && *c < 150.0)
            .reduce(f64::max);
        self.battery = s.battery.as_deref().map(|b| {
            let status = read(&format!("{b}/status")).unwrap_or_default();
            Battery {
                percent: read_num(&format!("{b}/capacity")).unwrap_or(0.0) as u32,
                charging: status.trim() == "Charging",
                full: status.trim() == "Full",
            }
        });
    }
}

/// Collapse helper processes into the app people recognise.
fn pretty_name(comm: &str) -> String {
    let base = comm.split([':', '/']).next().unwrap_or(comm);
    for (prefix, app) in [
        ("Isolated Web", "firefox"),
        ("Web Content", "firefox"),
        ("WebExtensions", "firefox"),
        ("chromium", "chromium"),
        ("chrome", "chrome"),
        ("electron", "electron"),
        ("Hyprland", "hyprland"),
    ] {
        if base.starts_with(prefix) {
            return app.into();
        }
    }
    base.to_string()
}

pub fn human_bytes(b: f64) -> String {
    const UNITS: [&str; 5] = ["B", "K", "M", "G", "T"];
    let mut v = b;
    let mut i = 0;
    while v >= 1000.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{v:.0}{}", UNITS[i])
    } else if v < 10.0 {
        format!("{v:.1}{}", UNITS[i])
    } else {
        format!("{v:.0}{}", UNITS[i])
    }
}
