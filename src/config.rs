use crate::os_util::AlphabeticKey;
use rdev::Key;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[derive(Serialize, Deserialize, Debug)]
pub struct GlyphlowTheme {
    pub font: String,
    pub font_size: u8,
    pub margin_size: u8,
    pub hint_radius: u8,
    pub hint_bg_color: String,
    pub hint_fg_color: String,
    pub hint_hl_color: String,
}

impl Default for GlyphlowTheme {
    fn default() -> Self {
        GlyphlowTheme {
            font: "Andale Mono".to_string(),
            font_size: 12,
            margin_size: 3,
            hint_radius: 5,
            hint_bg_color: "#769ff0A0".to_string(),
            hint_fg_color: "#FFFFFFFF".to_string(),
            hint_hl_color: "#FFFFFF20".to_string(),
        }
    }
}

#[derive(Serialize, Deserialize, Debug)]
pub struct TextAction {
    command: String,
    display: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct KeyBinding {
    // We use 'with' to point to our custom logic below
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
            if let Ok(existing_config) = toml::from_str::<Self>(&content) {
                existing_config
            } else {
                eprintln!("Failed to parse config file, using default config instead.");
                Self::default()
            }
        } else {
            println!("------------- Saving config to {path:?} -------------");
            let default_config = Self::default();
            // TODO: error logging
            let _ = default_config.save_config(&path);
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
    use serde::{Deserializer, Serializer};

    use super::*;

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

// TODO: tests
