use super::{AppEngine, delay, drawing, filtering, interaction};
use crate::{
    Mode,
    ax_element::{ElementOfInterest, GetAttribute, Target, traverse_elements},
    config::{GlyphlowConfig, RoleOfInterest, VisibilityCheckingLevel},
    os_util::{get_focused, get_system_alarm_window},
    util::Frame,
};
use accessibility::AXUIElementAttributes;
use log::Level;
use std::{path::PathBuf, time::Duration};

pub(crate) const SHORT_TIMEOUT: u64 = 1;
pub(crate) const LONG_TIMEOUT: u64 = 2;
pub(crate) const DEBUG_TIMEOUT: u64 = 5;

pub(crate) fn set_mode(executor: &AppEngine, mode: Mode) {
    if let Ok(mut state) = executor.state.lock() {
        *state = mode;
    }
}

pub(crate) fn set_simulating_key(executor: &AppEngine, flag: bool) {
    if let Ok(mut ks) = executor.key_state.lock() {
        ks.is_simulating = flag;
    }
}

pub(crate) fn check_mode(executor: &AppEngine, mode: Mode) -> bool {
    executor.state.try_lock().is_ok_and(|s| *s == mode)
}

pub(crate) fn deactivate(executor: &mut AppEngine) {
    clear_cache(executor);
    drawing::clear_drawing(executor);
    executor.selected = None;
    executor.pending_workflow_actions.clear();
    set_mode(executor, Mode::Idle);
}

pub(crate) fn clear_cache(executor: &mut AppEngine) {
    executor.word_picker = None;
    executor.ocr_cache = None;
    executor.notification_layers.clear();
    executor.hint_boxes.clear();
    executor.element_cache.clear();
    executor.key_prefix.clear();
    executor.multi_selection.reset();
}

pub(crate) fn notify_then_deactivate(executor: &mut AppEngine, msg: &str, log_level: Level) {
    set_mode(executor, Mode::WaitAndDeactivate);
    notify(executor, msg, log_level);
}

pub(crate) fn notify(executor: &mut AppEngine, msg: &str, log_level: Level) {
    let timeout_secs = match log_level {
        Level::Trace | Level::Info => SHORT_TIMEOUT,
        Level::Debug => DEBUG_TIMEOUT,
        _ => LONG_TIMEOUT,
    };
    log::log!(log_level, "{msg}");
    executor
        .notification_layers
        .push(drawing::draw_menu(executor, msg));
    let sender = executor.timeout_sender.clone();
    tokio::spawn(async move { delay(sender, timeout_secs).await });
}

pub(crate) fn select_app_window(
    executor: &mut AppEngine,
    vis_level: VisibilityCheckingLevel,
) -> Option<Frame> {
    let screen_frame = Frame::from_origion(executor.screen_size);

    // NOTE: prioritize system alarms
    if let Some(window) = get_system_alarm_window() {
        let frame = window.get_frame(screen_frame);
        executor.last_window_frame = frame;
        executor.is_electron = false;

        executor.selected = Some(ElementOfInterest::new(
            Some(window),
            None,
            RoleOfInterest::Generic,
            frame,
        ));

        return Some(frame);
    }

    let (pid, focused_app, is_electron) = get_focused()?;
    executor.is_electron = is_electron;

    // HACK: need this to bootstrap UI tree generation for some electron apps,
    // e.g. Discord
    if is_electron && (pid != executor.last_pid || vis_level == VisibilityCheckingLevel::Loosest) {
        let _ = focused_app.role();
        std::thread::sleep(Duration::from_millis(
            executor.config.electron_initial_wait_ms,
        ));
    }
    executor.last_pid = pid;

    // HACK: menu items may go out of focused window
    let (focused_window, window_frame) = if vis_level == VisibilityCheckingLevel::Loosest {
        (focused_app, screen_frame)
    } else {
        let mut window = focused_app.focused_window();
        // NOTE: prioritize popover windows, e.g. Apple Music search
        if let Ok(windows) = focused_app.windows()
            && windows.len() > 1
        {
            use accessibility_sys::kAXPopoverRole;
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
    executor.last_window_frame = window_frame;

    executor.selected = Some(ElementOfInterest::new(
        Some(focused_window),
        None,
        RoleOfInterest::Generic,
        window_frame,
    ));

    Some(window_frame)
}

pub(crate) fn ui_element_traverse_on_activation(executor: &mut AppEngine, target: Target) {
    // HACK: abuse executor.target to mark whether to call external editor
    executor.target = target.clone();
    let target = match target {
        Target::Edit => Target::Editable,
        _ => target,
    };

    let vis_level = match target {
        // NOTE: loose visibility checking for specific targets
        Target::MenuItem | Target::Custom(_) => VisibilityCheckingLevel::Loosest,
        _ => executor.config.visibility_checking_level,
    };

    if executor.selected.is_none() {
        select_app_window(executor, vis_level);
    }

    clear_cache(executor);
    if let Some(ElementOfInterest {
        element: Some(element),
        frame,
        ..
    }) = executor.selected.as_ref()
    {
        traverse_elements(
            element,
            // Very loose visibility constraint
            frame,
            &executor.last_window_frame,
            &mut executor.element_cache,
            &target,
            vis_level,
        );
    }
}

pub(crate) fn activate(executor: &mut AppEngine, target: Target) {
    let need_help_msg = target == Target::ChildElement && executor.selected.is_none();
    ui_element_traverse_on_activation(executor, target);

    if !executor.element_cache.cache.is_empty() {
        set_mode(executor, Mode::Filtering);
        drawing::draw_hints_from_cache(executor);
        if need_help_msg {
            notify(executor, "Press Enter to act.", Level::Trace);
        }
    } else if executor.target == Target::Scrollable
        && let Some(eoi) = executor.selected.as_ref()
    {
        // Fallback to mouse scroll if no scrollbar found
        let (x, y) = eoi.frame.center();
        interaction::simulate_event(&rdev::EventType::MouseMove { x, y });
        clear_cache(executor);
        drawing::draw_element_menu(executor, "", &RoleOfInterest::ScrollBar, true);
    } else {
        drawing::clear_drawing(executor);
        notify_then_deactivate(executor, "No relevant UI elements found.", Level::Warn);
    }
}

pub(crate) fn handle_file_update(executor: &mut AppEngine, pb: PathBuf) {
    if pb == executor.temp_file
        && let Ok(new_text) = std::fs::read_to_string(&executor.temp_file)
    {
        filtering::update_editing_text(executor, new_text);
    } else if pb != executor.temp_file {
        match GlyphlowConfig::load_config(&pb) {
            Ok(mut new_config) => {
                executor.element_cache.reload_config(&new_config);
                let need_warning = !executor.config.safe_reload(&mut new_config);
                executor.config = new_config;

                if need_warning {
                    notify_then_deactivate(
                        executor,
                        "Restart the app to apply full changes",
                        Level::Warn,
                    );
                } else {
                    notify_then_deactivate(executor, "Configuration reloaded", Level::Info);
                }
            }
            Err(msg) => {
                notify_then_deactivate(executor, &msg, Level::Error);
            }
        };
    }
}
