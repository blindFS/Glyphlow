use crate::{
    action::{Word, WordPicker, dictionary_lookup, text_to_clipboard},
    ax_element::{
        ElementCache, ElementOfInterest, Frame, GetAttribute, HintBox, RoleOfInterest,
        SetAttribute, Target, traverse_elements,
    },
    config::{ActionKind, AlphabeticKey, GlyphlowConfig},
    drawer::{GlyphlowDrawingLayer, create_overlay_window, get_main_screen_size},
    os_util::get_focused_pid,
};
use accessibility::{AXUIElement, AXUIElementActions, AXUIElementAttributes};
use accessibility_sys::kAXFocusedAttribute;
use core_foundation::{base::TCFType, boolean::CFBoolean, number::CFNumber, string::CFString};

use notify::{FsEventWatcher, RecursiveMode};
use objc2::{MainThreadMarker, rc::Retained};
use objc2_core_foundation::CGSize;
use objc2_quartz_core::CALayer;

use notify_debouncer_mini::{Debouncer, new_debouncer};
use rdev::Key;
use std::{
    collections::HashSet,
    path::PathBuf,
    process::Child,
    sync::mpsc::{self, Receiver, Sender},
    time::Duration,
};

#[derive(PartialEq)]
enum Mode {
    DashBoard,
    ElementActionMenu,
    Filtering,
    Idle,
    Scrolling,
    TextActionMenu,
    Transparent,
    WordPicking,
}

enum Signal {
    Stdout(String),
    Stderr(String),
    Exit,
}

static MAX_TEXT_DISPLAY_LEN: usize = 30;

/// Global state for Glyphlow,
/// mainly cached UI elements, and some related drawings
pub struct AppState {
    /// Keyboard listener for mod keys
    pub pressed_keys: HashSet<Key>,
    mode: Mode,
    /// Used for drawing hint boxes on screen
    hint_boxes: Vec<HintBox>,
    element_cache: ElementCache,
    key_prefix: String,
    screen_size: CGSize,
    window: Retained<CALayer>,
    /// Which elements of interest to look for
    target: Target,
    config: GlyphlowConfig,
    hint_width: u32,
    selected: Option<ElementOfInterest>,
    call_listener: Option<Receiver<Signal>>,
    /// For editing element text values
    temp_file: PathBuf,
    temp_file_listener: Option<(Debouncer<FsEventWatcher>, Receiver<()>)>,
    word_picker: Option<WordPicker>,
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}

impl AppState {
    pub fn new() -> Self {
        let mtm = MainThreadMarker::new().expect("Not on main thread");
        let screen_size = get_main_screen_size(mtm);
        let window = create_overlay_window(mtm, screen_size);
        window.makeKeyAndOrderFront(None);
        let window = CALayer::from_window(&window).expect("Failed to get root layer of window.");

        let config = GlyphlowConfig::load_config();
        let temp_file = Self::create_cache_file().expect("Failed to create temp file.");

        Self {
            pressed_keys: HashSet::new(),
            mode: Mode::Idle,
            hint_boxes: vec![],
            element_cache: ElementCache::new(
                config.element_min_width as f64,
                config.element_min_height as f64,
            ),
            key_prefix: String::new(),
            target: Target::default(),
            hint_width: 0,
            screen_size,
            window,
            config,
            selected: None,
            call_listener: None,
            temp_file,
            temp_file_listener: None,
            word_picker: None,
        }
    }

    fn create_cache_file() -> Option<PathBuf> {
        let cache_dir = std::env::var("HOME")
            .ok()
            .map(|dir| PathBuf::from(dir).join(".cache/glyphlow"))?;
        if !cache_dir.exists() {
            std::fs::create_dir_all(&cache_dir).ok()?;
        }
        let cache_file = cache_dir.join("tempfile.md");
        Some(cache_file)
    }

    fn deactivate(&mut self) {
        self.clear_cache();
        self.clear_drawing();
        self.selected = None;
        self.mode = Mode::Idle;
    }

    fn clear_cache(&mut self) {
        self.temp_file_listener = None;
        self.word_picker = None;
        self.hint_boxes.clear();
        self.element_cache.clear();
        self.key_prefix.clear();
    }

    fn clear_drawing(&self) {
        self.window.clear();
    }

    fn draw_hints(&self, boxes: &[HintBox]) {
        self.clear_drawing();
        self.window.draw_hints(
            boxes,
            &self.config.theme,
            self.key_prefix.len(),
            self.screen_size,
        );
    }

