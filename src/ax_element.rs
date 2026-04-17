use std::collections::HashSet;

use accessibility::{AXAttribute, AXUIElement, AXUIElementAttributes};
use accessibility_sys::{
    AXValueCreate, AXValueGetValue, AXValueRef, kAXButtonRole, kAXHiddenAttribute, kAXMenuBarRole,
    kAXMenuItemRole, kAXPopUpButtonRole, kAXPositionAttribute, kAXPressAction,
    kAXSelectedTextRangeAttribute, kAXSizeAttribute, kAXStaticTextRole, kAXTextAreaRole,
    kAXTextFieldRole, kAXTitleAttribute, kAXValueAttribute, kAXValueTypeCFRange,
    kAXValueTypeCGPoint, kAXValueTypeCGSize,
};
use core_foundation::{
    attributed_string::{CFAttributedStringGetString, CFAttributedStringRef},
    base::{CFRange, CFType, FromVoid, TCFType, TCFTypeRef},
    boolean::CFBoolean,
    string::CFString,
};
use objc2_core_foundation::{CGPoint, CGSize};

#[derive(Debug, PartialEq, Clone)]
pub enum RoleOfInterest {
    Button,
    TextField,
    StaticText,
    TextArea,
    MenuItem,
}

#[derive(Clone)]
pub struct ElementOfInterest {
    pub element: AXUIElement,
    pub context: Option<String>,
    // TODO: role based drawing
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

    pub fn hint_boxes(&self, screen_height: f64) -> (u32, Vec<HintBox>) {
        if self.cache.is_empty() {
            return (0, vec![]);
        }

        let digits = self.cache.len().ilog(26) + 1;

        (
            digits,
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
                .collect(),
        )
    }
}

pub trait GetAttribute {
    fn get_attribute(&self, attribute_name: &str) -> Option<CFType>;
    fn get_attribute_string(&self, attribute_name: &str) -> Option<String>;
    fn center(&self) -> Option<(f64, f64)>;
    fn get_pos(&self) -> Option<CGPoint>;
    fn get_size(&self) -> Option<CGSize>;
    fn get_frame(&self) -> Option<Frame>;
    fn inspect(&self);
    fn visible_frame(
        &self,
        parent_frame: &Frame,
        init_frame: &Frame,
        role: &CFString,
    ) -> Option<Frame>;
    fn is_clickable(&self) -> bool;
}

// TODO: logging
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

    fn get_pos(&self) -> Option<CGPoint> {
        let pos_cf = self.get_attribute(kAXPositionAttribute)?;
        cftype_to_rust_type::<CGPoint>(&pos_cf, kAXValueTypeCGPoint)
    }

    fn get_size(&self) -> Option<CGSize> {
        let size_cf = self.get_attribute(kAXSizeAttribute)?;
        cftype_to_rust_type::<CGSize>(&size_cf, kAXValueTypeCGSize)
    }

    fn get_frame(&self) -> Option<Frame> {
        let pos = self.get_pos()?;
        let size = self.get_size()?;

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

    fn visible_frame(
        &self,
        parent_frame: &Frame,
        init_frame: &Frame,
        _role: &CFString,
    ) -> Option<Frame> {
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
            this_frame.intersect(parent_frame)?;
            // HACK: For some fully visible structure of A -> B -> C,
            // somehow the intersection of either A and B or B and C is not empty,
            // but the intersection of all those 3 is empty.
            // We have to introduce the `init_frame` here, typically full screen,
            // to make sure that C is at least within visible area.
            // TODO: trade-off among false-positive, false-negative and performance
            this_frame.intersect(init_frame)
        } else {
            Some(parent_frame.clone())
        }
    }

    fn is_clickable(&self) -> bool {
        self.action_names().is_ok_and(|actions| {
            actions
                .iter()
                .any(|action| action.to_string() == kAXPressAction)
        })
    }
}

pub trait SetAttribute {
    fn set_attribute_by_name(&self, attribute_name: &str, value: CFType);
    fn set_selected_range(&self, location: isize, length: isize);
}

