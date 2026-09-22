use monio::Key;
use objc2::rc::Retained;
use objc2_app_kit::NSFont;
use objc2_core_foundation::CFRetained;
use objc2_core_graphics::CGColor;
use objc2_foundation::{NSString, ns_string};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use tabled::Tabled;

/// What kind of element a target or a workflow is about.
///
/// [`Any`](Self::Any), [`Some`](Self::Some) and [`Generic`](Self::Generic) are
/// sentinels a workflow may use as its `starting_role` instead of naming a real
/// role. They are progressively stricter — `Any` always matches, `Generic` needs
/// a selection backed by an accessibility element — and
/// [`CustomTarget`](Self::CustomTarget) is the fourth sentinel. The remaining
/// variants mirror accessibility roles.
#[derive(Debug, Default, PartialEq, Clone, Copy, Serialize, Deserialize)]
pub enum RoleOfInterest {
    Button,
    CheckBox,
    /// Default. A selection that has an accessibility element, whatever its role.
    #[default]
    Generic,
    /// Unconditional: matches even with nothing selected.
    Any,
    /// Something is selected, but it may be a pseudo element with no accessibility
    /// element behind it (clipboard, OCR).
    Some,
    Image,
    MenuItem,
    ScrollBar,
    StaticText,
    TextField,
    /// Text with no accessibility element behind it (clipboard, OCR).
    PseudoText,
    Cell,
    /// Result of a `SearchFor` action.
    CustomTarget,
}