    fn draw_text_action_menu(&self, text: &str) {
        // Truncate long text
        let text = if text.len() > MAX_TEXT_DISPLAY_LEN {
            &format!("{:.max_len$}...", text, max_len = MAX_TEXT_DISPLAY_LEN)
        } else {
            text
        };
        let mut msg = format!(
            "Select Action for Text:\n\n{text}\n\n⮺ Copy (C)\n◫ Dictionary (D)\n󰃻 Split (S)"
        );
        for action in self.config.text_actions.iter() {
            msg.push_str(&format!("\n{} ({})", action.display, action.key));
        }
        self.draw_menu(&msg);
    }

    fn draw_menu(&self, msg: &str) {
        self.window
            .draw_menu(msg, self.screen_size, &self.config.theme);
    }

    const ELEMENT_ACTIONS: &str =
        "Select Target:\n󰦨 Text (T)\n󰳽 Press (P)\n󱕒 ScrollBar (S)\n󰊄 Input (I)";

    fn draw_element_action_menu(&self) {
        self.draw_menu(Self::ELEMENT_ACTIONS);
    }

    fn draw_scroll_bar_menu(&self) {
        self.draw_menu("Scroll With Following Keys:\n> Down/Right (J)\n< Up/Left (K)\n+ Distance Increase (I)\n- Distance Decrease (D)");
    }

    fn draw_dash_board(&self) {
        let mut msg = format!("{}\n󰙅 Element (E)", Self::ELEMENT_ACTIONS);
        if let Some(editor) = self.config.editor.as_ref() {
            msg.push_str(&format!("\n{} ({})", editor.display, editor.key));
        }
        self.draw_menu(&msg);
    }

    /// Activates the app and caches UI elements
    fn activate(&mut self, target: Target) {
        // HACK: abuse self.target to mark whether to call external editor
        self.target = target.clone();
        let target = if target == Target::Edit {
            Target::Editable
        } else {
            target
        };

        if self.selected.is_none() {
            self.selected = get_focused_pid().map(|pid| {
                let focused_window = AXUIElement::application(pid);
                let window_frame = focused_window
                    .get_frame()
                    .unwrap_or_else(|| Frame::from_origion(self.screen_size));

                ElementOfInterest::new(
                    focused_window,
                    None,
                    RoleOfInterest::GenericNode,
                    window_frame.clone(),
                )
            });
        }

        self.clear_cache();
        if let Some(ElementOfInterest { element, .. }) = self.selected.as_ref() {
            traverse_elements(
                element,
                // Very loose visibility constraint
                &Frame::from_origion(self.screen_size),
                &mut self.element_cache,
                &target,
            );
        }

        if !self.element_cache.cache.is_empty() {
            self.mode = Mode::Filtering;

            let (hint_width, new_boxes) = self.element_cache.hint_boxes(
                &Frame::from_origion(self.screen_size),
                &self.config.theme.frame_colors,
                self.config.colored_frame_min_size as f64,
            );
            self.hint_width = hint_width;
            self.hint_boxes.extend(new_boxes);
            self.draw_hints(&self.hint_boxes);
        } else {
            // Don't deactivate yet, backspace to rollback
            self.clear_drawing();
        }
    }

    fn focus_on_element(element: &AXUIElement) {
        element.set_attribute_by_name(kAXFocusedAttribute, CFBoolean::true_value().as_CFType());
    }

    fn press_on_element(element: &AXUIElement) {
        Self::focus_on_element(element);
        if let Err(e) = element.press() {
            eprintln!("Failed to click element: {e}");
        };
        // let _ = element.show_menu();
    }

