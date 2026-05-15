use accessibility::{AXUIElement, AXUIElementActions, AXUIElementAttributes};
use accessibility_sys::{kAXErrorAttributeUnsupported, kAXErrorCannotComplete, kAXFocusedAttribute};
use core_foundation::{base::TCFType, boolean::CFBoolean, number::CFNumber};
use rdev::{Button, EventType, simulate};
use std::time::Duration;
use super::AppEngine;
use crate::ax_element::SetAttribute;
use crate::config::RoleOfInterest;

pub(crate) fn simulate_event(event_type: &EventType) {
    match simulate(event_type) {
        Ok(()) => (),
        Err(e) => {
            log::error!("Failed to simulate event {event_type:?}: {e}");
        }
    }
}

pub(crate) fn simulate_click(x: f64, y: f64, right: bool) {
    let button = if right { Button::Right } else { Button::Left };
    simulate_event(&EventType::MouseMove { x, y });
    std::thread::sleep(Duration::from_millis(20));
    simulate_event(&EventType::ButtonPress(button));
    std::thread::sleep(Duration::from_millis(20));
    simulate_event(&EventType::ButtonRelease(button));
}

pub(crate) fn focus_on_element(element: &AXUIElement) {
    element.set_attribute_by_name(kAXFocusedAttribute, CFBoolean::true_value().as_CFType());
}

pub(crate) fn press_on_element(executor: &AppEngine, element: &AXUIElement, role: &RoleOfInterest, center: (f64, f64)) {
    let (x, y) = center;
    focus_on_element(element);

    if executor.is_electron || *role == RoleOfInterest::Cell {
        simulate_click(x, y, false);
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
                simulate_click(x, y, false);
            }
        }
    };
}

pub(crate) fn scroll_to_value(element: &AXUIElement, val: f64) {
    if let Err(e) = element.set_value(CFNumber::from(val.clamp(0.0, 1.0)).as_CFType()) {
        log::warn!("Failed to set value to the selected scroll bar: {e}.");
    };
}

pub(crate) fn right_click_menu_on_element(executor: &AppEngine, element: &AXUIElement, center: (f64, f64)) {
    let (x, y) = center;

    if executor.is_electron {
        simulate_click(x, y, true);
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
                simulate_click(x, y, true);
            }
        }
    };
}
