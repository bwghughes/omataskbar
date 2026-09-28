//! Configuration: the built-in default (dist/config.toml, compiled in) with
//! /etc/omataskbar/config.toml layered on top. Top-level keys in the user file
//! win; `[Layers]` entries replace the built-in layer of the same name.

use crate::fonts::{FontConfig, Pattern};
use anyhow::{anyhow, bail, Result};
use cairo::FontFace;
use freetype::Library as FtLibrary;
use input_linux::Key;
use serde::{
    de::{self, Visitor},
    Deserialize, Deserializer,
};
use std::{collections::HashMap, fmt, fs};

pub const USER_CFG_DIR: &str = "/etc/omataskbar";
pub const USER_CFG_PATH: &str = "/etc/omataskbar/config.toml";
pub const DEFAULT_CFG: &str = include_str!("../dist/config.toml");

pub struct Config {
    pub font: FontFace,
    pub logo_font: FontFace,
    pub theme_path: String,
    pub adaptive_brightness: bool,
    pub active_brightness: u32,
    pub default_layer: usize,
    pub fn_layer: usize,
    pub layer_timeout_s: u64,
    pub layers: Vec<LayerConfig>,
}

pub struct LayerConfig {
    pub name: String,
    pub items: Vec<ItemConfig>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "PascalCase", deny_unknown_fields)]
struct Proxy {
    font: Option<String>,
    theme: Option<String>,
    adaptive_brightness: Option<bool>,
    active_brightness: Option<u32>,
    default_layer: Option<String>,
    fn_layer: Option<String>,
    layer_timeout: Option<u64>,
    #[serde(default)]
    layers: HashMap<String, Vec<ItemConfig>>,
    #[serde(default)]
    layer_order: Vec<String>,
}

#[derive(Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "PascalCase", deny_unknown_fields)]
pub struct ItemConfig {
    /// Live stat: cpu, cores, mem, top, net, power, temp, fan, battery, clock, load, uptime
    pub widget: Option<String>,
    /// Nerd Font glyph or short label drawn centred on a button.
    pub glyph: Option<String>,
    pub text: Option<String>,
    /// Draw the Omarchy logo (from the omarchy font).
    #[serde(default)]
    pub logo: bool,
    /// Key(s) sent while pressed, e.g. "VolumeUp" or ["LeftCtrl", "C"].
    #[serde(deserialize_with = "array_or_single", default)]
    pub action: Vec<Key>,
    /// Switch to this layer when tapped (tapping again returns to the default).
    pub layer: Option<String>,
    /// Relative width. Buttons default to 1, widgets to their own size.
    pub stretch: Option<f64>,
}

fn array_or_single<'de, D>(deserializer: D) -> Result<Vec<Key>, D::Error>
where
    D: Deserializer<'de>,
{
    struct ArrayOrSingle;

    impl<'de> Visitor<'de> for ArrayOrSingle {
        type Value = Vec<Key>;

        fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
            f.write_str("string or array of strings")
        }

        fn visit_str<E: de::Error>(self, value: &str) -> Result<Vec<Key>, E> {
            Ok(vec![Deserialize::deserialize(de::value::BorrowedStrDeserializer::new(value))?])
        }

        fn visit_seq<A: de::SeqAccess<'de>>(self, seq: A) -> Result<Vec<Key>, A::Error> {
            Deserialize::deserialize(de::value::SeqAccessDeserializer::new(seq))
        }
    }

    deserializer.deserialize_any(ArrayOrSingle)
}

pub fn load_font(name: &str) -> Result<FontFace> {
    let fontconfig = FontConfig::new();
    let mut pattern = Pattern::new(name);
    fontconfig.perform_substitutions(&mut pattern);
    let pat_match = fontconfig
        .match_pattern(&pattern)
        .map_err(|_| anyhow!("no font matches {name:?}"))?;
    let ft = FtLibrary::init()?;
    let face = ft.new_face(pat_match.get_file_name(), pat_match.get_font_index())?;
    Ok(FontFace::create_from_ft(&face)?)
}

fn parse(src: &str, what: &str) -> Result<Proxy> {
    toml::from_str(src).map_err(|e| anyhow!("{what}: {e}"))
}

pub fn load(user_path: Option<&str>) -> Result<Config> {
    let mut base = parse(DEFAULT_CFG, "built-in config")?;
    let user_path = user_path.unwrap_or(USER_CFG_PATH);
    if let Ok(src) = fs::read_to_string(user_path) {
        let user = parse(&src, user_path)?;
        base.font = user.font.or(base.font);
        base.theme = user.theme.or(base.theme);
        base.adaptive_brightness = user.adaptive_brightness.or(base.adaptive_brightness);
        base.active_brightness = user.active_brightness.or(base.active_brightness);
        base.default_layer = user.default_layer.or(base.default_layer);
        base.fn_layer = user.fn_layer.or(base.fn_layer);
        base.layer_timeout = user.layer_timeout.or(base.layer_timeout);
        for name in user.layer_order {
            if !base.layer_order.contains(&name) {
                base.layer_order.push(name);
            }
        }
        base.layers.extend(user.layers);
    }

    let mut names = base.layer_order.clone();
    let mut extra: Vec<_> = base.layers.keys().filter(|k| !names.contains(k)).cloned().collect();
    extra.sort();
    names.extend(extra);
    let mut layers = Vec::new();
    for name in names {
        if let Some(items) = base.layers.remove(&name) {
            if items.is_empty() {
                bail!("layer {name:?} has no items");
            }
            layers.push(LayerConfig { name, items });
        }
    }
    let find = |n: &Option<String>, what: &str| -> Result<usize> {
        let n = n.as_deref().ok_or_else(|| anyhow!("{what} is not set"))?;
        layers.iter().position(|l| l.name == n).ok_or_else(|| anyhow!("{what} {n:?} is not a layer"))
    };
    for l in &layers {
        for it in &l.items {
            if let Some(target) = &it.layer {
                if !layers.iter().any(|l| &l.name == target) {
                    bail!("layer {:?}: button switches to unknown layer {target:?}", l.name);
                }
            }
        }
    }

    Ok(Config {
        font: load_font(base.font.as_deref().unwrap_or("JetBrainsMono Nerd Font:bold"))?,
        logo_font: load_font("omarchy").or_else(|_| load_font(":bold"))?,
        theme_path: base.theme.unwrap_or_else(|| "/var/lib/omataskbar/colors.toml".into()),
        adaptive_brightness: base.adaptive_brightness.unwrap_or(true),
        active_brightness: base.active_brightness.unwrap_or(128),
        default_layer: find(&base.default_layer, "DefaultLayer")?,
        fn_layer: find(&base.fn_layer, "FnLayer")?,
        layer_timeout_s: base.layer_timeout.unwrap_or(10),
        layers,
    })
}