    /// Filter the UI elements and redraw hints.
    fn filter_by_key(&mut self, key_char: char) {
        if key_char == '-' {
            self.key_prefix.pop();
        } else {
            self.key_prefix.push(key_char);
        }

        let filtered_boxes = self
            .hint_boxes
            .iter()
            .filter(|b| b.label.starts_with(&self.key_prefix))
            .cloned()
            .collect::<Vec<_>>();

        // Only 1 remaining, take some actions
        if self.key_prefix.len() == self.hint_width as usize
            && filtered_boxes.len() == 1
            && let Some(HintBox { idx, .. }) = filtered_boxes.first()
            && let Some(
                eoi @ ElementOfInterest {
                    element, context, ..
                },
            ) = self.element_cache.cache.get(*idx)
        {
            // eoi.element.inspect();
            self.clear_drawing();
            match self.target {
                Target::Clickable => {
                    Self::press_on_element(element);
                    self.deactivate();
                }
                Target::Text => {
                    if let Some(text) = context {
                        self.selected = Some(eoi.clone());
                        self.draw_text_action_menu(text);
                        self.mode = Mode::TextActionMenu;
                    }
                }
                Target::ChildElement => {
                    self.selected = Some(eoi.clone());
                    // TODO: optimize UX for selected element
                    // 1. Parent frame
                    // 2. Action menu for parent
                    self.activate(Target::ChildElement);
                    if self.element_cache.cache.is_empty() {
                        // select actions for current selected element
                        // TODO:
                        // 1. Screen shot
                        // 2. Mouse ops
                        self.draw_element_action_menu();
                        self.mode = Mode::ElementActionMenu;
                    }
                }
                Target::ScrollBar => {
                    self.selected = Some(eoi.clone());
                    self.clear_cache();
                    self.draw_scroll_bar_menu();
                    self.mode = Mode::Scrolling;
                }
                Target::Editable => {
                    self.selected = Some(eoi.clone());
                    Self::focus_on_element(element);
                    self.deactivate();
                }
                Target::Edit => {
                    self.selected = Some(eoi.clone());
                    // Focused before editing to increase the success rate
                    Self::focus_on_element(element);
                    let text = context.clone().unwrap_or_default();
                    // Register file update listener
                    // So the text value on the UI element could be updated
                    // on the next keystroke after file saving.
                    self.temp_file_listener = self.open_editor(&text);
                    self.mode = Mode::Idle;
                }
            }
        } else if filtered_boxes.is_empty() {
            self.deactivate();
        } else {
            self.draw_hints(&filtered_boxes);
        }
    }

    fn quick_follow(&mut self) {
        if self.element_cache.cache.len() == 1 {
            self.filter_by_key('A');
        }
    }

    fn open_editor(&mut self, text: &str) -> Option<(Debouncer<FsEventWatcher>, Receiver<()>)> {
        let editor = self
            .config
            .editor
            .as_ref()
            .expect("Internal Error: No editor set.");

        // Write current selected text to temp file
        let _ = std::fs::write(&self.temp_file, text);
        let temp_fp = self
            .temp_file
            .to_str()
            .unwrap_or_else(|| panic!("Failed to get temp file path for {:?}.", self.temp_file));

        let (ftx, frx) = mpsc::channel();

        // NOTE: listen to file updates with FsEvent
        let mut debouncer = new_debouncer(Duration::from_millis(200), move |res| match res {
            Ok(_) => {
                // Notify: file updated
                ftx.send(()).expect("Failed to send file update signal.");
            }
            Err(e) => eprintln!("Watch error: {:?}", e),
        })
        .ok()?;

        debouncer
            .watcher()
            .watch(self.temp_file.as_path(), RecursiveMode::NonRecursive)
            .ok()?;

        let args = editor
            .args
            .iter()
            .map(|arg| arg.replace("{glyphlow_temp_file}", temp_fp));
        let child = std::process::Command::new(&editor.command)
            .args(args)
            .stdout(std::process::Stdio::piped())
            .spawn()
            .ok()?;

        std::thread::spawn(|| {
            if let Err(e) = child.wait_with_output() {
                eprintln!("{e}");
            };
        });
        Some((debouncer, frx))
    }

    fn take_external_action(&mut self, key_char: char, selected_text: &str) -> bool {
        for action in &self.config.text_actions {
            if action.key.to_ascii_uppercase() != key_char {
                continue;
            }

            let args = action
                .args
                .iter()
                .map(|arg| arg.replace("{glyphlow_text}", selected_text));
            let Ok(child) = std::process::Command::new(&action.command)
                .args(args)
                .stdout(std::process::Stdio::piped())
                .spawn()
            else {
                eprintln!(
                    "Failed to spawn command: {} {}",
                    action.command,
                    action.args.join(" ")
                );
                return false;
            };

            if action.kind == ActionKind::NonBlocking {
                let (tx, rx) = mpsc::channel();
                // Don't react to any key event before the finishing signal
                self.call_listener = Some(rx);
                self.mode = Mode::Transparent;
                external_call(child, tx);
            } else {
                // Wait for the stdout as the new text
                match child.wait_with_output() {
                    Ok(o) => {
                        if !o.stdout.is_empty() {
                            let new_text = String::from_utf8_lossy(&o.stdout)
                                .trim_end_matches('\n')
                                .to_string();
                            self.update_selected_text_and_show_menu(new_text);
                        } else if !o.stderr.is_empty() {
                            eprintln!("External stderr: {}", String::from_utf8_lossy(&o.stderr));
                            return false;
                        }
                    }
                    Err(e) => {
                        eprintln!("Failed to run command: {e}");
                        return false;
                    }
                }
            }

            return true;
        }
        false
    }

