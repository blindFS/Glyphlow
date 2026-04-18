use objc2::rc::Retained;
use objc2_app_kit::NSFont;
use objc2_core_foundation::CFRetained;
use objc2_core_graphics::CGColor;
use objc2_foundation::NSString;
use rdev::Key;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[derive(Serialize, Deserialize, Debug)]
pub struct GlyphlowTheme {
    #[serde(with = "nsfont_format")]
    pub hint_font: Retained<NSFont>,
    pub hint_margin_size: u8,
    #[serde(with = "cgcolor_format")]
    pub hint_bg_color: CFRetained<CGColor>,
    #[serde(with = "cgcolor_format")]
    pub hint_fg_color: CFRetained<CGColor>,
    #[serde(with = "cgcolor_format")]
    pub hint_hl_color: CFRetained<CGColor>,
    #[serde(with = "nsfont_format")]
    pub menu_font: Retained<NSFont>,
    pub menu_margin_size: u8,
    #[serde(with = "cgcolor_format")]
    pub menu_bg_color: CFRetained<CGColor>,
    #[serde(with = "cgcolor_format")]
    pub menu_fg_color: CFRetained<CGColor>,
}

impl Default for GlyphlowTheme {
    fn default() -> Self {
        GlyphlowTheme {
            hint_font: NSFont::fontWithName_size(&NSString::from_str("Andale Mono"), 12.0)
                .expect("Default font should exist."),
            hint_margin_size: 3,
            hint_bg_color: CGColor::new_generic_rgb(1.0, 1.0, 1.0, 0.8),
            hint_fg_color: CGColor::new_generic_rgb(0.1, 0.1, 0.1, 1.0),
            hint_hl_color: CGColor::new_generic_rgb(1.0, 1.0, 1.0, 0.2),
            menu_font: NSFont::fontWithName_size(&NSString::from_str("Andale Mono"), 12.0)
                .expect("Default font should exist."),
            menu_margin_size: 10,
            menu_bg_color: CGColor::new_generic_rgb(1.0, 1.0, 1.0, 0.8),
            menu_fg_color: CGColor::new_generic_rgb(0.1, 0.1, 0.1, 1.0),
        }
    }
}

#[derive(Serialize, Deserialize, Debug)]
pub struct TextAction {
    pub command: String,
    pub args: Vec<String>,
    pub display: String,
    pub key: char,
}

pub trait AlphabeticKey {
    fn to_char(&self) -> char;
    fn to_str(&self) -> String;
    fn from_str(c: &str) -> Option<Key>;
    fn right_alternative(&self) -> Option<Key>;
}

impl AlphabeticKey for Key {
    fn to_char(&self) -> char {
        match self {
            Key::KeyA => 'A',
            Key::KeyB => 'B',
            Key::KeyC => 'C',
            Key::KeyD => 'D',
            Key::KeyE => 'E',
            Key::KeyF => 'F',
            Key::KeyG => 'G',
            Key::KeyH => 'H',
            Key::KeyI => 'I',
            Key::KeyJ => 'J',
            Key::KeyK => 'K',
            Key::KeyL => 'L',
            Key::KeyM => 'M',
            Key::KeyN => 'N',
            Key::KeyO => 'O',
            Key::KeyP => 'P',
            Key::KeyQ => 'Q',
            Key::KeyR => 'R',
            Key::KeyS => 'S',
            Key::KeyT => 'T',
            Key::KeyU => 'U',
            Key::KeyV => 'V',
            Key::KeyW => 'W',
            Key::KeyX => 'X',
            Key::KeyY => 'Y',
            Key::KeyZ => 'Z',
            Key::Backspace | Key::Delete => '-',
            _ => ' ',
        }
    }

    fn to_str(&self) -> String {
        match self {
            Key::Alt | Key::AltGr => "ALT".to_string(),
            Key::ControlLeft | Key::ControlRight => "CTRL".to_string(),
            Key::MetaLeft | Key::MetaRight => "META".to_string(),
            Key::ShiftLeft | Key::ShiftRight => "SHIFT".to_string(),
            _ => self.to_char().to_string(),
        }
    }

    fn from_str(c: &str) -> Option<Self> {
        match c.to_uppercase().as_str() {
            "A" => Some(Key::KeyA),
            "B" => Some(Key::KeyB),
            "C" => Some(Key::KeyC),
            "D" => Some(Key::KeyD),
            "E" => Some(Key::KeyE),
            "F" => Some(Key::KeyF),
            "G" => Some(Key::KeyG),
            "H" => Some(Key::KeyH),
            "I" => Some(Key::KeyI),
            "J" => Some(Key::KeyJ),
            "K" => Some(Key::KeyK),
            "L" => Some(Key::KeyL),
            "M" => Some(Key::KeyM),
            "N" => Some(Key::KeyN),
            "O" => Some(Key::KeyO),
            "P" => Some(Key::KeyP),
            "Q" => Some(Key::KeyQ),
            "R" => Some(Key::KeyR),
            "S" => Some(Key::KeyS),
            "T" => Some(Key::KeyT),
            "U" => Some(Key::KeyU),
            "V" => Some(Key::KeyV),
            "W" => Some(Key::KeyW),
            "X" => Some(Key::KeyX),
            "Y" => Some(Key::KeyY),
            "Z" => Some(Key::KeyZ),
            "ALT" => Some(Key::Alt),
            "CTRL" => Some(Key::ControlLeft),
            "SHIFT" => Some(Key::ShiftLeft),
            "META" => Some(Key::MetaLeft),
            _ => None,
        }
    }

