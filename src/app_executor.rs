use crate::{
    AppSignal, DASH_BOARD_MENU_ITEMS, FilterMode, IMAGE_ACTION_MENU_ITEMS, KeyState, MenuItem,
    Mode, SCROLLBAR_MENU_ITEMS, ScrollAction, TEXT_ACTION_MENU_ITEMS, TextAction,
    action::{
        OCRResult, WordPicker, get_dictionary_attributed_string, perform_ocr, screen_shot,
        text_from_clipboard, text_to_clipboard,
    },
    ax_element::{
        ElementCache, ElementOfInterest, GetAttribute, SetAttribute, Target, traverse_elements,
    },
    config::{GlyphlowConfig, RoleOfInterest, VisibilityCheckingLevel, WorkFlow, WorkFlowAction},
    drawer::GlyphlowDrawingLayer,
    os_util::get_focused_pid,
    util::{Frame, HintBox, estimate_frame_for_text, hint_boxes_from_frames, select_range_helper},
};
use accessibility::{AXUIElement, AXUIElementActions, AXUIElementAttributes};
use accessibility_sys::{
    kAXErrorAttributeUnsupported, kAXErrorCannotComplete, kAXFocusedAttribute, kAXPopoverRole,
};
use core_foundation::{base::TCFType, boolean::CFBoolean, number::CFNumber, string::CFString};
use log::Level;
use objc2::rc::Retained;
use objc2_core_foundation::CGSize;
use objc2_quartz_core::CALayer;
use rdev::{Button, EventType, simulate};
use std::{
    collections::VecDeque,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::mpsc::Sender;

static MAX_TEXT_DISPLAY_LEN: usize = 30;

#[derive(Debug, Default)]
struct MultiSeletionState {
    is_on: bool,
    one_side_idex: Option<usize>,
    role: Option<RoleOfInterest>,
}

impl MultiSeletionState {
    fn toggle(&mut self) {
        self.is_on = !self.is_on;
        self.one_side_idex = None;
        self.role = None;
    }

    fn reset(&mut self) {
        self.is_on = false;
        self.one_side_idex = None;
        self.role = None;
    }

    fn clear_one_side(&mut self) {
        self.one_side_idex = None;
    }

    fn set_one_side(&mut self, other: usize) -> Option<(usize, usize)> {
        if let Some(one) = self.one_side_idex {
            Some((one, other))
        } else {
            self.one_side_idex = Some(other);
            None
        }
    }
}

/// Global state for Glyphlow,
/// mainly cached UI elements, and some related drawings
pub struct AppExecutor {
    state: Arc<Mutex<Mode>>,
    key_state: Arc<Mutex<KeyState>>,
    /// Used for drawing hint boxes on screen
    hint_boxes: Vec<HintBox>,
    element_cache: ElementCache,
    key_prefix: String,
    screen_size: CGSize,
    window: Retained<CALayer>,
    /// Useful for notification clearing
    notification_layers: Vec<Retained<CALayer>>,
    /// Which elements of interest to look for
    target: Target,
    config: GlyphlowConfig,
    hint_width: u32,
    selected: Option<ElementOfInterest>,
    /// Keep track of editing element,
    /// so that we can use other glyphlow actions while editing
    editing: Option<ElementOfInterest>,
    /// For editing element text values
    temp_file: PathBuf,
    word_picker: Option<WordPicker>,
    ocr_cache: Option<OCRResult>,
    timeout_sender: Sender<()>,
    /// Special treatment for Electron based apps.
    /// Like simulate mouse clicking instead of `element.press()`
    is_electron: bool,
    last_pid: i32,
    last_window_frame: Frame,
    /// For multi-selection
    multi_selection: MultiSeletionState,
    /// Something to finish after filtering
    pending_workflow_actions: VecDeque<WorkFlowAction>,
}

impl AppExecutor {
    pub fn new(
        state: Arc<Mutex<Mode>>,
        key_state: Arc<Mutex<KeyState>>,
        config: GlyphlowConfig,
        window: Retained<CALayer>,
        screen_size: CGSize,
        temp_file: PathBuf,
        timeout_sender: Sender<()>,
    ) -> Self {
        Self {
            state,
            key_state,
            hint_boxes: vec![],
            element_cache: ElementCache::new(
                config.element_min_width as f64,
                config.element_min_height as f64,
                config.image_min_size as f64,
            ),
            key_prefix: String::new(),
            target: Target::default(),
            hint_width: 0,
            screen_size,
            window,
            notification_layers: Vec::new(),
            config,
            timeout_sender,
            selected: None,
            editing: None,
            temp_file,
            word_picker: None,
            ocr_cache: None,
            is_electron: false,
            last_pid: 0,
            last_window_frame: Frame::from_origion(screen_size),
            multi_selection: MultiSeletionState::default(),
            pending_workflow_actions: VecDeque::new(),
        }
    }

    fn set_mode(&self, mode: Mode) {
        if let Ok(mut state) = self.state.lock() {
            *state = mode;
        }
    }

    fn set_simulating_key(&self, flag: bool) {
        if let Ok(mut ks) = self.key_state.lock() {
            ks.is_simulating = flag;
        }
    }

    fn check_mode(&self, mode: Mode) -> bool {
        self.state.try_lock().is_ok_and(|s| *s == mode)
    }

    fn deactivate(&mut self) {
        self.clear_cache();
        self.clear_drawing();
        self.selected = None;
        self.pending_workflow_actions.clear();
        self.set_mode(Mode::Idle);
    }

    fn clear_cache(&mut self) {
        self.word_picker = None;
        self.ocr_cache = None;
        self.notification_layers.clear();
        self.hint_boxes.clear();
        self.element_cache.clear();
        self.key_prefix.clear();
        self.multi_selection.reset();
    }

    fn clear_drawing(&self) {
        self.window.clear();
    }

    fn draw_selected_frame(&self) {
        if let Some(ElementOfInterest { frame, .. }) = self.selected.as_ref() {
            self.window.draw_frame_box(
                &frame.invert_y(self.screen_size.height),
                &self.config.theme.hint_bg_color,
            );
        }
    }

    fn draw_hints(&self, boxes: &[HintBox]) {
        self.clear_drawing();
        // NOTE: only select the other side of the same role,
        // and excluding the already selected one.
        if self.multi_selection.is_on
            && let Some(one_idx) = self.multi_selection.one_side_idex
        {
            let iter = boxes.iter().filter(|hb| hb.idx != one_idx);
            self.window.draw_hints(
                iter,
                &self.config.theme,
                self.key_prefix.len(),
                self.screen_size,
            );
        } else {
            self.window.draw_hints(
                boxes.iter(),
                &self.config.theme,
                self.key_prefix.len(),
                self.screen_size,
            );
        };
        self.draw_selected_frame();
    }

    fn menu_format_helper(
        key: &str,
        display: &str,
        prefix_len: usize,
        max_key_len: usize,
    ) -> String {
        let padding = " ".repeat(max_key_len - key.chars().count());
        let filling = "_".repeat(prefix_len);
        format!(
            "\n{padding}({filling}{}) {display}",
            key.chars().skip(prefix_len).collect::<String>(),
        )
    }

    fn menu_msg_alignment_helper(
        &self,
        head: &str,
        builtin_menu_items: &[MenuItem],
        need_editor: bool,
        need_action: bool,
        need_workflow: bool,
        key_prefix: &str,
    ) -> String {
        let prefix_len = key_prefix.chars().count();
        let mut max_key_len = 1;
        let mut menu_itmes = Vec::new();

        // Skip static single key menu items
        // when searching for multi-key actions
        for it in builtin_menu_items {
            if it.key.starts_with(key_prefix) {
                max_key_len = max_key_len.max(it.key.chars().count());
                menu_itmes.push((it.key, it.description));
            }
        }

        // Editor entry
        if need_editor
            && let Some(editor) = self.config.editor.as_ref()
            && editor.key.starts_with(key_prefix)
        {
            max_key_len = max_key_len.max(editor.key.chars().count());
            menu_itmes.push((&editor.key, &editor.display));
        }

        // TODO: refactor this if we introduce actions for elements other than text
        if need_action {
            for action in self.config.text_actions.iter() {
                if action.key.starts_with(key_prefix) {
                    max_key_len = max_key_len.max(action.key.chars().count());
                    menu_itmes.push((&action.key, &action.display));
                }
            }
        }

        // Workflows valid for current selected element
        if need_workflow {
            for workflow in self.config.workflows.iter() {
                if workflow.key.starts_with(key_prefix) && self.is_workflow_valid(workflow) {
                    max_key_len = max_key_len.max(workflow.key.chars().count());
                    menu_itmes.push((&workflow.key, &workflow.display));
                }
            }
        }

        if menu_itmes.is_empty() {
            return "Wrong key sequence\nPress Backspace to go back".to_string();
        }

        // Aligned
        let mut msg = head.to_string();
        for (key, display) in menu_itmes {
            msg.push_str(&Self::menu_format_helper(
                key,
                display,
                prefix_len,
                max_key_len,
            ));
        }

        msg
    }

    fn draw_menu(&self, msg: &str) -> Retained<CALayer> {
        self.window
            .draw_menu(msg, self.screen_size, &self.config.theme)
    }

    fn draw_dashboard(&self, key_prefix: &str) {
        let msg = self.menu_msg_alignment_helper(
            "Pick a Target:",
            &DASH_BOARD_MENU_ITEMS,
            true,
            false,
            true,
            key_prefix,
        );

        self.clear_drawing();
        self.draw_selected_frame();
        self.draw_menu(&msg);
    }

    fn draw_image_action_menu(&self, key_prefix: &str) {
        let msg = self.menu_msg_alignment_helper(
            "Pick an Action for Image:",
            &IMAGE_ACTION_MENU_ITEMS,
            false,
            false,
            true,
            key_prefix,
        );

        self.draw_menu(&msg);
    }

    fn draw_text_action_menu(&self, text: &str, key_prefix: &str) {
        // Truncate long text
        let text = if text.len() > MAX_TEXT_DISPLAY_LEN {
            &format!("{:.max_len$}...", text, max_len = MAX_TEXT_DISPLAY_LEN)
        } else {
            text
        };
        let header = format!("Pick an Action for Text:\n\n{}\n", text);
        let msg = self.menu_msg_alignment_helper(
            &header,
            &TEXT_ACTION_MENU_ITEMS,
            true,
            true,
            true,
            key_prefix,
        );

        self.draw_menu(&msg);
    }

    fn draw_scrolling_menu(&self, key_prefix: &str) {
        let msg = self.menu_msg_alignment_helper(
            "Pick a Scrolling Action:",
            &SCROLLBAR_MENU_ITEMS,
            false,
            false,
            false,
            key_prefix,
        );
        self.draw_menu(&msg);
    }

    fn draw_element_menu(&self, key_prefix: &str, role: &RoleOfInterest, set_mode: bool) {
        self.clear_drawing();
        // Set mode before drawing to make it more responsive
        if set_mode {
            match role {
                RoleOfInterest::Image => self.set_mode(Mode::ImageActionMenu),
                RoleOfInterest::ScrollBar => self.set_mode(Mode::Scrolling),
                RoleOfInterest::TextField
                | RoleOfInterest::StaticText
                | RoleOfInterest::PseudoText => self.set_mode(Mode::TextActionMenu),
                _ if self.target == Target::Text => self.set_mode(Mode::TextActionMenu),
                _ if self.target == Target::Scrollable => self.set_mode(Mode::Scrolling),
                _ => self.set_mode(Mode::DashBoard),
            }
        }

        let text_action_helper = || {
            let text = self
                .selected
                .as_ref()
                .and_then(|eoi| eoi.context.as_ref())
                .expect("Internal Error: selected text should be ready for text action menu");
            self.draw_text_action_menu(text, key_prefix);
        };

        match role {
            RoleOfInterest::Image => self.draw_image_action_menu(key_prefix),
            RoleOfInterest::ScrollBar => self.draw_scrolling_menu(key_prefix),
            RoleOfInterest::TextField | RoleOfInterest::StaticText | RoleOfInterest::PseudoText => {
                text_action_helper();
            }
            _ if self.target == Target::Text => text_action_helper(),
            _ if self.target == Target::Scrollable => self.draw_scrolling_menu(key_prefix),
            _ => self.draw_dashboard(key_prefix),
        }
    }

    fn menu_refresh(&self, key_prefix: &str, set_mode: bool) {
        if let Some(eoi) = self.selected.as_ref() {
            self.draw_element_menu(key_prefix, &eoi.role, set_mode);
        } else {
            self.clear_drawing();
            self.draw_dashboard(key_prefix);
        }
    }

    const SHORT_TIMEOUT: u64 = 1;
    const LONG_TIMEOUT: u64 = 2;
    const DEBUG_TIMEOUT: u64 = 5;

    fn notify_then_deactivate(&mut self, msg: &str, log_level: Level) {
        self.set_mode(Mode::WaitAndDeactivate);
        self.notify(msg, log_level);
    }

    fn notify(&mut self, msg: &str, log_level: Level) {
        let timeout_secs = match log_level {
            Level::Trace | Level::Info => Self::SHORT_TIMEOUT,
            Level::Debug => Self::DEBUG_TIMEOUT,
            _ => Self::LONG_TIMEOUT,
        };
        log::log!(log_level, "{msg}");
        self.notification_layers.push(self.draw_menu(msg));
        let sender = self.timeout_sender.clone();
        tokio::spawn(async move { delay(sender, timeout_secs).await });
    }

    fn draw_word_picker(&self) -> (Vec<(usize, String)>, u32) {
        let word_picker = self
            .word_picker
            .as_ref()
            .expect("Internal Error: No word picker set.");
        if let Some((text_size, attr_string)) = word_picker.get_attributed_string(
            self.screen_size,
            &self.config.theme,
            &self.key_prefix,
            self.multi_selection.one_side_idex,
        ) {
            self.window.draw_attributed_string(
                attr_string,
                self.screen_size,
                text_size,
                &self.config.theme,
            );
        };

        (
            word_picker.matched_words(&self.key_prefix),
            word_picker.digits,
        )
    }

    fn select_app_window(&mut self, vis_level: VisibilityCheckingLevel) -> Option<Frame> {
        let screen_frame = Frame::from_origion(self.screen_size);

        // NOTE: prioritize system alarms
        if let Ok(app) = AXUIElement::application_with_bundle("com.apple.coreservices.uiagent")
            && let Ok(window) = app.focused_window()
        {
            let frame = window.get_frame(screen_frame);
            self.last_window_frame = frame;
            self.is_electron = false;

            self.selected = Some(ElementOfInterest::new(
                Some(window),
                None,
                RoleOfInterest::Generic,
                frame,
            ));

            return Some(frame);
        }

        let (pid, is_electron) = get_focused_pid()?;
        self.is_electron = is_electron;
        let focused_app = AXUIElement::application(pid);

        // HACK: need this to bootstrap UI tree generation for some electron apps,
        // e.g. Discord
        if is_electron && (pid != self.last_pid || vis_level == VisibilityCheckingLevel::Loosest) {
            let _ = focused_app.role();
            std::thread::sleep(Duration::from_millis(self.config.electron_initial_wait_ms));
        }
        self.last_pid = pid;

        // HACK: menu items may go out of focused window
        let (focused_window, window_frame) = if vis_level == VisibilityCheckingLevel::Loosest {
            (focused_app, screen_frame)
        } else {
            let mut window = focused_app.focused_window();
            // NOTE: prioritize popover windows, e.g. Apple Music search
            if let Ok(windows) = focused_app.windows()
                && windows.len() > 1
            {
                for win in windows.iter() {
                    if win.role().is_ok_and(|r| r == kAXPopoverRole) {
                        window = Ok(win.clone());
                        break;
                    }
                }
            }
            let window = window.unwrap_or(focused_app);
            let frame = window.get_frame(screen_frame);
            (window, frame)
        };
        self.last_window_frame = window_frame;

        self.selected = Some(ElementOfInterest::new(
            Some(focused_window),
            None,
            RoleOfInterest::Generic,
            window_frame,
        ));

        Some(window_frame)
    }

    fn ui_element_traverse_on_activation(&mut self, target: Target) {
        // HACK: abuse self.target to mark whether to call external editor
        self.target = target.clone();
        let target = match target {
            Target::Edit => Target::Editable,
            _ => target,
        };

        let vis_level = match target {
            // NOTE: loose visibility checking for specific targets
            Target::MenuItem | Target::Custom(_) => VisibilityCheckingLevel::Loosest,
            _ => self.config.visibility_checking_level,
        };

        if self.selected.is_none() {
            self.select_app_window(vis_level);
        }

        self.clear_cache();
        if let Some(ElementOfInterest {
            element: Some(element),
            frame,
            ..
        }) = self.selected.as_ref()
        {
            traverse_elements(
                element,
                // Very loose visibility constraint
                frame,
                &self.last_window_frame,
                &mut self.element_cache,
                &target,
                vis_level,
            );
        }
    }

    fn draw_hints_from_cache(&mut self) {
        let (hint_width, new_boxes) = self.element_cache.hint_boxes(
            &Frame::from_origion(self.screen_size),
            &self.config.theme,
            self.config.colored_frame_min_size as f64,
        );
        self.hint_width = hint_width;
        self.hint_boxes = new_boxes;
        self.draw_hints(&self.hint_boxes);
    }

    /// Activates the app and caches UI elements
    fn activate(&mut self, target: Target) {
        let need_help_msg = target == Target::ChildElement && self.selected.is_none();
        self.ui_element_traverse_on_activation(target);

        if !self.element_cache.cache.is_empty() {
            self.set_mode(Mode::Filtering);
            self.draw_hints_from_cache();
            if need_help_msg {
                self.notify("Press Enter to act.", Level::Trace);
            }
        } else if self.target == Target::Scrollable
            && let Some(eoi) = self.selected.as_ref()
        {
            // Fallback to mouse scroll if no scrollbar found
            let (x, y) = eoi.frame.center();
            Self::simulate_event(&EventType::MouseMove { x, y });
            self.clear_cache();
            self.draw_element_menu("", &RoleOfInterest::ScrollBar, true);
        } else {
            self.clear_drawing();
            self.notify_then_deactivate("No relevant UI elements found.", Level::Warn);
        }
    }

    fn simulate_event(event_type: &EventType) {
        match simulate(event_type) {
            Ok(()) => (),
            Err(e) => {
                log::error!("Failed to simulate event {event_type:?}: {e}");
            }
        }
    }

    fn simulate_click(x: f64, y: f64, right: bool) {
        let button = if right { Button::Right } else { Button::Left };
        Self::simulate_event(&EventType::MouseMove { x, y });
        std::thread::sleep(Duration::from_millis(20));
        Self::simulate_event(&EventType::ButtonPress(button));
        std::thread::sleep(Duration::from_millis(20));
        Self::simulate_event(&EventType::ButtonRelease(button));
    }

    fn focus_on_element(element: &AXUIElement) {
        element.set_attribute_by_name(kAXFocusedAttribute, CFBoolean::true_value().as_CFType());
    }

    fn press_on_element(&self, element: &AXUIElement, role: &RoleOfInterest, center: (f64, f64)) {
        let (x, y) = center;
        Self::focus_on_element(element);

        if self.is_electron || *role == RoleOfInterest::Cell {
            Self::simulate_click(x, y, false);
        } else if let Err(e) = element.press() {
            log::warn!("Failed to do UI press on element: {e}");
            match e {
                // NOTE: Sometimes this error is false alarm, usually because it takes longer
                // than expected, we shouldn't click in this case, otherwise it is performed twice.
                accessibility::Error::Ax(err_num)
                    if err_num == kAXErrorCannotComplete
                        || err_num == kAXErrorAttributeUnsupported => {}
                _ => {
                    log::info!("Simulating mouse click instead...");
                    Self::simulate_click(x, y, false);
                }
            }
        };
    }

    fn scroll_to_value(element: &AXUIElement, val: f64) {
        if let Err(e) = element.set_value(CFNumber::from(val.clamp(0.0, 1.0)).as_CFType()) {
            log::warn!("Failed to set value to the selected scroll bar: {e}.");
        };
    }

    fn right_click_menu_on_element(&self, element: &AXUIElement, center: (f64, f64)) {
        let (x, y) = center;

        if self.is_electron {
            Self::simulate_click(x, y, true);
        } else if let Err(e) = element.show_menu() {
            log::warn!("Failed to show menu on element: {e}");
            match e {
                // NOTE: Sometimes this error is false alarm, usually because it takes longer
                // than expected, we shouldn't click in this case, otherwise it is performed twice.
                accessibility::Error::Ax(err_num)
                    if err_num == kAXErrorCannotComplete
                        || err_num == kAXErrorAttributeUnsupported => {}
                _ => {
                    log::info!("Simulating mouse click instead...");
                    Self::simulate_click(x, y, true);
                }
            }
        };
    }

    fn ocr_res_filtering(&mut self) {
        let ocr_res = self
            .ocr_cache
            .as_ref()
            .expect("Internal Error: OCR cache not set.");
        let len = ocr_res.len();
        let iter = ocr_res.iter().map(|(_, rect)| Frame::from_cgrect(rect));
        let (digits, ocr_hints) = hint_boxes_from_frames(
            len,
            iter,
            &Frame::from_origion(self.screen_size),
            &self.config.theme,
            self.config.colored_frame_min_size as f64,
        );
        self.hint_width = digits;

        let filtered = ocr_hints
            .iter()
            .filter(|b| b.label.starts_with(&self.key_prefix))
            .cloned()
            .collect::<Vec<_>>();

        if self.key_prefix.len() == digits as usize
            && let Some(hb) = filtered.first()
        {
            if self.multi_selection.is_on {
                if let Some((idx1, idx2)) = self.multi_selection.set_one_side(hb.idx) {
                    let choices: Vec<(String, Frame, bool)> = ocr_res
                        .iter()
                        .map(|(s, rect)| (s.clone(), Frame::from_cgrect(rect), true))
                        .collect::<Vec<_>>();
                    let (text, frame) = select_range_helper(&choices, idx1, idx2)
                        .expect("Internal Error: wrong ocr hint indexing.");
                    self.selected = Some(ElementOfInterest::pseudo(None, frame));
                    self.update_selected_text_and_show_menu(text.clone());
                } else {
                    self.key_prefix.clear();
                    self.draw_hints(&ocr_hints);
                }
            } else {
                let (selected_text, cg_rect) = ocr_res
                    .get(hb.idx)
                    .expect("Internal Error: wrong ocr hint indexing.");
                // Context initialized as None, but updated right after
                self.selected = Some(ElementOfInterest::pseudo(None, Frame::from_cgrect(cg_rect)));
                self.update_selected_text_and_show_menu(selected_text.clone());
            }
        } else {
            self.draw_hints(&filtered);
        }
    }

    async fn perform_ocr_on_frame(&mut self, frame: Frame) {
        self.clear_drawing();
        // NOTE: for images with parts out of sight
        let frame = frame
            .intersect(&Frame::from_origion(self.screen_size))
            .unwrap_or(frame);
        match perform_ocr(&frame, &self.config.ocr_languages).await {
            Ok(ocr_res) if !ocr_res.is_empty() => {
                self.ocr_cache = Some(ocr_res);
                self.key_prefix.clear();
                self.set_mode(Mode::OCRResultFiltering);
                self.ocr_res_filtering();
            }
            Err(e) => {
                self.notify_then_deactivate(&format!("OCR failed: {e:?}"), Level::Error);
            }
            _ => {
                self.notify_then_deactivate("Empty OCR result.", Level::Warn);
            }
        }
    }

    /// Filter the UI elements and redraw hints.
    async fn filter_by_key(&mut self) {
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
                    element: Some(element),
                    context,
                    frame,
                    role,
                    ..
                },
            ) = self.element_cache.cache.get(*idx)
        {
            match self.target {
                Target::MenuItem | Target::Clickable => {
                    let center = frame.center();
                    self.press_on_element(element, role, center);
                    self.deactivate();
                }
                Target::Image => {
                    self.selected = Some(eoi.clone());
                    self.draw_element_menu("", role, true);
                }
                Target::Custom(_) => {
                    self.selected = Some(eoi.clone());
                    self.execute_pending_workflow_actions();
                }
                Target::ImageOCR => self.perform_ocr_on_frame(*frame).await,
                Target::Text => {
                    if self.multi_selection.is_on {
                        if let Some((idx1, idx2)) = self.multi_selection.set_one_side(*idx) {
                            // NOTE: Role based filtering only when the roles on both sides match
                            let role_ref = if self
                                .multi_selection
                                .role
                                .as_ref()
                                .is_some_and(|other| *role == *other)
                            {
                                Some(role)
                            } else {
                                None
                            };
                            let (text, frame) = self
                                .element_cache
                                .select_range(idx1, idx2, role_ref)
                                .expect("Internal Error: wrong indexing of hints.");
                            self.selected = Some(ElementOfInterest::pseudo(None, frame));
                            self.update_selected_text_and_show_menu(text);
                        } else {
                            self.multi_selection.role = Some(*role);
                            self.key_prefix.clear();
                            self.draw_hints(&self.hint_boxes);
                        }
                    } else if context.is_some() {
                        self.selected = Some(eoi.clone());
                        self.draw_element_menu("", role, true);
                    }
                }
                Target::ChildElement => {
                    self.selected = Some(eoi.clone());
                    self.ui_element_traverse_on_activation(Target::ChildElement);
                    // Quick follow if only 1 element remaining
                    // NOTE: use count to avoid circular pointer
                    let mut count = 0;
                    while self.element_cache.cache.len() == 1 && count < 10 {
                        count += 1;
                        self.selected = Some(self.element_cache.cache[0].clone());
                        self.ui_element_traverse_on_activation(Target::ChildElement);
                    }

                    // Actions for current selected element
                    if self.element_cache.cache.is_empty() {
                        let role = self
                            .selected
                            .as_ref()
                            .map(|eoi| eoi.role)
                            .unwrap_or_default();
                        self.draw_element_menu("", &role, true);
                    } else {
                        self.draw_hints_from_cache();
                    }
                }
                Target::Scrollable => {
                    self.selected = Some(eoi.clone());
                    self.clear_cache();
                    self.draw_element_menu("", &RoleOfInterest::ScrollBar, true);
                }
                Target::Editable => {
                    self.selected = Some(eoi.clone());
                    Self::focus_on_element(element);
                    self.deactivate();
                }
                Target::Edit => {
                    self.editing = Some(eoi.clone());
                    // Focused before editing to increase the success rate
                    Self::focus_on_element(element);
                    let text = context.clone().unwrap_or_default();
                    match self.open_editor(&text) {
                        Ok(_) => {
                            self.set_mode(Mode::Editing);
                            self.selected = None;
                        }
                        Err(e) => {
                            self.notify_then_deactivate(
                                &format!("Failed to open editor: {e}"),
                                Level::Error,
                            );
                        }
                    }
                    self.clear_drawing();
                }
            }
        } else {
            self.draw_hints(&filtered_boxes);
        }
    }

    async fn quick_follow(&mut self) {
        if self.element_cache.cache.len() == 1 {
            self.key_prefix.push('A');
            self.filter_by_key().await;
        }
    }

    fn open_editor(&mut self, text: &str) -> Result<(), Box<dyn std::error::Error>> {
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

        let args = editor
            .args
            .iter()
            .map(|arg| arg.replace("{glyphlow_temp_file}", temp_fp));
        let mut child = std::process::Command::new(&editor.command)
            .args(args)
            .spawn()?;

        std::thread::spawn(move || {
            if let Err(e) = child.wait() {
                log::error!("Editor failed to run: {e}");
            }
        });
        Ok(())
    }

    fn take_external_action(&mut self, idx: usize, selected_text: &str) {
        let action = self
            .config
            .text_actions
            .get(idx)
            .expect("Internal Error: text action index out of bounds.");
        let args = action
            .args
            .iter()
            .map(|arg| arg.replace("{glyphlow_text}", selected_text));
        let Ok(child) = std::process::Command::new(&action.command)
            .args(args)
            .stdout(std::process::Stdio::piped())
            .spawn()
        else {
            self.notify_then_deactivate(
                &format!(
                    "Failed to spawn command: {} {}",
                    action.command,
                    action.args.join(" ")
                ),
                Level::Error,
            );
            return;
        };

        // Wait for the stdout as the new text
        match child.wait_with_output() {
            Ok(o) => {
                if !o.stdout.is_empty() {
                    let new_text = String::from_utf8_lossy(&o.stdout)
                        .trim_end_matches('\n')
                        .to_string();
                    self.update_selected_text_and_show_menu(new_text);
                } else if !o.stderr.is_empty() {
                    self.notify_then_deactivate(
                        &format!("External stderr: {}", String::from_utf8_lossy(&o.stderr)),
                        Level::Error,
                    );
                }
            }
            Err(e) => {
                self.notify_then_deactivate(&format!("Failed to run command: {e}"), Level::Error);
            }
        }
    }

    /// Check if a workflow's starting_role matches current selected element
    fn is_workflow_valid(&self, wf: &WorkFlow) -> bool {
        match wf.starting_role {
            RoleOfInterest::Empty => self.selected.is_none(),
            RoleOfInterest::Generic => self.selected.as_ref().is_some_and(|s| s.element.is_some()),
            _ => self
                .selected
                .as_ref()
                .is_some_and(|s| s.element.is_some() && s.role == wf.starting_role),
        }
    }

    /// Returns true if there're pending actions to finish
    fn execute_workflow_action(&mut self, act: &WorkFlowAction) -> bool {
        // Actions don't need a selected element
        match act {
            WorkFlowAction::GlyphlowMenu => {
                self.menu_refresh("", true);
                // HACK: break the loop so the notification will be kept,
                // basically `GlyphlowMenu` should be a terminal op
                self.pending_workflow_actions.clear();
                return true;
            }
            WorkFlowAction::Sleep(ms) => {
                std::thread::sleep(Duration::from_millis(*ms));
                return false;
            }
            WorkFlowAction::SearchFor(ct) => {
                self.selected = None;
                self.activate(Target::Custom(ct.clone()));
                if self.element_cache.cache.len() == 1 {
                    self.clear_drawing();
                    self.selected = Some(self.element_cache.cache[0].clone());
                } else if self.element_cache.cache.len() > 1 {
                    return true;
                } else {
                    // Stop on empty result
                    return true;
                }
                return false;
            }
            WorkFlowAction::KeyCombo(kb) => {
                self.set_simulating_key(true);
                for k in kb.keys.iter() {
                    Self::simulate_event(&EventType::KeyPress(*k));
                    std::thread::sleep(Duration::from_millis(20));
                }
                for k in kb.keys.iter().rev() {
                    Self::simulate_event(&EventType::KeyRelease(*k));
                }
                self.set_simulating_key(false);
                return false;
            }
            _ => (),
        }

        // Actions that require a selected element
        let Some(ElementOfInterest {
            element: Some(element),
            context,
            role,
            frame,
            ..
        }) = self.selected.as_ref()
        else {
            self.notify_then_deactivate(
                &format!("Running a workflow action with no element selected. {act:?}"),
                Level::Error,
            );
            return true;
        };

        match act {
            WorkFlowAction::Focus => {
                Self::focus_on_element(element);
            }
            WorkFlowAction::Press => {
                let center = frame.center();
                self.press_on_element(element, role, center);
            }
            WorkFlowAction::Click => {
                let (x, y) = frame.center();
                Self::simulate_click(x, y, false);
            }
            WorkFlowAction::ShowMenu => {
                let center = frame.center();
                self.right_click_menu_on_element(element, center);
            }
            WorkFlowAction::GoParent => {
                if self.select_parent() {
                    self.target = Target::ChildElement;
                };
            }
            WorkFlowAction::Debug => {
                self.clear_drawing();
                self.notify(&element.inspect(), Level::Debug);
                // HACK: break the loop so the notification will be kept,
                // basically `Debug` should be a terminal op
                self.pending_workflow_actions.clear();
                return true;
            }
            WorkFlowAction::SelectAll => {
                let len = context
                    .clone()
                    .map(|txt| txt.encode_utf16().count())
                    .unwrap_or(0) as isize;
                element.set_selected_range(0, len);
            }
            _ => (),
        }
        false
    }

    fn execute_pending_workflow_actions(&mut self) {
        self.clear_drawing();
        while let Some(act) = self.pending_workflow_actions.pop_front() {
            if self.execute_workflow_action(&act) {
                return;
            };
        }
        self.clear_drawing();
        if self.notification_layers.is_empty() {
            self.notify_then_deactivate("Done", Level::Trace);
        }
    }

    fn execute_workflow(&mut self, idx: usize) {
        let workflow = self
            .config
            .workflows
            .get(idx)
            .cloned()
            .expect("Internal Error: text workflow index out of bounds.");

        // Silently quit if workflow is not valid for current selected element
        if self.is_workflow_valid(&workflow) {
            self.pending_workflow_actions = workflow.actions.into();
            self.execute_pending_workflow_actions();
        }
    }

    /// Select the parent of the currently selected element
    fn select_parent(&mut self) -> bool {
        if let Some(parent_element) = self
            .selected
            .as_ref()
            .and_then(|eoi| eoi.element.as_ref())
            .and_then(|ele| ele.parent().ok())
        {
            let screen_frame = Frame::from_origion(self.screen_size);
            let frame = parent_element.get_frame(screen_frame);
            self.selected = Some(ElementOfInterest {
                element: Some(parent_element),
                context: None,
                role: RoleOfInterest::Generic,
                frame,
            });
            return true;
        }
        false
    }

    async fn go_back_in_filtering(&mut self, mode: FilterMode) {
        match mode {
            // Go back 1 level in element explorer
            FilterMode::Generic if self.target == Target::ChildElement => {
                if self.select_parent() {
                    self.activate(Target::ChildElement);
                }
            }
            FilterMode::WordPicking => {
                if !self.multi_selection.is_on || self.multi_selection.one_side_idex.is_none() {
                    // Go back to text action menu
                    self.word_picker = None;
                    self.draw_element_menu("", &RoleOfInterest::PseudoText, true);
                } else {
                    self.clear_drawing();
                    self.multi_selection.clear_one_side();
                    self.draw_word_picker();
                }
            }
            FilterMode::Generic if self.multi_selection.is_on => {
                self.multi_selection.clear_one_side();
                self.filter_by_key().await;
            }
            FilterMode::OCR if self.multi_selection.is_on => {
                self.multi_selection.clear_one_side();
                self.ocr_res_filtering();
            }
            _ => (),
        }
    }

    async fn perform_filtering(&mut self, key_char: char, mode: FilterMode) {
        if key_char == '-' {
            if self.key_prefix.is_empty() {
                self.go_back_in_filtering(mode).await;
                return;
            } else {
                self.key_prefix.pop();
            }
        } else if self.key_prefix.len() < self.hint_width as usize {
            self.key_prefix.push(key_char);
        }

        match mode {
            FilterMode::OCR => {
                self.ocr_res_filtering();
            }
            FilterMode::Generic => {
                self.filter_by_key().await;
            }
            FilterMode::WordPicking => {
                self.clear_drawing();
                let (matched_words, digits) = self.draw_word_picker();
                self.hint_width = digits;

                if self.key_prefix.len() == digits as usize
                    && matched_words.len() == 1
                    && let Some((idx, text)) = matched_words.first()
                {
                    if self.multi_selection.is_on {
                        if let Some((idx1, idx2)) = self.multi_selection.set_one_side(*idx) {
                            let text = self
                                .word_picker
                                .as_ref()
                                .expect("Internal Error: no word picker set yet.")
                                .select_range(idx1, idx2)
                                .expect("Internal Error: wrong word picker indexing.");
                            self.update_selected_text_and_show_menu(text.clone())
                        } else {
                            self.key_prefix.clear();
                            self.draw_word_picker();
                        }
                    } else {
                        self.update_selected_text_and_show_menu(text.clone())
                    }
                }
            }
        }
    }

    fn perform_text_action(&mut self, ta: TextAction) {
        let Some(ElementOfInterest {
            context: Some(text),
            ..
        }) = self.selected.as_ref()
        else {
            panic!("Internal Error: No selected text in Mode::TextActionMenu.");
        };

        let text = text.clone();

        // Clear old menu no matter which action is taken
        self.clear_drawing();

        // TODO:
        // 1. URL handling
        let keep_drawing = match ta {
            TextAction::Copy => {
                text_to_clipboard(&text);
                self.notify_then_deactivate("Copied to clipboard.", Level::Info);
                true
            }
            TextAction::Dictionary => {
                log::trace!("Looking up `{text}` in Apple Dictionary.");
                if let Some(attr_string) = get_dictionary_attributed_string(
                    &text,
                    &self.config.dictionaries,
                    &self.config.theme,
                ) {
                    let CGSize { width, height } = self.screen_size;
                    let (text_size, _) = estimate_frame_for_text(&attr_string, (width, height));
                    self.window.draw_attributed_string(
                        attr_string,
                        self.screen_size,
                        text_size,
                        &self.config.theme,
                    );
                } else {
                    self.notify_then_deactivate("No definition found.", Level::Warn);
                }
                true
            }
            TextAction::Split => {
                let word_picker = WordPicker::new(text);

                self.clear_cache();
                self.word_picker = Some(word_picker);
                self.set_mode(Mode::WordPicking);
                self.draw_word_picker();
                true
            }
            TextAction::Editor => {
                if let Err(e) = self.open_editor(&text) {
                    self.notify_then_deactivate(
                        &format!("Failed to open editor: {e}"),
                        Level::Error,
                    );
                    true
                } else {
                    false
                }
            }
            TextAction::UserDefined(idx) => {
                self.take_external_action(idx, &text);
                true
            }
        };

        if !keep_drawing {
            self.deactivate();
        }
    }

    fn perform_scroll_action(&mut self, sa: ScrollAction) {
        let Some(ElementOfInterest {
            element: Some(element),
            role,
            frame,
            ..
        }) = self.selected.as_ref()
        else {
            panic!("An element is supposed to be selected before entering Mode::Scrolling!")
        };

        if *role == RoleOfInterest::ScrollBar {
            let Some(old_val) = element
                .value()
                .ok()
                .and_then(|v| v.downcast::<CFNumber>())
                .and_then(|f| f.to_f64())
            else {
                self.deactivate();
                return;
            };

            let scroll_unit = self.config.scroll_distance;
            match sa {
                ScrollAction::DownRight => {
                    Self::scroll_to_value(element, old_val + scroll_unit);
                }
                ScrollAction::UpLeft => {
                    Self::scroll_to_value(element, old_val - scroll_unit);
                }
                ScrollAction::IncreaseDistance => {
                    self.config.scroll_distance *= 1.5;
                }
                ScrollAction::DecreaseDistance => {
                    self.config.scroll_distance /= 1.5;
                }
                ScrollAction::Top => {
                    Self::scroll_to_value(element, 0.0);
                    self.draw_element_menu("", &RoleOfInterest::ScrollBar, false);
                }
                ScrollAction::Bottom => {
                    Self::scroll_to_value(element, 1.0);
                    self.draw_element_menu("", &RoleOfInterest::ScrollBar, false);
                }
            }
        } else {
            let distance = (frame.size().1 * self.config.scroll_distance).max(1.0) as i64;
            match sa {
                ScrollAction::DownRight => {
                    Self::simulate_event(&EventType::Wheel {
                        delta_x: 0,
                        delta_y: -distance,
                    });
                }
                ScrollAction::UpLeft => {
                    Self::simulate_event(&EventType::Wheel {
                        delta_x: 0,
                        delta_y: distance,
                    });
                }
                ScrollAction::IncreaseDistance => {
                    self.config.scroll_distance *= 1.5;
                }
                ScrollAction::DecreaseDistance => {
                    self.config.scroll_distance /= 1.5;
                }
                ScrollAction::Top => {
                    Self::simulate_event(&EventType::Wheel {
                        delta_x: 0,
                        delta_y: 999999,
                    });
                    self.draw_element_menu("", &RoleOfInterest::ScrollBar, false);
                }
                ScrollAction::Bottom => {
                    Self::simulate_event(&EventType::Wheel {
                        delta_x: 0,
                        delta_y: -999999,
                    });
                    self.draw_element_menu("", &RoleOfInterest::ScrollBar, false);
                }
            }
        }
    }

    fn handle_file_update(&mut self, pb: PathBuf) {
        if pb == self.temp_file
            && let Ok(new_text) = std::fs::read_to_string(&self.temp_file)
        {
            self.update_editing_text(new_text);
        } else if pb != self.temp_file {
            match GlyphlowConfig::load_config(&pb) {
                Ok(mut new_config) => {
                    self.element_cache.reload_config(&new_config);
                    let need_warning = !self.config.safe_reload(&mut new_config);
                    self.config = new_config;

                    if need_warning {
                        self.notify_then_deactivate(
                            "Restart the app to apply full changes",
                            Level::Warn,
                        );
                    } else {
                        self.notify_then_deactivate("Configuration reloaded", Level::Info);
                    }
                }
                Err(msg) => {
                    self.notify_then_deactivate(&msg, Level::Error);
                }
            };
        }
    }

    fn update_selected_text(&mut self, new_text: String) {
        if let Some(ElementOfInterest { context, .. }) = self.selected.as_mut() {
            *context = Some(new_text);
        }
    }

    fn update_editing_text(&mut self, new_text: String) {
        if let Some(ElementOfInterest {
            element: Some(ele), ..
        }) = self.editing.as_ref()
            && let Err(e) = ele.set_value(CFString::new(&new_text).as_CFType())
        {
            log::warn!("Failed to set the text of focused element: {ele:?}\n Error: {e}");
            // Reset editing upon failure
            self.editing = None;
        }
    }

    fn update_selected_text_and_show_menu(&mut self, new_text: String) {
        self.update_selected_text(new_text);
        self.draw_element_menu("", &RoleOfInterest::PseudoText, true);
    }

    fn toggle_multiselection(&mut self) {
        self.multi_selection.toggle();
        let on_off = if self.multi_selection.is_on {
            "on"
        } else {
            "off"
        };
        self.notify(&format!("Multi-selection is now {on_off}."), Level::Info);
    }

    pub async fn handle_signal(&mut self, signal: AppSignal) {
        match signal {
            AppSignal::Activate(target) => {
                let quick_follow = target == Target::Scrollable
                    || target == Target::Editable
                    || target == Target::Edit;
                self.activate(target);
                if quick_follow {
                    self.quick_follow().await;
                }
            }
            AppSignal::DeActivate => {
                self.deactivate();
            }
            AppSignal::RunWorkFlow(idx) => {
                self.execute_workflow(idx);
            }
            AppSignal::MenuRefresh(key_prefix) => {
                self.menu_refresh(&key_prefix, false);
            }
            AppSignal::ToggleMultiSelection => match self.target {
                Target::Text | Target::ImageOCR => {
                    self.toggle_multiselection();
                }
                _ if self.word_picker.is_some() => {
                    self.toggle_multiselection();
                }
                _ => {
                    self.notify("Multi selection only works for text.", Level::Warn);
                }
            },
            AppSignal::Filter(key_char, mode) => {
                self.perform_filtering(key_char, mode).await;
            }
            AppSignal::ScrollAction(sa) => {
                self.perform_scroll_action(sa);
            }
            AppSignal::TextAction(ta) => self.perform_text_action(ta),
            AppSignal::ScreenShot => {
                self.clear_drawing();
                let frame = if let Some(eoi) = self.selected.as_ref() {
                    &eoi.frame
                } else {
                    // Defaults to the window
                    &self
                        .select_app_window(self.config.visibility_checking_level)
                        .unwrap_or_else(|| Frame::from_origion(self.screen_size))
                };
                if screen_shot(frame).await {
                    self.notify_then_deactivate("Screenshot copied to clipboard.", Level::Info);
                } else {
                    self.notify_then_deactivate("Failed to take screenshot.", Level::Error);
                };
            }
            AppSignal::FrameOCR => {
                if let Some(ElementOfInterest { frame, .. }) = self.selected.as_ref() {
                    self.target = Target::ImageOCR;
                    self.perform_ocr_on_frame(*frame).await;
                } else {
                    self.clear_drawing();
                    self.activate(Target::ImageOCR);
                }
            }
            AppSignal::FileUpdate(pb) => self.handle_file_update(pb),
            AppSignal::ReadClipboard => {
                self.clear_drawing();
                if let Some(text) = text_from_clipboard() {
                    self.selected = Some(ElementOfInterest::pseudo(
                        None,
                        Frame::from_origion(self.screen_size),
                    ));
                    self.update_selected_text_and_show_menu(text);
                } else {
                    self.notify_then_deactivate("No text found in clipboard.", Level::Warn);
                }
            }
            AppSignal::ClearNotification => {
                if self.check_mode(Mode::WaitAndDeactivate) {
                    self.deactivate();
                } else {
                    for nl in &self.notification_layers {
                        nl.removeFromSuperlayer();
                    }
                }
                self.notification_layers.clear();
            }
        }
    }
}

async fn delay(sender: Sender<()>, timeout_secs: u64) {
    tokio::time::sleep(Duration::from_secs(timeout_secs)).await;
    let _ = sender.send(()).await;
}