    fn update_selected_text(&mut self, new_text: String, replace: bool) {
        if let Some(ElementOfInterest {
            element,
            context,
            // role,
            ..
        }) = self.selected.as_mut()
        {
            // if *role == RoleOfInterest::TextField {
            if replace && let Err(e) = element.set_value(CFString::new(&new_text).as_CFType()) {
                eprintln!("Failed to set the text of focused element: {element:?}\n Error: {e}");
            }
            // }
            *context = Some(new_text);
        }
    }

    fn update_selected_text_and_show_menu(&mut self, new_text: String) {
        self.clear_drawing();
        self.draw_text_action_menu(&new_text);
        self.update_selected_text(new_text, false);
        self.mode = Mode::TextActionMenu;
    }

    pub fn is_active(&self) -> bool {
        self.mode != Mode::Idle && self.mode != Mode::Transparent
    }

    pub fn check_external_output(&mut self) -> bool {
        // Temp file updated usually means the text is changed in an external editor,
        // update the text value of current selected AXUIElement.
        if self
            .temp_file_listener
            .as_ref()
            .and_then(|(_, rx)| rx.try_recv().ok())
            .is_some()
            && let Ok(new_text) = std::fs::read_to_string(&self.temp_file)
        {
            // println!("Temp file updated: {}\nEOI: {:?}", new_text, self.selected);
            self.update_selected_text(new_text, true);
            return true;
        }

        if let Some(msg) = self
            .call_listener
            .as_ref()
            .and_then(|rx| rx.try_recv().ok())
        {
            match msg {
                Signal::Stdout(msg) => {
                    self.update_selected_text_and_show_menu(msg);
                    return true;
                }
                Signal::Stderr(msg) => {
                    eprintln!("Error: {msg}");
                    self.deactivate();
                }
                Signal::Exit => {
                    self.deactivate();
                }
            }
            self.call_listener = None;
        }
        false
    }