    fn right_alternative(&self) -> Option<Key> {
        match self {
            Key::Alt => Some(Key::AltGr),
            Key::ControlLeft => Some(Key::ControlRight),
            Key::ShiftLeft => Some(Key::ShiftRight),
            Key::ShiftRight => Some(Key::MetaRight),
            _ => None,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct KeyBinding {
    #[serde(with = "key_combo_format")]
    pub keys: Vec<Key>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct GlyphlowConfig {
    pub global_trigger_key: KeyBinding,
    pub theme: GlyphlowTheme,
    pub text_actions: Vec<TextAction>,
}

impl Default for GlyphlowConfig {
    fn default() -> Self {
        GlyphlowConfig {
            global_trigger_key: KeyBinding {
                keys: vec![Key::Alt, Key::KeyG],
            },
            theme: GlyphlowTheme::default(),
            text_actions: vec![],
        }
    }
}

impl GlyphlowConfig {
    pub fn load_config() -> Self {
        let Some(path) = get_config_path() else {
            return Self::default();
        };

        if let Ok(content) = fs::read_to_string(&path) {
            println!("------------- Loading config from {path:?} -------------");
            match toml::from_str::<Self>(&content) {
                Ok(existing_config) => existing_config,
                Err(e) => {
                    eprintln!(
                        "Failed to parse config file, using default config instead. Error: {e}"
                    );
                    Self::default()
                }
            }
        } else {
            println!("------------- Saving config to {path:?} -------------");
            let default_config = Self::default();
            if let Err(e) = default_config.save_config(&path) {
                eprintln!("Failed to save config file. Error: {e}");
            }
            default_config
        }
    }

    fn save_config(&self, path: &PathBuf) -> Result<(), Box<dyn std::error::Error>> {
        let content = toml::to_string_pretty(self)?;
        fs::write(path, content)?;
        Ok(())
    }
}

fn get_config_path() -> Option<PathBuf> {
    let base_dir = std::env::var("XDG_CONFIG_HOME")
        .ok()
        .map(|dir| PathBuf::from(dir).join("glyphlow"))?;
    if !base_dir.exists() {
        fs::create_dir_all(&base_dir).ok()?;
    }
    Some(base_dir.join("config.toml"))
}

mod key_combo_format {
    use super::*;
    use serde::{Deserializer, Serializer};

    /// --- Serialization: Vec<Key> -> e.g. "ALT + G" ---
    pub fn serialize<S>(keys: &[Key], serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let s = keys
            .iter()
            .map(|k| k.to_str())
            .collect::<Vec<_>>()
            .join(" + ");
        serializer.serialize_str(&s)
    }

    // --- Deserialization: e.g. "ALT + G" -> Vec<Key> ---
    pub fn deserialize<'de, D>(deserializer: D) -> Result<Vec<Key>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        s.split('+')
            .map(|part| {
                Key::from_str(part.trim())
                    .ok_or_else(|| serde::de::Error::custom(format!("Invalid key: {}", part)))
            })
            .collect()
    }
}

mod cgcolor_format {
    use super::*;
    use serde::{Deserializer, Serializer};

    fn hex_to_rgba(hex: &str) -> Option<(f64, f64, f64, f64)> {
        let hex = hex.trim_start_matches('#');
        let to_float = |i: std::ops::Range<usize>| -> Option<f64> {
            hex.get(i)
                .and_then(|s| u8::from_str_radix(s, 16).ok())
                .map(|iu8| iu8 as f64 / 255.0)
        };
        let r = to_float(0..2)?;
        let g = to_float(2..4)?;
        let b = to_float(4..6)?;
        let a = if hex.len() == 8 { to_float(6..8)? } else { 1.0 };
        Some((r, g, b, a))
    }

    fn cgcolor_from_hex(hex: &str) -> Option<CFRetained<CGColor>> {
        let (r, g, b, a) = hex_to_rgba(hex)?;
        Some(CGColor::new_generic_rgb(r, g, b, a))
    }

    pub fn serialize<S>(color: &CFRetained<CGColor>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        unsafe {
            let ptr = CGColor::components(Some(color));
            if !ptr.is_null() {
                let r = (*ptr.offset(0) * 255.0) as u8;
                let g = (*ptr.offset(1) * 255.0) as u8;
                let b = (*ptr.offset(2) * 255.0) as u8;
                let a = (*ptr.offset(3) * 255.0) as u8;
                let s = format!("#{:02x}{:02x}{:02x}{:02x}", r, g, b, a);
                serializer.serialize_str(&s)
            } else {
                Err(serde::ser::Error::custom(
                    "Failed to convert color {color:?} to hex string.",
                ))
            }
        }
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<CFRetained<CGColor>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        cgcolor_from_hex(&s)
            .ok_or_else(|| serde::de::Error::custom(format!("Invalid color: {}", s)))
    }
}

mod nsfont_format {
    use super::*;
    use serde::{Deserializer, Serializer};

    /// --- Serialization: NSFont -> e.g. "Helvetica:15" ---
    pub fn serialize<S>(font: &Retained<NSFont>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let name = NSFont::fontName(font);
        let size = NSFont::pointSize(font);
        let s = format!("{}:{}", name, size);
        serializer.serialize_str(&s)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Retained<NSFont>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        let mut iter = s.split(':');
        let name = iter
            .next()
            .ok_or_else(|| serde::de::Error::custom("Missing font name."))?
            .trim();
        let size = if let Some(num_str) = iter.next() {
            num_str
                .parse::<f64>()
                .map_err(|e| serde::de::Error::custom(format!("Invalid font size: {e}")))?
        } else {
            NSFont::systemFontSize()
        };
        NSFont::fontWithName_size(&NSString::from_str(name), size).ok_or_else(|| {
            serde::de::Error::custom(format!("Failed to find font with name {name}."))
        })
    }
}

// TODO: tests
