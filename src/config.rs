use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug)]
pub struct GlyphlowTheme {
    pub font: String,
    pub font_size: u8,
    pub hint_radius: u8,
    pub hint_bg_color: String,
    pub hint_fg_color: String,
}

impl Default for GlyphlowTheme {
    fn default() -> Self {
        GlyphlowTheme {
            font: "Andale Mono".to_string(),
            font_size: 12,
            hint_radius: 5,
            hint_bg_color: "#769ff0A0".to_string(),
            hint_fg_color: "#FFFFFFFF".to_string(),
        }
    }
}

#[derive(Serialize, Deserialize, Debug)]
pub struct TextAction {
    command: String,
    display: String,
}

#[derive(Default, Serialize, Deserialize, Debug)]
pub struct GlyphlowConfig {
    pub global_trigger_key: String,
    pub theme: GlyphlowTheme,
    pub text_actions: Vec<TextAction>,
}

use std::fs;
use std::path::PathBuf;

fn get_config_path() -> Option<PathBuf> {
    let base_dir = std::env::var("XDG_CONFIG_HOME")
        .ok()
        .map(|dir| PathBuf::from(dir).join("glyphlow"))?;
    if !base_dir.exists() {
        fs::create_dir_all(&base_dir).ok()?;
    }
    Some(base_dir.join("config.toml"))
}

pub fn load_config() -> GlyphlowConfig {
    let Some(path) = get_config_path() else {
        return GlyphlowConfig::default();
    };

    if let Ok(content) = fs::read_to_string(&path) {
        println!("------------- Loading config from {path:?} -------------");
        toml::from_str::<GlyphlowConfig>(&content).unwrap_or_default()
    } else {
        println!("------------- Saving config to {path:?} -------------");
        let default_config = GlyphlowConfig::default();
        let _ = save_config(&path, &default_config);
        default_config
    }
}

fn save_config(path: &PathBuf, config: &GlyphlowConfig) -> Result<(), Box<dyn std::error::Error>> {
    let content = toml::to_string_pretty(config)?;
    fs::write(path, content)?;
    Ok(())
}