    pub fn act_on_key(&mut self, key: Key) -> bool {
        let key_char = key.to_char();

        match self.mode {
            // Don't swallow any key
            Mode::Transparent => false,
            Mode::Idle => {
                if self.config.global_trigger_key.keys.iter().all(|k| {
                    k == &key
                        || self.pressed_keys.contains(k)
                        || k.right_alternative()
                            .is_some_and(|r| *k == r || self.pressed_keys.contains(&r))
                }) {
                    self.selected = None;
                    self.mode = Mode::DashBoard;
                    self.draw_dash_board();
                    true
                } else {
                    false
                }
            }
            Mode::DashBoard => {
                match key_char {
                    'P' => {
                        self.activate(Target::Clickable);
                    }
                    'T' => {
                        self.activate(Target::Text);
                    }
                    'E' => {
                        self.activate(Target::ChildElement);
                    }
                    'S' => {
                        self.activate(Target::ScrollBar);
                        self.quick_follow();
                    }
                    'I' => {
                        self.activate(Target::Editable);
                    }
                    _ => {
                        if let Some(editor) = self.config.editor.as_ref()
                            && editor.key.to_ascii_uppercase() == key_char
                        {
                            self.activate(Target::Edit);
                            self.quick_follow();
                        } else {
                            self.deactivate();
                        }
                    }
                }
                true
            }
            Mode::Filtering => {
                // NOTE: Act on currently selected parent node
                if key == Key::Return && self.selected.is_some() {
                    self.draw_element_action_menu();
                    self.mode = Mode::ElementActionMenu;
                } else if key_char == ' ' {
                    self.deactivate();
                } else {
                    self.filter_by_key(key_char);
                }
                true
            }
            Mode::ElementActionMenu => {
                match key_char {
                    'P' => {
                        self.activate(Target::Clickable);
                    }
                    'T' => {
                        self.activate(Target::Text);
                    }
                    'S' => {
                        self.activate(Target::ScrollBar);
                    }
                    'I' => {
                        self.activate(Target::Editable);
                    }
                    _ => {
                        self.deactivate();
                        return false;
                    }
                }
                self.quick_follow();
                true
            }
            Mode::TextActionMenu => {
                let Some(ElementOfInterest {
                    context: Some(text),
                    ..
                }) = self.selected.as_ref()
                else {
                    panic!("Internal Error: No selected element in Mode::TextActionMenu.");
                };

                let text = text.clone();

                // Clear old menu no matter which action is taken
                self.clear_drawing();

                // TODO:
                // 1. URL handling
                let keep_drawing = match key_char {
                    'C' => {
                        text_to_clipboard(&text);
                        // TODO: better notification
                        self.draw_menu("Copied to clipboard.");
                        true
                    }
                    'D' => {
                        if let Some(def_str) = dictionary_lookup(&text) {
                            self.draw_menu(&def_str);
                        } else {
                            // TODO: better notification
                            self.draw_menu("No definition found.");
                        }
                        true
                    }
                    'S' => {
                        let word_picker = WordPicker::new(text);

                        let (text_size, attr_string) = word_picker
                            .get_attributed_string(self.screen_size, &self.config.theme.menu_font);
                        self.window.draw_attributed_string(
                            attr_string,
                            self.screen_size,
                            text_size,
                            &self.config.theme,
                        );

                        self.clear_cache();
                        self.word_picker = Some(word_picker);
                        self.mode = Mode::WordPicking;
                        true
                    }
                    _ => self.take_external_action(key_char, &text),
                };

                if !keep_drawing {
                    self.deactivate();
                }

                true
            }
            Mode::WordPicking => {
                let word_picker = self
                    .word_picker
                    .as_ref()
                    .expect("Internal Error: No word picker in Mode::WordPicking.");

                if key_char == ' ' {
                    self.deactivate();
                    return true;
                } else if key_char == '-' {
                    self.key_prefix.pop();
                } else {
                    self.key_prefix.push(key_char);
                }

                let filtered_words = word_picker
                    .words
                    .iter()
                    .filter(|w| w.label.starts_with(&self.key_prefix))
                    .collect::<Vec<_>>();

                if self.key_prefix.len() == word_picker.digits as usize
                    && filtered_words.len() == 1
                    && let Some(Word { text, .. }) = filtered_words.first()
                {
                    self.update_selected_text_and_show_menu(text.clone())
                }

                true
            }
            Mode::Scrolling => {
                let ElementOfInterest { element, .. } = self.selected.as_ref().expect(
                    "A scrollbar is supposed to be selected before entering Mode::Scrolling!",
                );

                let Some(old_val) = element
                    .value()
                    .ok()
                    .and_then(|v| v.downcast::<CFNumber>())
                    .and_then(|f| f.to_f64())
                else {
                    self.deactivate();
                    return false;
                };

                let scroll_unit = self.config.scroll_distance;
                match key_char {
                    'J' => {
                        let _ = element.set_value(
                            CFNumber::from((old_val + scroll_unit).min(1.0)).as_CFType(),
                        );
                    }
                    'K' => {
                        let _ = element.set_value(
                            CFNumber::from((old_val - scroll_unit).max(0.0)).as_CFType(),
                        );
                    }
                    'I' => {
                        self.config.scroll_distance *= 1.5;
                    }
                    'D' => {
                        self.config.scroll_distance /= 1.5;
                    }
                    _ => {
                        self.deactivate();
                    }
                }
                true
            }
        }
    }
}

fn external_call(child: Child, tx: Sender<Signal>) {
    std::thread::spawn(move || {
        let output = child.wait_with_output();
        match output {
            Ok(o) => {
                if !o.stdout.is_empty() {
                    let _ = tx.send(Signal::Stdout(
                        String::from_utf8_lossy(&o.stdout)
                            .trim_end_matches('\n')
                            .to_string(),
                    ));
                } else if !o.stderr.is_empty() {
                    let _ = tx.send(Signal::Stderr(
                        String::from_utf8_lossy(&o.stderr)
                            .trim_end_matches('\n')
                            .to_string(),
                    ));
                } else {
                    let _ = tx.send(Signal::Exit);
                }
            }
            Err(e) => {
                eprintln!("Failed to run command: {e}");
                let _ = tx.send(Signal::Exit);
            }
        }
    });
}