/// Custom target element to search for in a workflow.
#[derive(Serialize, Deserialize, Debug, Default, PartialEq, Clone)]
pub struct CustomTarget {
    pub role: String,
    pub subrole: Option<String>,
    pub label: Option<String>,
    pub value: Option<String>,
    pub title: Option<String>,
    pub description: Option<String>,
    pub size: Option<(f64, f64)>,
    pub action: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum WorkFlowAction {
    /// Print the element's attributes as a notification.
    Debug,
    SelectAll,
    GoParent,
    Focus,
    Press,
    Hover,
    Move(f64, f64),
    Click,
    RightClick,
    MiddleClick,
    ShowMenu,
    /// Open the action menu for the current selection.
    GlyphlowMenu,
    KeyCombo(KeyBinding),
    /// Search for a [`CustomTarget`] and continue with whatever it finds.
    SearchFor(CustomTarget),
    Sleep(u64),
}

/// A pre-defined workflow: a key, the apps it applies to and the actions it runs.
///
/// The [`Tabled`] derive is what lets `glyphlow-cli workflow list` print these
/// directly. Three fields need a `display` function because their stored shape
/// is not worth showing, and `order` puts `valid_app_ids` after the role even
/// though it is declared next to `key`.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Tabled)]
pub struct WorkFlow {
    #[tabled(rename = "NAME", order = 0)]
    pub display: String,
    #[tabled(rename = "KEY", order = 1)]
    pub key: String,
    #[tabled(rename = "ROLE", order = 2, display = "role_label")]
    #[serde(default = "default_starting_role")]
    pub starting_role: RoleOfInterest,
    #[tabled(rename = "APPS", order = 3, display = "apps_label")]
    pub valid_app_ids: Option<Vec<String>>,
    #[tabled(rename = "STEPS", order = 4, display = "step_count")]
    pub actions: Vec<WorkFlowAction>,
}

/// Table cell for [`WorkFlow::starting_role`].
///
/// The variant name doubles as the string a workflow writes in `config.toml`:
/// [`RoleOfInterest`] has no `#[serde(rename)]`, so this `Debug` output and the
/// accepted config spelling are the same text. Renaming a variant — or adding a
/// serde rename — makes this column name a role the parser will reject, so the
/// two must move together. `AppEngine::workflow_invalid_reason`'s "needs a …
/// selection" message reads the same way.
fn role_label(role: &RoleOfInterest) -> String {
    format!("{role:?}")
}

/// Table cell for [`WorkFlow::valid_app_ids`]: the apps a workflow is limited
/// to, or `any` when it applies everywhere.
fn apps_label(ids: &Option<Vec<String>>) -> String {
    match ids {
        Some(ids) if !ids.is_empty() => ids.join(", "),
        _ => "any".to_owned(),
    }
}

/// Table cell for [`WorkFlow::actions`]: how many steps the workflow runs.
///
/// Takes a slice so it does not trip `clippy::ptr_arg`; the derive's
/// `&self.actions` coerces.
fn step_count(actions: &[WorkFlowAction]) -> String {
    actions.len().to_string()
}

fn default_starting_role() -> RoleOfInterest {
    RoleOfInterest::Generic
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone)]
pub struct CommandAction {
    pub command: String,
    pub args: Vec<String>,
    pub display: String,
    pub key: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
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
    #[serde(default = "default_enable_animation")]
    pub enable_animation: bool,
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
fn default_enable_animation() -> bool {
    true
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
            menu_hl_color: default_menu_hl(),
            frame_colors: default_frame_colors(),
            enable_animation: default_enable_animation(),
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

/// Keys as characters, for the hint labels, the menus and the config file.
pub trait AlphabeticKey {
    /// What the key types unshifted, or a stand-in glyph for keys that type
    /// nothing at all (`Backspace`, `Shift`, ...).
    fn to_char(&self) -> char;
    /// What the key types with Shift held.
    fn shifted_char(&self) -> char;
    /// The name this key is written as in the config file (`"A"`, `"ALT"`, ...).
    fn to_str(&self) -> String;
    /// The inverse of [`AlphabeticKey::to_str`] — but not exactly: it knows
    /// `A`-`Z` and the five modifier names only, so digits and punctuation
    /// serialize but do not parse back. That asymmetry is deliberate.
    fn from_str(c: &str) -> Option<Key>;
    /// The right-hand twin of a left-hand modifier.
    ///
    /// `MetaLeft` is missing and `ShiftRight` maps to `MetaRight`, so a
    /// `META + X` binding never fires with the right Cmd key. Its only consumer
    /// is the global-trigger matcher in `key_listener`.
    fn right_alternative(&self) -> Option<Key>;
}

/// Builds [`AlphabeticKey::to_char`] and [`AlphabeticKey::shifted_char`] from a
/// single table, so the two views of one keyboard layout cannot drift apart.
/// Keys that type nothing are left out and fall through to `' '`.
macro_rules! key_char_table {
    ($($key:ident => $plain:literal / $shifted:literal),* $(,)?) => {
        fn to_char(&self) -> char {
            match self {
                $(Key::$key => $plain,)*
                // Stand-ins for keys that are not characters at all.
                Key::Backspace | Key::Delete => '󰁮',
                Key::ShiftLeft | Key::ShiftRight => '󰘶',
                _ => ' ',
            }
        }

        fn shifted_char(&self) -> char {
            match self {
                $(Key::$key => $shifted,)*
                Key::Backspace | Key::Delete => '󰁮',
                _ => ' ',
            }
        }
    };
}

impl AlphabeticKey for Key {
    key_char_table! {
        // Letters report themselves either way: the key listener delivers the
        // base key, never the character Shift would have produced.
        KeyA => 'A' / 'A',
        KeyB => 'B' / 'B',
        KeyC => 'C' / 'C',
        KeyD => 'D' / 'D',
        KeyE => 'E' / 'E',
        KeyF => 'F' / 'F',
        KeyG => 'G' / 'G',
        KeyH => 'H' / 'H',
        KeyI => 'I' / 'I',
        KeyJ => 'J' / 'J',
        KeyK => 'K' / 'K',
        KeyL => 'L' / 'L',
        KeyM => 'M' / 'M',
        KeyN => 'N' / 'N',
        KeyO => 'O' / 'O',
        KeyP => 'P' / 'P',
        KeyQ => 'Q' / 'Q',
        KeyR => 'R' / 'R',
        KeyS => 'S' / 'S',
        KeyT => 'T' / 'T',
        KeyU => 'U' / 'U',
        KeyV => 'V' / 'V',
        KeyW => 'W' / 'W',
        KeyX => 'X' / 'X',
        KeyY => 'Y' / 'Y',
        KeyZ => 'Z' / 'Z',
        // The digit row, unshifted and shifted.
        Num1 => '1' / '!',
        Num2 => '2' / '@',
        Num3 => '3' / '#',
        Num4 => '4' / '$',
        Num5 => '5' / '%',
        Num6 => '6' / '^',
        Num7 => '7' / '&',
        Num8 => '8' / '*',
        Num9 => '9' / '(',
        Num0 => '0' / ')',
        Grave => '`' / '~',
        Minus => '-' / '_',
        Equal => '=' / '+',
        BracketLeft => '[' / '{',
        BracketRight => ']' / '}',
        Backslash => '\\' / '|',
        Semicolon => ';' / ':',
        Quote => '\'' / '"',
        Comma => ',' / '<',
        Period => '.' / '>',
        // `/` starts a text search in filtering mode, so it never reaches the
        // hint filter — but it is still a character the key reports.
        Slash => '/' / '?',
    }

    fn to_str(&self) -> String {
        match self {
            Key::AltLeft | Key::AltRight => "ALT".to_string(),
            Key::ControlLeft | Key::ControlRight => "CTRL".to_string(),
            Key::MetaLeft | Key::MetaRight => "META".to_string(),
            Key::ShiftLeft | Key::ShiftRight => "SHIFT".to_string(),
            Key::Space => "SPACE".to_string(),
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
            "ALT" => Some(Key::AltLeft),
            "CTRL" => Some(Key::ControlLeft),
            "SHIFT" => Some(Key::ShiftLeft),
            "META" => Some(Key::MetaLeft),
            "SPACE" => Some(Key::Space),
            _ => None,
        }
    }

    fn right_alternative(&self) -> Option<Key> {
        match self {
            Key::AltLeft => Some(Key::AltRight),
            Key::ControlLeft => Some(Key::ControlRight),
            Key::ShiftLeft => Some(Key::ShiftRight),
            Key::ShiftRight => Some(Key::MetaRight),
            _ => None,
        }
    }
}

/// Characters that can reach the hint filter as a plain keystroke.
///
/// Mirrors the non-modifier branches of `Key::to_char()`. `/` is deliberately
/// absent: `KeyListener::handle_filter_key` intercepts it to start a text search, so
/// a hint labelled with it could never be typed.
const TYPABLE_HINT_CHARS: &str = "ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789`-=[]\\;',.";

/// Bit for `c` in a 128-bit bitmap indexed by ASCII code point, or `0` for
/// non-ASCII — which can never be a hint key, and is ignored rather than
/// silently corrupting the byte-indexed alphabet.
const fn ascii_bit(c: char) -> u128 {
    if c.is_ascii() { 1u128 << (c as u8) } else { 0 }
}

/// [`TYPABLE_HINT_CHARS`] as a bitmap, so membership is a single `and`.
const TYPABLE_HINT_MASK: u128 = {
    let bytes = TYPABLE_HINT_CHARS.as_bytes();
    let mut mask = 0u128;
    let mut i = 0;
    while i < bytes.len() {
        mask |= ascii_bit(bytes[i] as char);
        i += 1;
    }
    mask
};

/// The ordered set of keys used to label hints.
///
/// Normalized on construction: upper-cased, de-duplicated, and restricted to
/// `TYPABLE_HINT_CHARS`. An input that leaves fewer than two usable keys is
/// rejected in favour of [`HintKeys::DEFAULT_ALPHABET`], because a single key
/// cannot tell hints apart.
///
/// Always ASCII, which is what lets the label builders index it as bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "String", into = "String")]
pub struct HintKeys(String);

impl HintKeys {
    /// Plain uppercase ASCII letters.
    pub const DEFAULT_ALPHABET: &'static str = "ABCDEFGHIJKLMNOPQRSTUVWXYZ";

    /// Below this many distinct keys hints cannot be addressed unambiguously.
    const MIN_LEN: usize = 2;

    pub fn new(raw: &str) -> Self {
        // `seen` replaces a scan of the alphabet for every input character, and
        // `ignored` stays allocation-free unless something actually is dropped.
        let mut seen = 0u128;
        let mut ignored = String::new();
        let mut sanitized = String::with_capacity(raw.len());

        for c in raw.chars() {
            let c = c.to_ascii_uppercase();
            let bit = ascii_bit(c) & TYPABLE_HINT_MASK;

            if bit == 0 {
                if !ignored.contains(c) {
                    ignored.push(c);
                }
            } else if seen & bit == 0 {
                seen |= bit;
                sanitized.push(c);
            }
        }

        if !ignored.is_empty() {
            log::warn!(
                "Ignoring {ignored:?} in `hint_keys` = {raw:?}: not usable as a single \
                 keystroke hint key."
            );
        }

        if sanitized.len() < Self::MIN_LEN {
            log::warn!(
                "`hint_keys` = {raw:?} does not provide at least {} distinct usable keys, \
                 falling back to {:?}.",
                Self::MIN_LEN,
                Self::DEFAULT_ALPHABET
            );
            return Self::default();
        }

        Self(sanitized)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Radix of the hint labels: how many distinct keys there are.
    pub fn base(&self) -> usize {
        self.0.len()
    }

    /// The key every padded label starts with, and the first key to press.
    pub fn first(&self) -> char {
        // Non-empty by construction.
        self.0.as_bytes().first().copied().unwrap_or(b'A') as char
    }

    /// Label of the `i`-th hint.
    ///
    /// Without `digits` the label is as short as possible; with `digits` it is
    /// zero-padded to a fixed width so that no label is a prefix of another.
    pub fn label_for_index(&self, i: usize, digits: Option<u32>) -> String {
        // The alphabet is ASCII, so byte indexing matches character indexing and
        // the label can be built in place without an intermediate buffer.
        let keys = self.0.as_bytes();
        let base = keys.len();

        if i == 0 && digits.is_none() {
            return (keys[0] as char).to_string();
        }

        // Width of the unpadded representation; `i == 0` needs no digits.
        let natural = if i == 0 { 0 } else { i.ilog(base) as usize + 1 };
        let width = natural.max(digits.unwrap_or(0) as usize);

        let mut label = String::with_capacity(width);
        let mut n = i;
        while n > 0 {
            label.push(keys[n % base] as char);
            n /= base;
        }
        while label.len() < width {
            label.push(keys[0] as char);
        }

        label
    }

    /// Shortest label width that can address `len` distinct hints.
    pub fn digits_for_len(&self, len: usize) -> u32 {
        if len <= 1 {
            1
        } else {
            (len - 1).ilog(self.base()) + 1
        }
    }
}

impl Default for HintKeys {
    fn default() -> Self {
        Self(Self::DEFAULT_ALPHABET.to_string())
    }
}

impl From<String> for HintKeys {
    fn from(raw: String) -> Self {
        Self::new(&raw)
    }
}

impl From<HintKeys> for String {
    fn from(keys: HintKeys) -> Self {
        keys.0
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct KeyBinding {
    #[serde(with = "key_combo_format")]
    pub keys: Vec<Key>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Copy)]
pub enum VisibilityCheckingLevel {
    /// The frame only has to intersect the whole screen. Reserved for limited
    /// targets.
    Loosest,
    /// The frame has to intersect the window frame.
    Loose,
    /// The frame has to intersect its own parent, as well as the window frame.
    Medium,
    /// The frame has to intersect all its ancestors.
    Strict,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct GlyphlowConfig {
    #[serde(default = "default_global_keybinding")]
    pub global_trigger_key: KeyBinding,
    pub editor: Option<CommandAction>,
    #[serde(default = "default_theme")]
    pub theme: GlyphlowTheme,
    #[serde(default = "default_text_actions")]
    pub text_actions: Vec<CommandAction>,
    #[serde(default = "default_workflows")]
    pub workflows: Vec<WorkFlow>,
    #[serde(default = "default_scroll_distance")]
    pub scroll_distance: f64,
    #[serde(default = "default_hide_scrolling_menu")]
    pub hide_scrolling_menu: bool,
    #[serde(default = "default_hide_covered_elements")]
    pub hide_covered_elements: bool,
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
    #[serde(default = "default_wait_ms")]
    pub electron_initial_wait_ms: u64,
    #[serde(default = "default_hint_keys")]
    pub hint_keys: HintKeys,
}

impl GlyphlowConfig {
    /// Reconcile a freshly loaded config (`other`) with the running one (`self`).
    ///
    /// The global trigger, the editor, the text actions and the workflows are all
    /// baked into the key listener at startup, so their keys cannot change at
    /// runtime: for each one that differs, the running value is forced back into
    /// `other` and `false` is returned, telling the caller to ask for a restart.
    pub fn safe_reload(&self, other: &mut GlyphlowConfig) -> bool {
        let mut compatible = true;

        if self.global_trigger_key != other.global_trigger_key {
            compatible = false;
            other.global_trigger_key = self.global_trigger_key.clone();
        }

        match (self.editor.as_ref(), other.editor.as_ref()) {
            (Some(e1), Some(e2)) if e1.key != e2.key => {
                compatible = false;
                other.editor = self.editor.clone();
            }
            (None, Some(_)) | (Some(_), None) => {
                compatible = false;
                other.editor = self.editor.clone();
            }
            _ => (),
        }

        if self.text_actions.len() != other.text_actions.len()
            || self
                .text_actions
                .iter()
                .zip(other.text_actions.iter())
                .any(|(a1, a2)| a1.key != a2.key)
        {
            compatible = false;
            other.text_actions = self.text_actions.clone();
        }

        if self.workflows.len() != other.workflows.len()
            || self
                .workflows
                .iter()
                .zip(other.workflows.iter())
                .any(|(w1, w2)| w1.key != w2.key)
        {
            compatible = false;
            other.workflows = self.workflows.clone();
        }
        compatible
    }
}

fn default_theme() -> GlyphlowTheme {
    GlyphlowTheme::default()
}
fn default_global_keybinding() -> KeyBinding {
    KeyBinding {
        keys: vec![Key::AltLeft, Key::KeyG],
    }
}
fn default_text_actions() -> Vec<CommandAction> {
    vec![]
}
fn default_workflows() -> Vec<WorkFlow> {
    vec![
        WorkFlow {
            key: "R".into(),
            display: " ProofRead".into(),
            starting_role: RoleOfInterest::TextField,
            valid_app_ids: None,
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
            key: "C".into(),
            display: "⮺ Copy".into(),
            starting_role: RoleOfInterest::Image,
            valid_app_ids: None,
            actions: vec![
                WorkFlowAction::ShowMenu,
                WorkFlowAction::Sleep(150),
                WorkFlowAction::SearchFor(CustomTarget {
                    role: "MenuItem".into(),
                    title: Some("Copy Image$".into()),
                    ..Default::default()
                }),
                WorkFlowAction::Press,
            ],
        },
        WorkFlow {
            key: "L".into(),
            display: " Copy Link".into(),
            starting_role: RoleOfInterest::Image,
            valid_app_ids: None,
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
        WorkFlow {
            key: "[".into(),
            display: "󰳽 Press [Left Click]".into(),
            starting_role: RoleOfInterest::Generic,
            valid_app_ids: None,
            actions: vec![WorkFlowAction::Press],
        },
        WorkFlow {
            key: "]".into(),
            display: " Menu [Right Click]".into(),
            starting_role: RoleOfInterest::Generic,
            valid_app_ids: None,
            actions: vec![
                WorkFlowAction::ShowMenu,
                WorkFlowAction::Sleep(150),
                WorkFlowAction::SearchFor(CustomTarget {
                    role: "MenuItem".into(),
                    title: Some(".+".into()),
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
fn default_hide_scrolling_menu() -> bool {
    false
}
fn default_hide_covered_elements() -> bool {
    true
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
fn default_wait_ms() -> u64 {
    100
}
fn default_hint_keys() -> HintKeys {
    HintKeys::default()
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
            hide_scrolling_menu: default_hide_scrolling_menu(),
            hide_covered_elements: default_hide_covered_elements(),
            element_min_width: default_element_min_width(),
            element_min_height: default_element_min_height(),
            image_min_size: default_image_min_size(),
            colored_frame_min_size: default_frame_min_size(),
            ocr_languages: default_ocr_languages(),
            dictionaries: default_dictionaries(),
            visibility_checking_level: default_vis_level(),
            electron_initial_wait_ms: default_wait_ms(),
            hint_keys: default_hint_keys(),
        }
    }
}

impl GlyphlowConfig {
    /// Index of the first workflow whose display name contains `pattern`
    /// (case-insensitive, surrounding whitespace ignored).
    ///
    /// Returns `None` for an empty pattern or when nothing matches.
    pub fn find_workflow(&self, pattern: &str) -> Option<usize> {
        let pattern = pattern.trim().to_lowercase();
        if pattern.is_empty() {
            return None;
        }
        self.workflows
            .iter()
            .position(|wf| wf.display.to_lowercase().contains(&pattern))
    }

    /// Load the config, **creating a default file when none exists**.
    ///
    /// Callers that must not write to disk (e.g. `workflow list`) want
    /// [`Self::load_config_readonly`] instead.
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

    pub fn load_config_readonly(path: &PathBuf) -> Result<Self, String> {
        match fs::read_to_string(path) {
            Ok(content) => toml::from_str::<Self>(&content)
                .map_err(|e| format!("Failed to parse config file {path:?}: {e}")),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(format!("Failed to read config file {path:?}: {e}")),
        }
    }

    fn save_config(&self, path: &PathBuf) -> Result<(), Box<dyn std::error::Error>> {
        let content = toml::to_string_pretty(self)?;
        fs::write(path, content)?;
        Ok(())
    }
}

/// Path of `config.toml`, creating the containing directory if it is missing.
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

    /// --- Serialization: `Vec<Key>` -> e.g. "ALT + G" ---
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
    use rstest::rstest;
    use std::collections::HashSet;

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

    /// `to_char` and `shifted_char` are generated from one table, so this pins
    /// the layout itself: what each key reports, and what it reports with Shift
    /// held. `TYPABLE_HINT_CHARS` is derived from the same layout, which is why
    /// the digits and this punctuation have to stay usable as hint keys.
    #[rstest]
    #[case::letter_ignores_shift(Key::KeyQ, 'Q', 'Q')]
    #[case::first_digit(Key::Num1, '1', '!')]
    #[case::last_digit(Key::Num0, '0', ')')]
    #[case::grave(Key::Grave, '`', '~')]
    #[case::minus(Key::Minus, '-', '_')]
    #[case::equal(Key::Equal, '=', '+')]
    #[case::open_bracket(Key::BracketLeft, '[', '{')]
    #[case::close_bracket(Key::BracketRight, ']', '}')]
    #[case::backslash(Key::Backslash, '\\', '|')]
    #[case::semicolon(Key::Semicolon, ';', ':')]
    #[case::quote(Key::Quote, '\'', '"')]
    #[case::comma(Key::Comma, ',', '<')]
    #[case::period(Key::Period, '.', '>')]
    #[case::slash(Key::Slash, '/', '?')]
    // Not characters: they report a stand-in glyph instead. Shift has none, so
    // it falls back to the blank.
    #[case::backspace(Key::Backspace, '󰁮', '󰁮')]
    #[case::delete_reports_the_same(Key::Delete, '󰁮', '󰁮')]
    #[case::shift(Key::ShiftLeft, '󰘶', ' ')]
    // Everything else is blank.
    #[case::escape(Key::Escape, ' ', ' ')]
    #[case::space(Key::Space, ' ', ' ')]
    #[case::modifier(Key::MetaLeft, ' ', ' ')]
    fn to_char_and_shifted_char_follow_one_keyboard_layout(
        #[case] key: Key,
        #[case] plain: char,
        #[case] shifted: char,
    ) {
        assert_eq!(key.to_char(), plain, "to_char for {key:?}");
        assert_eq!(key.shifted_char(), shifted, "shifted_char for {key:?}");
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

    /// A config that says nothing about the theme — or only mentions it — must
    /// give `GlyphlowTheme::default()`. With no `[theme]` table the `Default`
    /// impl is used, with an empty one the per-field serde defaults are, and the
    /// two had drifted on `menu_hl_color`.
    #[rstest]
    #[case::no_theme_table("")]
    #[case::empty_theme_table("[theme]\n")]
    #[case::other_settings_only("scroll_distance = 0.3\n")]
    fn omitting_the_theme_gives_the_default_theme(#[case] config_toml: &str) {
        let config: GlyphlowConfig = toml::from_str(config_toml).expect("config should parse");
        let (got, want) = (&config.theme, GlyphlowTheme::default());
        let rgba = |c: &CFRetained<CGColor>| cgcolor_to_rgba(c);

        assert_eq!(
            rgba(&got.hint_bg_color),
            rgba(&want.hint_bg_color),
            "hint_bg_color"
        );
        assert_eq!(
            rgba(&got.hint_fg_color),
            rgba(&want.hint_fg_color),
            "hint_fg_color"
        );
        assert_eq!(
            rgba(&got.hint_hl_color),
            rgba(&want.hint_hl_color),
            "hint_hl_color"
        );
        assert_eq!(
            rgba(&got.menu_bg_color),
            rgba(&want.menu_bg_color),
            "menu_bg_color"
        );
        assert_eq!(
            rgba(&got.menu_fg_color),
            rgba(&want.menu_fg_color),
            "menu_fg_color"
        );
        assert_eq!(
            rgba(&got.menu_hl_color),
            rgba(&want.menu_hl_color),
            "menu_hl_color"
        );
        assert_eq!(
            got.frame_colors.iter().map(rgba).collect::<Vec<_>>(),
            want.frame_colors.iter().map(rgba).collect::<Vec<_>>(),
            "frame_colors"
        );

        let font = |f: &NSFont| (NSFont::fontName(f).to_string(), NSFont::pointSize(f));
        assert_eq!(font(&got.hint_font), font(&want.hint_font), "hint_font");
        assert_eq!(font(&got.menu_font), font(&want.menu_font), "menu_font");

        assert_eq!(got.hint_margin_size, want.hint_margin_size);
        assert_eq!(got.menu_margin_size, want.menu_margin_size);
        assert_eq!(got.enable_animation, want.enable_animation);
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
        assert!(config.theme.enable_animation);
        assert_eq!(config.scroll_distance, 0.05);
        assert_eq!(config.ocr_languages, vec!["en-US".to_string()]);
    }

    #[test]
    fn test_config_global_key_with_space() {
        let toml_input = r#"
            [global_trigger_key]
            keys = "META + SPACE"
        "#;

        let config: GlyphlowConfig = toml::from_str(toml_input).unwrap();

        assert_eq!(
            config.global_trigger_key.keys,
            vec![Key::MetaLeft, Key::Space]
        );
    }

    #[test]
    fn test_safe_reload_compatibility() {
        let mut old_config = GlyphlowConfig::default();
        let mut new_config = GlyphlowConfig::default();

        assert!(new_config.safe_reload(&mut old_config));

        new_config.scroll_distance = 99.9;
        assert!(
            new_config.safe_reload(&mut old_config),
            "Minor settings should be compatible"
        );

        new_config.global_trigger_key = KeyBinding {
            keys: vec![Key::ControlLeft, Key::KeyX],
        };
        assert!(
            !new_config.safe_reload(&mut old_config),
            "Hotkey change should be incompatible"
        );
        assert_eq!(
            old_config.global_trigger_key, new_config.global_trigger_key,
            "Should sync value"
        );

        new_config.editor = Some(CommandAction {
            command: "code".into(),
            args: vec![],
            display: "VSCode".into(),
            key: "E".into(),
        });
        assert!(
            !new_config.safe_reload(&mut old_config),
            "Adding editor should be incompatible"
        );

        // 5. Test change in workflow keys
        new_config.workflows[0].key = "X".into();
        assert!(
            !new_config.safe_reload(&mut old_config),
            "Workflow key change should be incompatible"
        );
    }

    #[test]
    fn test_safe_reload_workflow_len_mismatch() {
        let mut old_config = GlyphlowConfig::default();
        let mut new_config = GlyphlowConfig::default();

        new_config.workflows.pop();

        let compatible = new_config.safe_reload(&mut old_config);
        assert!(!compatible, "Changing workflow count must be incompatible");
        assert_eq!(
            old_config.workflows.len(),
            new_config.workflows.len(),
            "Workflow list should sync"
        );
    }

    #[test]
    fn test_safe_reload_text_actions() {
        let mut old_config = GlyphlowConfig::default();
        let mut new_config = GlyphlowConfig::default();

        new_config.text_actions.push(CommandAction {
            command: "echo".into(),
            args: vec![],
            display: "Echo".into(),
            key: "E".into(),
        });

        assert!(
            !new_config.safe_reload(&mut old_config),
            "New text action should be incompatible"
        );
        assert_eq!(old_config.text_actions.len(), 1);
    }

    /// Sanitizing is lenient on purpose: case is normalized, duplicates keep
    /// their first occurrence, and anything the key listener cannot report is
    /// dropped rather than rejected.
    #[rstest]
    #[case::case_is_normalized("asdfjkl;", "ASDFJKL;")]
    #[case::duplicates_keep_the_first_occurrence("aabcc", "ABC")]
    #[case::untypable_characters_are_dropped("a!b c\tdé", "ABCD")]
    #[case::digits_and_most_punctuation_are_valid("123;',.", "123;',.")]
    fn hint_keys_sanitizes_input(#[case] raw: &str, #[case] expected: &str) {
        assert_eq!(HintKeys::new(raw).as_str(), expected);
    }

    /// `/` starts a text search in filtering mode, so it never reaches the hint
    /// filter and must not be accepted as a hint key.
    #[test]
    fn test_hint_keys_rejects_slash() {
        let keys = HintKeys::new("asdfjkl/;");
        assert_eq!(keys.as_str(), "ASDFJKL;");
        assert!(!keys.as_str().contains('/'));

        // A slash-only alphabet leaves nothing usable, so the default is kept.
        assert_eq!(HintKeys::new("//").as_str(), HintKeys::DEFAULT_ALPHABET);
    }

    /// The bitmap must stay in sync with the readable list, and everything that
    /// can enter an alphabet must be ASCII — `label_for_index` indexes the
    /// alphabet as bytes, so a non-ASCII key would corrupt every label.
    #[test]
    fn test_typable_hint_chars_match_mask_and_are_ascii() {
        assert!(TYPABLE_HINT_CHARS.is_ascii());
        assert!(HintKeys::DEFAULT_ALPHABET.is_ascii());

        for c in TYPABLE_HINT_CHARS.chars() {
            assert_ne!(
                ascii_bit(c) & TYPABLE_HINT_MASK,
                0,
                "{c:?} is listed as typable but is missing from the mask"
            );
            // The leading 'A' guarantees the input is long enough to keep.
            assert!(
                HintKeys::new(&format!("A{c}")).as_str().contains(c),
                "{c:?} is listed as typable but was dropped by sanitizing"
            );
        }

        // Non-ASCII never makes it into an alphabet, so byte indexing is sound.
        for raw in ["é", "aé", "日本", "a\u{301}"] {
            assert!(
                HintKeys::new(raw).as_str().is_ascii(),
                "{raw:?} produced a non-ASCII alphabet"
            );
        }

        // The default must be a valid alphabet, not merely a string.
        assert_eq!(
            HintKeys::new(HintKeys::DEFAULT_ALPHABET),
            HintKeys::default()
        );
        assert!(HintKeys::default().base() >= HintKeys::MIN_LEN);
    }

    #[test]
    fn test_hint_keys_falls_back_when_unusable() {
        // A single key cannot tell hints apart, so the default is kept instead.
        for raw in ["", "a", "aa", "a!!a", "  "] {
            assert_eq!(
                HintKeys::new(raw),
                HintKeys::default(),
                "{raw:?} should fall back to the default alphabet"
            );
        }
        assert_eq!(HintKeys::default().as_str(), "ABCDEFGHIJKLMNOPQRSTUVWXYZ");
    }

    /// How a custom alphabet is laid out: 8 keys address 8 hints with a single
    /// keystroke, and the 9th hint starts the two-keystroke block.
    #[test]
    fn test_hint_keys_custom_alphabet_labels() {
        let keys = HintKeys::new("asdfjkl;");
        assert_eq!(keys.base(), 8);

        assert_eq!(keys.digits_for_len(8), 1);
        assert_eq!(keys.label_for_index(0, Some(1)), "A");
        assert_eq!(keys.label_for_index(7, Some(1)), ";");

        assert_eq!(keys.digits_for_len(9), 2);
        assert_eq!(keys.label_for_index(0, Some(2)), "AA");
        assert_eq!(keys.label_for_index(8, Some(2)), "AS");
        assert_eq!(keys.label_for_index(63, Some(2)), ";;");
    }

    /// The width needed is the smallest `d` with `base.pow(d) >= len`, so each
    /// alphabet flips to the next width exactly one hint past its capacity.
    #[rstest]
    // 26 keys: one keystroke covers 26 hints, two cover 676.
    #[case::letters_zero("ABCDEFGHIJKLMNOPQRSTUVWXYZ", 0, 1)]
    #[case::letters_one("ABCDEFGHIJKLMNOPQRSTUVWXYZ", 1, 1)]
    #[case::letters_at_capacity("ABCDEFGHIJKLMNOPQRSTUVWXYZ", 26, 1)]
    #[case::letters_one_past_capacity("ABCDEFGHIJKLMNOPQRSTUVWXYZ", 27, 2)]
    #[case::letters_two_digit_capacity("ABCDEFGHIJKLMNOPQRSTUVWXYZ", 676, 2)]
    #[case::letters_one_past_two_digit_capacity("ABCDEFGHIJKLMNOPQRSTUVWXYZ", 677, 3)]
    // 8 keys: one keystroke covers 8 hints, two cover 64.
    #[case::home_row_one("ASDFJKL;", 1, 1)]
    #[case::home_row_one_past_capacity("ASDFJKL;", 9, 2)]
    #[case::home_row_two_digit_capacity("ASDFJKL;", 64, 2)]
    #[case::home_row_one_past_two_digit_capacity("ASDFJKL;", 65, 3)]
    // 2 keys: every extra keystroke only doubles the reach.
    #[case::binary_at_capacity("JK", 2, 1)]
    #[case::binary_one_past_capacity("JK", 3, 2)]
    #[case::binary_two_digit_capacity("JK", 4, 2)]
    #[case::binary_one_past_two_digit_capacity("JK", 5, 3)]
    fn digits_for_len_is_the_smallest_sufficient_width(
        #[case] alphabet: &str,
        #[case] len: usize,
        #[case] expected: u32,
    ) {
        assert_eq!(HintKeys::new(alphabet).digits_for_len(len), expected);
    }

    /// `label_for_index` is written for speed (no intermediate buffer, exact
    /// capacity, O(1) alphabet lookup), so it must still agree exactly with the
    /// straightforward reference implementation it replaced.
    #[test]
    fn test_label_for_index_matches_reference() {
        fn reference(keys: &[char], i: usize, digits: Option<u32>) -> String {
            if i == 0 && digits.is_none() {
                return keys[0].to_string();
            }
            let mut n = i;
            let mut result = Vec::new();
            while n > 0 {
                result.push(keys[n % keys.len()]);
                n /= keys.len();
            }
            if let Some(digits) = digits {
                while result.len() < digits as usize {
                    result.push(keys[0]);
                }
            }
            result.into_iter().collect()
        }

        for raw in [
            "asdfjkl;",
            "jk",
            "a;",
            "0123456789",
            "ABCDEFGHIJKLMNOPQRSTUVWXYZ",
        ] {
            let keys = HintKeys::new(raw);
            let chars = keys.as_str().chars().collect::<Vec<_>>();
            assert_eq!(chars.len(), keys.base());

            for digits in [None, Some(1), Some(2), Some(3), Some(5)] {
                for i in 0..256 {
                    assert_eq!(
                        keys.label_for_index(i, digits),
                        reference(&chars, i, digits),
                        "mismatch for {raw:?} at i = {i}, digits = {digits:?}"
                    );
                }
            }
        }
    }

    /// Fixed width matters: without it one label would be a prefix of another,
    /// which makes filtering ambiguous.
    #[test]
    fn test_hint_keys_labels_are_unique_and_fixed_width() {
        for raw in ["asdfjkl;", "jk", "0123456789", "ABCDEFGHIJKLMNOPQRSTUVWXYZ"] {
            let keys = HintKeys::new(raw);
            let alphabet = keys.as_str();

            for len in 1..=(keys.base() * keys.base() + 1) {
                let digits = keys.digits_for_len(len);
                let labels = (0..len)
                    .map(|i| keys.label_for_index(i, Some(digits)))
                    .collect::<HashSet<_>>();

                assert_eq!(
                    labels.len(),
                    len,
                    "labels for {len} hints over {alphabet:?} are not unique: {labels:?}"
                );
                for label in &labels {
                    assert_eq!(
                        label.chars().count(),
                        digits as usize,
                        "label {label:?} is not {digits} characters wide"
                    );
                    assert!(
                        label.chars().all(|c| alphabet.contains(c)),
                        "label {label:?} uses a key outside of {alphabet:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn test_hint_keys_toml() {
        let config: GlyphlowConfig = toml::from_str(r#"hint_keys = "asdfjkl;""#).unwrap();
        assert_eq!(config.hint_keys.as_str(), "ASDFJKL;");
        assert_eq!(config.hint_keys.base(), 8);
        assert_eq!(config.hint_keys.first(), 'A');

        // Unusable values are normalized instead of failing the whole config.
        let config: GlyphlowConfig = toml::from_str(r#"hint_keys = "aaaa""#).unwrap();
        assert_eq!(config.hint_keys, HintKeys::default());

        // Missing field keeps the default alphabet.
        let config: GlyphlowConfig = toml::from_str("").unwrap();
        assert_eq!(config.hint_keys, HintKeys::default());

        // Round trip preserves the normalized value.
        let config = GlyphlowConfig {
            hint_keys: HintKeys::new("asdfjkl;"),
            ..Default::default()
        };
        let toml_str = toml::to_string(&config).unwrap();
        assert!(toml_str.contains("hint_keys = \"ASDFJKL;\""));
        assert_eq!(
            toml::from_str::<GlyphlowConfig>(&toml_str)
                .unwrap()
                .hint_keys,
            config.hint_keys
        );
    }

    fn workflow_with_display(display: &str) -> WorkFlow {
        WorkFlow {
            display: display.into(),
            key: "X".into(),
            valid_app_ids: None,
            starting_role: RoleOfInterest::Generic,
            actions: vec![],
        }
    }

    #[test]
    fn test_find_workflow_substring_case_insensitive() {
        let config = GlyphlowConfig {
            workflows: vec![
                workflow_with_display(" ProofRead"),
                workflow_with_display("⮺ Copy Link"),
            ],
            ..GlyphlowConfig::default()
        };

        // Substring match, ignoring case and surrounding whitespace
        assert_eq!(config.find_workflow("proofread"), Some(0));
        assert_eq!(config.find_workflow("  PROOF  "), Some(0));
        assert_eq!(config.find_workflow("Copy"), Some(1));

        // First match wins when several workflows contain the pattern
        assert_eq!(config.find_workflow("o"), Some(0));

        // No match / empty pattern
        assert_eq!(config.find_workflow("missing"), None);
        assert_eq!(config.find_workflow(""), None);
        assert_eq!(config.find_workflow("   "), None);
    }

    /// The three table cells the [`Tabled`] derive cannot infer. They live here
    /// rather than in the CLI because `#[tabled(display = "…")]` names them by
    /// path in this module.
    #[test]
    fn test_workflow_table_cells() {
        assert_eq!(role_label(&RoleOfInterest::StaticText), "StaticText");
        assert_eq!(step_count(&[]), "0");
        assert_eq!(
            step_count(&[WorkFlowAction::Click, WorkFlowAction::Press]),
            "2"
        );

        assert_eq!(apps_label(&None), "any");
        assert_eq!(apps_label(&Some(vec![])), "any");
        assert_eq!(
            apps_label(&Some(vec!["com.apple.Safari".into(), "Notes".into()])),
            "com.apple.Safari, Notes"
        );
    }

    /// The column order and headings the CLI prints come from the derive, so
    /// assert them here rather than only through the rendered table.
    #[test]
    fn test_workflow_table_headers_and_order() {
        let headers: Vec<String> = WorkFlow::headers()
            .into_iter()
            .map(|h| h.into_owned())
            .collect();
        assert_eq!(headers, ["NAME", "KEY", "ROLE", "APPS", "STEPS"]);

        let wf = WorkFlow {
            display: "ProofRead".into(),
            key: "p".into(),
            valid_app_ids: Some(vec!["Notes".into()]),
            starting_role: RoleOfInterest::TextField,
            actions: vec![WorkFlowAction::Click],
        };
        let fields: Vec<String> = wf.fields().into_iter().map(|f| f.into_owned()).collect();
        assert_eq!(fields, ["ProofRead", "p", "TextField", "Notes", "1"]);
        assert_eq!(WorkFlow::LENGTH, headers.len());
        assert_eq!(fields.len(), headers.len());
    }
}
