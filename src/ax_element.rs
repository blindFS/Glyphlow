use std::collections::HashMap;

use accessibility::{AXAttribute, AXUIElement, AXUIElementAttributes};
use accessibility_sys::{
    AXValueGetValue, AXValueRef, kAXButtonRole, kAXHiddenAttribute, kAXMenuItemRole, kAXMenuRole,
    kAXPositionAttribute, kAXSizeAttribute, kAXStaticTextRole, kAXTextAreaRole, kAXTextFieldRole,
    kAXTitleAttribute, kAXValueAttribute, kAXValueTypeCGPoint, kAXValueTypeCGSize,
};
use core_foundation::{
    base::{CFType, TCFType},
    boolean::CFBoolean,
    string::CFString,
};
use objc2_core_foundation::{CGPoint, CGSize};

pub type StringishElement = (AXUIElement, Option<String>);
pub type ElementCache = HashMap<String, StringishElement>;

pub trait GetAttribute {
    fn get_attribute(&self, attribute_name: &str) -> Option<CFType>;
    fn get_attribute_string(&self, attribute_name: &str) -> Option<String>;
    fn center(&self) -> Option<(f64, f64)>;
    fn inspect(&self);
    fn is_visible(&self, screen_size: CGSize) -> bool;
}

impl GetAttribute for AXUIElement {
    fn get_attribute(&self, attribute_name: &str) -> Option<CFType> {
        self.attribute(&AXAttribute::new(&CFString::new(attribute_name)))
            .ok()
    }

    fn center(&self) -> Option<(f64, f64)> {
        let pos_cf = self.get_attribute(kAXPositionAttribute)?;
        let size_cf = self.get_attribute(kAXSizeAttribute)?;

        let point = get_ax_struct::<CGPoint>(&pos_cf, kAXValueTypeCGPoint)?;
        let size = get_ax_struct::<CGSize>(&size_cf, kAXValueTypeCGSize)?;

        Some((point.x + size.width / 2.0, point.y + size.height / 2.0))
    }

    fn get_attribute_string(&self, attribute_name: &str) -> Option<String> {
        self.get_attribute(attribute_name)
            .and_then(|val| val.downcast::<CFString>())
            .map(|cf| cf.to_string())
    }

    fn inspect(&self) {
        let role = self.role();
        for attr in &self.attribute_names().unwrap() {
            println!(
                "{role:?} - {attr:?} - {:?}",
                self.get_attribute(attr.to_string().as_str()),
            );
        }
    }

    fn is_visible(&self, screen_size: CGSize) -> bool {
        let is_hidden = self
            .get_attribute(kAXHiddenAttribute)
            .and_then(|val| val.downcast::<CFBoolean>())
            .map(bool::from)
            .unwrap_or(false);
        let pos_cf = self.get_attribute(kAXPositionAttribute);
        let pos = pos_cf.and_then(|val| get_ax_struct::<CGPoint>(&val, kAXValueTypeCGPoint));
        pos.is_none_or(|pos| {
            pos.x >= 0.0
                && pos.x <= screen_size.width
                && pos.y >= 0.0
                && pos.y <= screen_size.height
                && !is_hidden
        })
    }
}

/// A safe helper to extract C-structs from an AXValue stored inside a CFType.
fn get_ax_struct<T: Default>(cf_type: &CFType, value_type: u32) -> Option<T> {
    unsafe {
        let value_ref = cf_type.as_CFTypeRef() as AXValueRef;
        let mut result = T::default();

        AXValueGetValue(value_ref, value_type, &mut result as *mut T as *mut _).then_some(result)
    }
}

pub fn traverse_elements(
    element: &AXUIElement,
    screen_size: CGSize,
    cache: &mut Vec<StringishElement>,
) {
    if let Ok(role) = element.role() {
        #[allow(non_upper_case_globals)]
        match role.to_string().as_str() {
            kAXButtonRole => {
                if let Some(title) = element.get_attribute_string(kAXTitleAttribute)
                    && !title.is_empty()
                {
                    // println!("{role} - {:?}", title);
                    cache.push((element.clone(), Some(title)));
                }
            }
            kAXTextFieldRole => {
                // inspect_element(element);
            }
            kAXStaticTextRole => {
                if let Some(value) = element.get_attribute_string(kAXValueAttribute)
                    && !value.is_empty()
                {
                    // println!("{role} - {:?}", value);
                    cache.push((element.clone(), Some(value)));
                }
            }
            kAXTextAreaRole => {
                // inspect_element(element);
            }
            kAXMenuItemRole => {
                if let Some(title) = element.get_attribute_string(kAXTitleAttribute) {
                    println!("{role} - {:?}", title);
                    // cache.push((element.clone(), Some(title)));
                }
            }
            _ => {
                println!("-------------------- {role} -----------------");
            }
        }

        if role != CFString::new(kAXMenuRole)
            && element.is_visible(screen_size)
            && let Ok(children) = element.children()
        {
            for child in &children {
                traverse_elements(&child, screen_size, cache);
            }
        }
    }
}
