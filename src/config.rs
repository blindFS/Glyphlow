use objc2::rc::Retained;
use objc2_app_kit::NSFont;
use objc2_core_foundation::CFRetained;
use objc2_core_graphics::CGColor;
use objc2_foundation::{NSString, ns_string};
use rdev::Key;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[derive(Debug, PartialEq, Clone, Serialize, Deserialize)]
pub enum RoleOfInterest {
    Button,
    Generic,
    Empty,
    Image,
    MenuItem,
    ScrollBar,
    StaticText,
    TextField,
    Cell,
    CustomTarget,
}

/// Custom target element to search for in a workflow
#[derive(Serialize, Deserialize, Debug, Default, PartialEq, Clone)]
pub struct CustomTarget {
    pub role: String,
    pub label: Option<String>,
    pub value: Option<String>,
    pub title: Option<String>,
    pub description: Option<String>,
    pub size: Option<(f64, f64)>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum WorkFlowAction {
    SelectAll,
    Focus,
    Press,
    ShowMenu,
    KeyCombo(KeyBinding),
    SearchFor(CustomTarget),
    Sleep(u64),
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct WorkFlow {
    pub display: String,
    pub key: char,
    #[serde(default = "default_starting_role")]
    pub starting_role: RoleOfInterest,
    pub actions: Vec<WorkFlowAction>,
}

fn default_starting_role() -> RoleOfInterest {
    RoleOfInterest::Generic
}

#[derive(Serialize, Deserialize, Debug)]
pub struct CommandAction {
    pub command: String,
    pub args: Vec<String>,
    pub display: String,
    pub key: char,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct GlyphlowTheme {
    #[serde(with = "nsfont_format", default = "default_hint_font")]
    pub hint_font: Retained<NSFont>,
    #[serde(default = "default_hint_margin")]
    pub hint_margin_size: u8,
    #[serde(with = "cgcolor_format", default = "default_hint_bg")]
    pub hint_bg_color: CFRetained<CGColor>,
    #[serde(with = "cgcolor_format", default = "default_hint_fg")]
    pub hint_fg_color: CFRetained<CGColor>,
    #[serde(with = "cgcolor_format", default = "default_hint_hl")]
    pub hint_hl_color: CFRetained<CGColor>,
    #[serde(with = "nsfont_format", default = "default_menu_font")]
    pub menu_font: Retained<NSFont>,
    #[serde(default = "default_menu_margin")]
    pub menu_margin_size: u8,
    #[serde(with = "cgcolor_format", default = "default_menu_bg")]
    pub menu_bg_color: CFRetained<CGColor>,
    #[serde(with = "cgcolor_format", default = "default_menu_fg")]
    pub menu_fg_color: CFRetained<CGColor>,
    #[serde(with = "cgcolor_format", default = "default_menu_hl")]
    pub menu_hl_color: CFRetained<CGColor>,
    #[serde(with = "vec_cgcolor_format", default = "default_frame_colors")]
    pub frame_colors: Vec<CFRetained<CGColor>>,
}

fn default_hint_font() -> Retained<NSFont> {
    NSFont::fontWithName_size(ns_string!("Andale Mono"), 12.0).expect("Default font should exist.")
}
fn default_hint_margin() -> u8 {
    3
}
fn default_hint_bg() -> CFRetained<CGColor> {
    color_from_hex("#769ff0d0")
}
fn default_hint_fg() -> CFRetained<CGColor> {
    color_from_hex("#111726ff")
}
fn default_hint_hl() -> CFRetained<CGColor> {
    color_from_hex("#11172620")
}
fn default_menu_font() -> Retained<NSFont> {
    NSFont::fontWithName_size(ns_string!("Andale Mono"), 20.0).expect("Default font should exist.")
}
fn default_menu_margin() -> u8 {
    10
}
fn default_menu_bg() -> CFRetained<CGColor> {
    color_from_hex("#111726dd")
}
fn default_menu_fg() -> CFRetained<CGColor> {
    color_from_hex("#a3aed2ff")
}
fn default_menu_hl() -> CFRetained<CGColor> {
    color_from_hex("#769ff0d0")
}
fn default_frame_colors() -> Vec<CFRetained<CGColor>> {
    vec![
        color_from_hex("#e0af68ff"),
        color_from_hex("#9ece6aff"),
        color_from_hex("#bb9af7ff"),
        color_from_hex("#f7768eff"),
    ]
}

impl Default for GlyphlowTheme {
    fn default() -> Self {
        Self {
            hint_font: default_hint_font(),
            hint_margin_size: default_hint_margin(),
            hint_bg_color: default_hint_bg(),
            hint_fg_color: default_hint_fg(),
            hint_hl_color: default_hint_hl(),
            menu_font: default_menu_font(),
            menu_margin_size: default_menu_margin(),
            menu_bg_color: default_menu_bg(),
            menu_fg_color: default_menu_fg(),
            menu_hl_color: default_hint_hl(),
            frame_colors: default_frame_colors(),
        }
    }
}

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

fn color_try_from_hex(hex: &str) -> Option<CFRetained<CGColor>> {
    let (r, g, b, a) = hex_to_rgba(hex)?;
    Some(CGColor::new_generic_rgb(r, g, b, a))
}

fn color_from_hex(hex: &str) -> CFRetained<CGColor> {
    color_try_from_hex(hex).expect("Invalid color")
}

pub fn cgcolor_to_rgba(cgcolor: &CFRetained<CGColor>) -> Option<(u8, u8, u8, u8)> {
    unsafe {
        let ptr = CGColor::components(Some(cgcolor));
        if !ptr.is_null() {
            let r = *ptr.offset(0) * 255.0;
            let g = *ptr.offset(1) * 255.0;
            let b = *ptr.offset(2) * 255.0;
            let a = *ptr.offset(3) * 255.0;
            Some((r as u8, g as u8, b as u8, a as u8))
        } else {
            None
        }
    }
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

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct KeyBinding {
    #[serde(with = "key_combo_format")]
    pub keys: Vec<Key>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Copy)]
pub enum VisibilityCheckingLevel {
    Loose,
    Medium,
    Strict,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct GlyphlowConfig {
    #[serde(default = "default_global_keybinding")]
    pub global_trigger_key: KeyBinding,
    pub editor: Option<CommandAction>,
    pub theme: GlyphlowTheme,
    #[serde(default = "default_text_actions")]
    pub text_actions: Vec<CommandAction>,
    #[serde(default = "default_workflows")]
    pub workflows: Vec<WorkFlow>,
    #[serde(default = "default_scroll_distance")]
    pub scroll_distance: f64,
    #[serde(default = "default_element_min_width")]
    pub element_min_width: u16,
    #[serde(default = "default_element_min_height")]
    pub element_min_height: u16,
    #[serde(default = "default_image_min_size")]
    pub image_min_size: u16,
    #[serde(default = "default_frame_min_size")]
    pub colored_frame_min_size: u16,
    #[serde(default = "default_ocr_languages")]
    pub ocr_languages: Vec<String>,
    #[serde(default = "default_dictionaries")]
    pub dictionaries: Vec<String>,
    #[serde(default = "default_vis_level")]
    pub visibility_checking_level: VisibilityCheckingLevel,
    #[serde(default = "default_menu_wait_ms")]
    pub menu_wait_ms: u64,
}

fn default_global_keybinding() -> KeyBinding {
    KeyBinding {
        keys: vec![Key::Alt, Key::KeyG],
    }
}
fn default_text_actions() -> Vec<CommandAction> {
    vec![]
}
fn default_workflows() -> Vec<WorkFlow> {
    vec![
        WorkFlow {
            key: 'R',
            display: " ProofRead".into(),
            starting_role: RoleOfInterest::TextField,
            actions: vec![
                WorkFlowAction::Focus,
                WorkFlowAction::SelectAll,
                WorkFlowAction::ShowMenu,
                WorkFlowAction::Sleep(150),
                WorkFlowAction::SearchFor(CustomTarget {
                    role: "MenuItem".into(),
                    title: Some("Proofread".into()),
                    ..Default::default()
                }),
                WorkFlowAction::Press,
            ],
        },
        WorkFlow {
            key: 'C',
            display: "⮺ Copy".into(),
            starting_role: RoleOfInterest::Image,
            actions: vec![
                WorkFlowAction::ShowMenu,
                WorkFlowAction::Sleep(150),
                WorkFlowAction::SearchFor(CustomTarget {
                    role: "MenuItem".into(),
                    title: Some("Copy Image".into()),
                    ..Default::default()
                }),
                WorkFlowAction::Press,
            ],
        },
        WorkFlow {
            key: 'L',
            display: " Copy Link".into(),
            starting_role: RoleOfInterest::Image,
            actions: vec![
                WorkFlowAction::ShowMenu,
                WorkFlowAction::Sleep(150),
                WorkFlowAction::SearchFor(CustomTarget {
                    role: "MenuItem".into(),
                    title: Some("Copy Image Address".into()),
                    ..Default::default()
                }),
                WorkFlowAction::Press,
            ],
        },
    ]
}
fn default_scroll_distance() -> f64 {
    0.05
}
fn default_element_min_width() -> u16 {
    15
}
fn default_element_min_height() -> u16 {
    15
}
fn default_frame_min_size() -> u16 {
    200
}
fn default_image_min_size() -> u16 {
    20
}
fn default_ocr_languages() -> Vec<String> {
    vec!["en-US".into()]
}
fn default_dictionaries() -> Vec<String> {
    vec!["New Oxford American Dictionary".into()]
}
fn default_vis_level() -> VisibilityCheckingLevel {
    VisibilityCheckingLevel::Loose
}

fn default_menu_wait_ms() -> u64 {
    100
}

impl Default for GlyphlowConfig {
    fn default() -> Self {
        GlyphlowConfig {
            global_trigger_key: default_global_keybinding(),
            editor: None,
            theme: GlyphlowTheme::default(),
            text_actions: default_text_actions(),
            workflows: default_workflows(),
            scroll_distance: default_scroll_distance(),
            element_min_width: default_element_min_width(),
            element_min_height: default_element_min_height(),
            image_min_size: default_image_min_size(),
            colored_frame_min_size: default_frame_min_size(),
            ocr_languages: default_ocr_languages(),
            dictionaries: default_dictionaries(),
            visibility_checking_level: default_vis_level(),
            menu_wait_ms: default_menu_wait_ms(),
        }
    }
}

impl GlyphlowConfig {
    pub fn load_config(path: &PathBuf) -> Result<Self, String> {
        if let Ok(content) = fs::read_to_string(path) {
            log::info!("Loading config from {path:?}");
            match toml::from_str::<Self>(&content) {
                Ok(existing_config) => Ok(existing_config),
                Err(e) => Err(format!(
                    "Failed to parse config file, using default config instead. Error: {e}"
                )),
            }
        } else {
            log::info!("Saving config to {path:?}");
            let default_config = Self::default();
            if let Err(e) = default_config.save_config(path) {
                log::error!("Failed to save config file. Error: {e}");
            }
            Ok(default_config)
        }
    }

    fn save_config(&self, path: &PathBuf) -> Result<(), Box<dyn std::error::Error>> {
        let content = toml::to_string_pretty(self)?;
        fs::write(path, content)?;
        Ok(())
    }
}

pub fn get_config_path() -> Result<PathBuf, String> {
    let base_dir = std::env::var("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|_| std::env::var("HOME").map(|dir| PathBuf::from(dir).join(".config")))
        .map_err(|_| {
            "Need environment variable `XDG_CONFIG_HOME` or `HOME` to load a configurtion file."
                .to_string()
        })?;

    let base_dir = base_dir.join("glyphlow");
    if !base_dir.exists() {
        fs::create_dir_all(&base_dir)
            .map_err(|e| format!("Failed to create config directory at {base_dir:?}: {e:?}"))?;
    }
    Ok(base_dir.join("config.toml"))
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

    pub fn serialize<S>(color: &CFRetained<CGColor>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let Some((r, g, b, a)) = cgcolor_to_rgba(color) else {
            return Err(serde::ser::Error::custom(
                "Failed to convert color {color:?} to hex string.",
            ));
        };
        let s = if a == 255 {
            format!("#{:02x}{:02x}{:02x}", r, g, b)
        } else {
            format!("#{:02x}{:02x}{:02x}{:02x}", r, g, b, a)
        };
        serializer.serialize_str(&s)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<CFRetained<CGColor>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        color_try_from_hex(&s)
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

mod vec_cgcolor_format {
    use super::*;
    use serde::{Deserializer, Serializer};

    pub fn serialize<S>(v: &[CFRetained<CGColor>], s: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        // Use a local wrapper to bridge the "with" module to the vector
        #[derive(Serialize)]
        struct Wrapper<'a>(#[serde(with = "cgcolor_format")] &'a CFRetained<CGColor>);

        v.iter().map(Wrapper).collect::<Vec<_>>().serialize(s)
    }

    pub fn deserialize<'de, D>(d: D) -> Result<Vec<CFRetained<CGColor>>, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct Wrapper(#[serde(with = "cgcolor_format")] CFRetained<CGColor>);

        let vec = Vec::<Wrapper>::deserialize(d)?;
        Ok(vec.into_iter().map(|w| w.0).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_key_combo_formatting() {
        let binding = KeyBinding {
            keys: vec![Key::ControlLeft, Key::ShiftLeft, Key::KeyZ],
        };

        // Test Serialization
        let toml_str = toml::to_string(&binding).unwrap();
        assert_eq!(toml_str, "keys = \"CTRL + SHIFT + Z\"\n");

        // Test Deserialization
        let decoded: KeyBinding = toml::from_str(&toml_str).unwrap();
        assert_eq!(
            decoded.keys,
            vec![Key::ControlLeft, Key::ShiftLeft, Key::KeyZ]
        );
    }

    #[test]
    fn test_hex_color_conversion() {
        let hex = "#ff000080"; // 50% transparent red
        let color = color_try_from_hex(hex).expect("Should parse valid hex");

        let (r, g, b, a) = cgcolor_to_rgba(&color).expect("Should extract components");

        assert_eq!(r, 255);
        assert_eq!(g, 0);
        assert_eq!(b, 0);
        assert_eq!(a, 128); // 0.5 * 255
    }

    #[test]
    fn test_hex_no_alpha() {
        let hex_6 = "#FF00FF";
        let (r, g, b, a) = hex_to_rgba(hex_6).expect("Should parse 6-digit hex");

        assert_eq!(r, 1.0);
        assert_eq!(g, 0.0);
        assert_eq!(b, 1.0);
        assert_eq!(a, 1.0);
    }

    #[test]
    fn test_theme_toml_roundtrip() {
        let mut theme = GlyphlowTheme::default();
        let custom_color = color_from_hex("#aabbccff");
        theme.hint_bg_color = custom_color;

        let toml_str = toml::to_string(&theme).expect("Should serialize theme");

        // Ensure our custom color string is present in the TOML
        assert!(toml_str.contains("#aabbcc"));

        let decoded: GlyphlowTheme = toml::from_str(&toml_str).expect("Should deserialize theme");

        // Verify the font name survived
        let font_name = NSFont::fontName(&decoded.hint_font).to_string();
        assert_eq!(font_name, "AndaleMono"); // NSFont often strips spaces in fontName
    }

    #[test]
    fn test_config_partial_deserialize() {
        // Test that missing fields fill in with @serde(default)
        let toml_input = r#"
            [theme]
            hint_margin_size = 5

            [global_trigger_key]
            keys = "META + P"
        "#;

        let config: GlyphlowConfig = toml::from_str(toml_input).unwrap();

        // Check explicit values
        assert_eq!(config.theme.hint_margin_size, 5);
        assert_eq!(
            config.global_trigger_key.keys,
            vec![Key::MetaLeft, Key::KeyP]
        );

        // Check defaulted values
        assert_eq!(config.scroll_distance, 0.05);
        assert_eq!(config.ocr_languages, vec!["en-US".to_string()]);
    }
}
