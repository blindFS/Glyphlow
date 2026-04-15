use std::collections::HashSet;

use accessibility::{AXAttribute, AXUIElement, AXUIElementAttributes};
use accessibility_sys::{
    AXValueGetValue, AXValueRef, kAXButtonRole, kAXHiddenAttribute, kAXMenuItemRole,
    kAXPopUpButtonRole, kAXPositionAttribute, kAXSizeAttribute, kAXStaticTextRole, kAXTextAreaRole,
    kAXTextFieldRole, kAXTitleAttribute, kAXValueAttribute, kAXValueTypeCGPoint,
    kAXValueTypeCGSize,
};
use core_foundation::{
    base::{CFType, TCFType},
    boolean::CFBoolean,
    string::CFString,
};
use objc2_core_foundation::{CGPoint, CGSize};

#[derive(Debug, PartialEq)]
pub enum RoleOfInterest {
    Button,
    TextField,
    StaticText,
    TextArea,
    MenuItem,
}

pub struct ElementOfInterest {
    pub element: AXUIElement,
    pub context: Option<String>,
    pub role: RoleOfInterest,
    pub center: (f64, f64),
}

impl ElementOfInterest {
    pub fn new(
        element: AXUIElement,
        context: Option<String>,
        role: RoleOfInterest,
        center: (f64, f64),
    ) -> Self {
        Self {
            element,
            context,
            role,
            center,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Frame {
    top_left: CGPoint,
    bottom_right: CGPoint,
}

impl Frame {
    fn new(x1: f64, y1: f64, x2: f64, y2: f64) -> Self {
        Frame {
            top_left: CGPoint { x: x1, y: y1 },
            bottom_right: CGPoint { x: x2, y: y2 },
        }
    }

    pub fn from_origion(size: CGSize) -> Self {
        Self::new(0.0, 0.0, size.width, size.height)
    }

    /// Calculate the boundaries of the potential intersection
    fn intersect(&self, other: &Frame) -> Option<Self> {
        let inter_x1 = self.top_left.x.max(other.top_left.x);
        let inter_y1 = self.top_left.y.max(other.top_left.y);
        let inter_x2 = self.bottom_right.x.min(other.bottom_right.x);
        let inter_y2 = self.bottom_right.y.min(other.bottom_right.y);

        if inter_x1 < inter_x2 && inter_y1 < inter_y2 {
            Some(Frame::new(inter_x1, inter_y1, inter_x2, inter_y2))
        } else {
            None
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct HintBox {
    pub label: String,
    pub x: f64,
    pub y: f64,
    pub idx: usize,
}

impl HintBox {
    pub fn new(label: String, x: f64, y: f64, idx: usize) -> Self {
        Self { label, x, y, idx }
    }
}

#[derive(Default)]
pub struct ElementCache {
    pub cache: Vec<ElementOfInterest>,
    pub seen_center: HashSet<(u64, u64)>,
}

impl ElementCache {
    pub fn new() -> Self {
        ElementCache {
            cache: vec![],
            seen_center: HashSet::new(),
        }
    }

    pub fn clear(&mut self) {
        self.cache.clear();
        self.seen_center.clear();
    }

    pub fn add(&mut self, element: AXUIElement, context: Option<String>, role: RoleOfInterest) {
        if let Some(center) = element.center()
            // NOTE: de-duplication for DOM elements
            && !self
                .seen_center
                .contains(&(center.0.to_bits(), center.1.to_bits()))
            && (role == RoleOfInterest::Button
                || context
                    .as_ref()
                    // naive filtering
                    .is_none_or(|ctx| {
                        !ctx.is_empty() && !ctx.chars().all(|c| c.is_ascii_punctuation())
                    }))
        {
            self.seen_center
                .insert((center.0.to_bits(), center.1.to_bits()));
            self.cache
                .push(ElementOfInterest::new(element, context, role, center));
        }
    }

    fn int_to_string(i: usize, digits: u32) -> String {
        let mut n = i;
        let mut result = Vec::new();

        while n > 0 {
            let remainder = (n % 26) as u8;
            let char = (b'A' + remainder) as char;
            result.push(char);
            n /= 26;
        }

        // pad to fixed length
        while result.len() < digits as usize {
            result.push('A');
        }
        result.into_iter().rev().collect()
    }

    pub fn hint_boxes(&self, screen_height: f64) -> Vec<HintBox> {
        if self.cache.is_empty() {
            return vec![];
        }

        let digits = self.cache.len().ilog(26) + 1;

        self.cache
            .iter()
            .enumerate()
            .map(|(idx, it)| {
                let ElementOfInterest { center, .. } = it;
                HintBox::new(
                    Self::int_to_string(idx, digits),
                    center.0,
                    screen_height - center.1,
                    idx,
                )
            })
            .collect()
    }
}

pub trait GetAttribute {
    fn get_attribute(&self, attribute_name: &str) -> Option<CFType>;
    fn get_attribute_string(&self, attribute_name: &str) -> Option<String>;
    fn center(&self) -> Option<(f64, f64)>;
    fn get_frame(&self) -> Option<Frame>;
    fn inspect(&self);
    fn visible_frame(&self, parent_frame: &Frame, role: &CFString) -> Option<Frame>;
}

impl GetAttribute for AXUIElement {
    fn get_attribute(&self, attribute_name: &str) -> Option<CFType> {
        self.attribute(&AXAttribute::new(&CFString::new(attribute_name)))
            .ok()
    }

    fn center(&self) -> Option<(f64, f64)> {
        self.get_frame().map(|f| {
            (
                (f.top_left.x + f.bottom_right.x) / 2.0,
                (f.top_left.y + f.bottom_right.y) / 2.0,
            )
        })
    }

    fn get_attribute_string(&self, attribute_name: &str) -> Option<String> {
        self.get_attribute(attribute_name)
            .and_then(|val| val.downcast::<CFString>())
            .map(|cf| cf.to_string())
    }

    fn get_frame(&self) -> Option<Frame> {
        let pos_cf = self.get_attribute(kAXPositionAttribute)?;
        let pos = get_ax_struct::<CGPoint>(&pos_cf, kAXValueTypeCGPoint)?;

        let size_cf = self.get_attribute(kAXSizeAttribute)?;
        let size = get_ax_struct::<CGSize>(&size_cf, kAXValueTypeCGSize)?;

        Some(Frame::new(
            pos.x,
            pos.y,
            pos.x + size.width,
            pos.y + size.height,
        ))
    }

    fn inspect(&self) {
        let role = self.role();
        println!("{role:?} ==== {:?}", self.action_names());
        for attr in &self.attribute_names().unwrap() {
            println!(
                "{role:?} - {attr:?} - {:?}",
                self.get_attribute(attr.to_string().as_str()),
            );
        }
    }

    fn visible_frame(&self, parent_frame: &Frame, _role: &CFString) -> Option<Frame> {
        let is_hidden = self
            .get_attribute(kAXHiddenAttribute)
            .and_then(|val| val.downcast::<CFBoolean>())
            .map(bool::from)
            .unwrap_or(false);

        if is_hidden {
            return None;
        }

        // TODO: handle edge cases according to role
        // e.g. popup menu
        if let Some(this_frame) = self.get_frame() {
            this_frame.intersect(parent_frame)
        } else {
            Some(parent_frame.clone())
        }
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

#[derive(Default, PartialEq, Clone)]
pub enum Target {
    #[default]
    Clickable,
    Text,
}

pub fn traverse_elements(
    element: &AXUIElement,
    parent_frame: &Frame,
    cache: &mut ElementCache,
    target: &Target,
) {
    if let Ok(role) = element.role() {
        // if invisible, return early
        let Some(new_frame) = element.visible_frame(parent_frame, &role) else {
            return;
        };

        #[allow(non_upper_case_globals)]
        match role.to_string().as_str() {
            kAXPopUpButtonRole | kAXButtonRole | "AXRadioButton" => {
                if *target == Target::Clickable {
                    cache.add(element.clone(), None, RoleOfInterest::Button);
                } else if let Some(ctx) = element
                    .label_value()
                    .ok()
                    .or_else(|| {
                        element
                            .get_attribute(kAXTitleAttribute)
                            .and_then(|val| val.downcast::<CFString>())
                    })
                    .map(|cf| cf.to_string())
                {
                    cache.add(element.clone(), Some(ctx), RoleOfInterest::Button);
                }
            }
            kAXTextFieldRole => {
                // element.inspect();
            }
            kAXStaticTextRole => {
                if *target == Target::Text
                    && let Some(value) = element.get_attribute_string(kAXValueAttribute)
                {
                    cache.add(element.clone(), Some(value), RoleOfInterest::StaticText);
                }
            }
            kAXTextAreaRole => {
                // element.inspect();
            }
            kAXMenuItemRole => {
                if let Some(title) = element.get_attribute_string(kAXTitleAttribute) {
                    cache.add(element.clone(), Some(title), RoleOfInterest::MenuItem);
                }
            }
            _ => {
                if *target == Target::Clickable
                    && let Ok(actions) = element.action_names()
                {
                    for action in &actions {
                        if action.to_string().as_str() == "AXPress" {
                            cache.add(element.clone(), None, RoleOfInterest::Button);
                            break;
                        }
                    }
                }
            }
        }

        if let Ok(children) = element.visible_children().or_else(|_| element.children()) {
            for child in &children {
                traverse_elements(&child, &new_frame, cache, target);
            }
        }
    }
}
