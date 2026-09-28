//! Colours come from the active Omarchy theme's colors.toml. The daemon runs
//! as root with /home hidden, so a theme-set hook copies that file to
//! /var/lib/omataskbar/colors.toml, where we pick it up (and watch it).

use std::{collections::HashMap, fs};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rgb(pub f64, pub f64, pub f64);

impl Rgb {
    fn parse(s: &str) -> Option<Rgb> {
        let h = s.trim().strip_prefix('#')?;
        if h.len() < 6 {
            return None;
        }
        let c = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).ok().map(|v| v as f64 / 255.0);
        Some(Rgb(c(0)?, c(2)?, c(4)?))
    }

    pub fn mix(self, other: Rgb, t: f64) -> Rgb {
        Rgb(
            self.0 + (other.0 - self.0) * t,
            self.1 + (other.1 - self.1) * t,
            self.2 + (other.2 - self.2) * t,
        )
    }
}

#[derive(Clone, Debug)]
pub struct Theme {
    pub bg: Rgb,
    pub surface: Rgb,
    pub muted: Rgb,
    pub fg: Rgb,
    pub dim: Rgb,
    pub accent: Rgb,
    pub warn: Rgb,
    pub crit: Rgb,
    pub good: Rgb,
}

impl Default for Theme {
    // Tokyo Night, Omarchy's default.
    fn default() -> Theme {
        Theme::from_map(&HashMap::new())
    }
}

impl Theme {
    pub fn load(path: &str) -> Theme {
        let map = fs::read_to_string(path)
            .ok()
            .and_then(|s| toml::from_str::<HashMap<String, toml::Value>>(&s).ok())
            .map(|m| {
                m.into_iter()
                    .filter_map(|(k, v)| Some((k, Rgb::parse(v.as_str()?)?)))
                    .collect()
            })
            .unwrap_or_default();
        Theme::from_map(&map)
    }

    fn from_map(m: &HashMap<String, Rgb>) -> Theme {
        let get = |keys: &[&str], fallback: &str| {
            keys.iter()
                .find_map(|k| m.get(*k).copied())
                .unwrap_or_else(|| Rgb::parse(fallback).unwrap())
        };
        let bg = get(&["background"], "#1a1b26");
        let fg = get(&["foreground"], "#a9b1d6");
        Theme {
            bg: get(&["darker_background", "dark_background"], "#16161e").mix(Rgb(0.0, 0.0, 0.0), 0.35),
            surface: get(&["lighter_background", "selection_background"], "#24283b").mix(bg, 0.25),
            muted: get(&["muted", "selection", "color8"], "#414868"),
            dim: get(&["dark_foreground", "color8"], "#565f89"),
            accent: get(&["accent", "blue", "color4"], "#7aa2f7"),
            warn: get(&["yellow", "color3"], "#e0af68"),
            crit: get(&["red", "color1"], "#f7768e"),
            good: get(&["green", "color2"], "#9ece6a"),
            fg,
        }
    }
}