// TODO: logging
impl SetAttribute for AXUIElement {
    fn set_attribute_by_name(&self, attribute_name: &str, value: CFType) {
        let attr = AXAttribute::new(&CFString::new(attribute_name));
        let _ = self.set_attribute(&attr, value);
    }

    fn set_selected_range(&self, location: isize, length: isize) {
        let range = CFRange::init(location, length);
        if let Some(wrapped_range) = rust_type_to_cftype(range, kAXValueTypeCFRange) {
            self.set_attribute_by_name(kAXSelectedTextRangeAttribute, wrapped_range);
        }
    }
}

/// A safe helper to extract C-structs from an AXValue stored inside a CFType.
fn cftype_to_rust_type<T: Default>(cf_type: &CFType, value_type: u32) -> Option<T> {
    unsafe {
        let value_ref = cf_type.as_CFTypeRef() as AXValueRef;
        let mut result = T::default();

        AXValueGetValue(value_ref, value_type, &mut result as *mut T as *mut _).then_some(result)
    }
}

/// A helper for types have no impl Into<CFType>
fn rust_type_to_cftype<T>(value: T, value_type: u32) -> Option<CFType> {
    unsafe {
        let raw_value = AXValueCreate(value_type, &value as *const _ as *const std::ffi::c_void);
        if raw_value.is_null() {
            eprintln!("Failed to create AXValue");
            return None;
        }

        Some(CFType::from_void(raw_value.as_void_ptr()).as_CFType())
    }
}

// TODO: image
#[derive(Default, PartialEq, Clone)]
pub enum Target {
    #[default]
    Clickable,
    Text,
}

pub fn traverse_elements(
    element: &AXUIElement,
    parent_frame: &Frame,
    init_frame: &Frame,
    cache: &mut ElementCache,
    target: &Target,
) {
    if let Ok(role) = element.role() {
        // if invisible, return early
        let Some(new_frame) = element.visible_frame(parent_frame, init_frame, &role) else {
            return;
        };

        // TODO: Fine-grained control
        // 1. Image
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
                            .get_attribute("AXAttributedDescription")
                            .and_then(|val| unsafe {
                                let string_ref = CFAttributedStringGetString(
                                    val.as_concrete_TypeRef() as CFAttributedStringRef,
                                );
                                if string_ref.is_null() {
                                    return None;
                                }
                                Some(CFString::wrap_under_get_rule(string_ref))
                            })
                    })
                    .map(|cf| cf.to_string())
                {
                    cache.add(element.clone(), Some(ctx), RoleOfInterest::Button);
                }
            }
            kAXStaticTextRole => match target {
                Target::Clickable if element.is_clickable() => {
                    cache.add(element.clone(), None, RoleOfInterest::Button);
                }
                Target::Text => {
                    if let Some(value) = element.get_attribute_string(kAXValueAttribute) {
                        cache.add(element.clone(), Some(value), RoleOfInterest::StaticText);
                    }
                }
                _ => (),
            },
            // TODO: select only the visible part, `kAXVisibleCharacterRangeAttribute`
            kAXTextFieldRole | kAXTextAreaRole => {
                // element.inspect();
                if *target == Target::Text
                    && let Some(value) = element.get_attribute_string(kAXValueAttribute)
                {
                    cache.add(element.clone(), Some(value), RoleOfInterest::StaticText);
                }
            }
            kAXMenuBarRole => {
                // NOTE: Exclude system menu bar items
                if let Some(CGPoint { x, y }) = element.get_pos()
                    && x == 0.0
                    && y == 0.0
                {
                    return;
                }
            }
            kAXMenuItemRole => {
                if let Some(title) = element.get_attribute_string(kAXTitleAttribute) {
                    cache.add(element.clone(), Some(title), RoleOfInterest::MenuItem);
                }
            }
            _ => {
                if *target == Target::Clickable && element.is_clickable() {
                    cache.add(element.clone(), None, RoleOfInterest::Button);
                }
            }
        }

        if let Ok(children) = element.visible_children().or_else(|_| element.children()) {
            for child in &children {
                traverse_elements(&child, &new_frame, init_frame, cache, target);
            }
        }
    }
}
