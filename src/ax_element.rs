use crate::{
    config::{GlyphlowTheme, VisibilityCheckingLevel},
    util::{Frame, HintBox, hint_boxes_from_frames, select_range_helper},
};
use accessibility::{AXAttribute, AXUIElement, AXUIElementAttributes};
use accessibility_sys::{
    AXUIElementCopyMultipleAttributeValues, AXValueCreate, AXValueGetValue, AXValueRef,
    kAXButtonRole, kAXCellRole, kAXComboBoxRole, kAXDescriptionAttribute, kAXErrorSuccess,
    kAXGroupRole, kAXHiddenAttribute, kAXImageRole, kAXMenuItemRole, kAXPopUpButtonRole,
    kAXPositionAttribute, kAXPressAction, kAXRoleAttribute, kAXRowRole, kAXScrollBarRole,
    kAXSelectedTextRangeAttribute, kAXSizeAttribute, kAXStaticTextRole, kAXTextAreaRole,
    kAXTextFieldRole, kAXTitleAttribute, kAXValueAttribute, kAXValueTypeCFRange,
    kAXValueTypeCGPoint, kAXValueTypeCGSize, kAXWindowRole,
};
use core_foundation::{
    array::{CFArray, CFArrayRef},
    base::{CFRange, CFType, CFTypeRef, FromVoid, TCFType},
    boolean::CFBoolean,
    string::CFString,
};
use objc2_core_foundation::{CGPoint, CGSize};
use std::collections::HashMap;

#[derive(Debug, PartialEq, Clone)]
pub enum RoleOfInterest {
    Button,
    GenericNode,
    Image,
    MenuItem,
    ScrollBar,
    StaticText,
    TextField,
    Cell,
}

const BASIC_ATTRIBUTES: [&str; 4] = [
    kAXRoleAttribute,
    kAXPositionAttribute,
    kAXSizeAttribute,
    kAXHiddenAttribute,
];

struct ElementBasicAttributes {
    pub frame: Option<Frame>,
    pub hidden: bool,
    pub role: String,
}

impl ElementBasicAttributes {
    fn visible_frame(&self, parent_frame: &Frame) -> Option<Frame> {
        // NOTE: scroll bar positioning depends on its value
        if self.role == kAXScrollBarRole {
            return Some(*parent_frame);
        }

        if self.hidden {
            return None;
        }

        // TODO: handle edge cases according to role
        // e.g. popup menu
        if let Some(this_frame) = self.frame
            // HACK: For some apps, like Finder, it may return false empty frames
            && this_frame.size() != (0.0, 0.0)
        {
            // TODO: For some fully visible structure of A -> B -> C,
            // somehow the intersection of either A and B or B and C is not empty,
            // but the intersection of all those 3 is empty.
            // An extra mode that dives elements 1 level at a time, instead of flattening them all at once
            // TODO: trade-off among false-positive, false-negative and performance
            this_frame.intersect(parent_frame)
        } else {
            Some(*parent_frame)
        }
    }

    fn from(element: &AXUIElement) -> Option<Self> {
        let cf_attributes: Vec<CFString> =
            BASIC_ATTRIBUTES.iter().map(|&s| CFString::new(s)).collect();
        let cf_array_in = CFArray::from_CFTypes(&cf_attributes);

        let mut values_ref: CFArrayRef = std::ptr::null();
        let err = unsafe {
            AXUIElementCopyMultipleAttributeValues(
                element.as_concrete_TypeRef(),
                cf_array_in.as_concrete_TypeRef(),
                // Don't stop on error
                0,
                &mut values_ref,
            )
        };

        if err != kAXErrorSuccess || values_ref.is_null() {
            None
        } else {
            let values_array: CFArray<CFType> =
                unsafe { CFArray::wrap_under_create_rule(values_ref) };
            let values = values_array.get_all_values();

            let role_cf = values_array.get(0).and_then(|v| v.downcast::<CFString>())?;
            let role = role_cf.to_string();

            let pos = values
                .get(1)
                .and_then(|pos_ptr| cftype_to_rust_type::<CGPoint>(*pos_ptr, kAXValueTypeCGPoint));

            let size = values
                .get(2)
                .and_then(|size_ptr| cftype_to_rust_type::<CGSize>(*size_ptr, kAXValueTypeCGSize));

            let frame = match (pos, size) {
                (Some(p), Some(s)) => Some(Frame::new(p.x, p.y, p.x + s.width, p.y + s.height)),
                _ => None,
            };

            let hidden = values_array
                .get(3)
                .and_then(|v| v.downcast::<CFBoolean>())
                .map(bool::from)
                .unwrap_or_default();

            Some(Self {
                role,
                frame,
                hidden,
            })
        }
    }
}

#[derive(Clone, Debug)]
pub struct ElementOfInterest {
    pub element: Option<AXUIElement>,
    pub context: Option<String>,
    // TODO: role based drawing?
    pub role: RoleOfInterest,
    pub frame: Frame,
}

impl ElementOfInterest {
    pub fn new(
        element: Option<AXUIElement>,
        context: Option<String>,
        role: RoleOfInterest,
        frame: Frame,
    ) -> Self {
        Self {
            element,
            context,
            role,
            frame,
        }
    }

    pub fn pseudo(context: Option<String>, frame: Frame) -> Self {
        Self {
            element: None,
            context,
            role: RoleOfInterest::GenericNode,
            frame,
        }
    }
}

#[derive(Default)]
pub struct ElementCache {
    pub cache: Vec<ElementOfInterest>,
    seen_center: HashMap<(u64, u64), usize>,
    element_min_width: f64,
    element_min_height: f64,
    image_min_size: f64,
}

impl ElementCache {
    pub fn new(min_width: f64, min_height: f64, image_min_size: f64) -> Self {
        ElementCache {
            cache: vec![],
            seen_center: HashMap::new(),
            element_min_width: min_width,
            element_min_height: min_height,
            image_min_size,
        }
    }

    pub fn clear(&mut self) {
        self.cache.clear();
        self.seen_center.clear();
    }

    pub fn add(
        &mut self,
        element: &AXUIElement,
        context: Option<String>,
        role: RoleOfInterest,
        frame: Option<Frame>,
    ) {
        // Has to be concrete leaf elements with frames
        let Some(frame) = frame else {
            return;
        };

        // NOTE: Use parent frames for scroll bars,
        // replaces the existing AXScrollArea in later center point check
        let frame = if role == RoleOfInterest::ScrollBar
            && let Some(parent_frame) = element
                .parent()
                .ok()
                .and_then(|p| ElementBasicAttributes::from(&p))
                .and_then(|p_fp| p_fp.frame)
        {
            parent_frame
        } else {
            frame
        };

        let (w, h) = frame.size();
        match role {
            // NOTE: some roles to keep
            RoleOfInterest::GenericNode | RoleOfInterest::ScrollBar | RoleOfInterest::TextField => {
            }
            RoleOfInterest::Image if w.min(h) < self.image_min_size => {
                return;
            }
            // Keep large enough images
            RoleOfInterest::Image => (),
            // Check text before size, keep small texts
            _ if context.is_some() => {
                // Skip elements with empty/nonsense text
                if context.as_ref().is_some_and(|ctx| {
                    ctx.is_empty()
                        || ctx
                            .chars()
                            .all(|c| c.is_ascii_punctuation() || c.is_whitespace())
                }) {
                    return;
                }
            }
            _ if (w < self.element_min_width || h < self.element_min_height) => {
                return;
            }
            _ => (),
        }

        let (x, y) = frame.center();
        // f64 to u64 for hashing
        let center = (x.to_bits(), y.to_bits());

        // NOTE: de-duplication for DOM elements
        let new_ele = ElementOfInterest::new(Some(element.clone()), context, role.clone(), frame);
        // Keep all nodes with Target::ChildElement/GenericNode, as it's basically a debugging mode
        if role != RoleOfInterest::GenericNode
            && let Some(idx) = self.seen_center.get(&center)
        {
            self.cache[*idx] = new_ele;
        } else {
            self.seen_center.insert(center, self.cache.len());
            self.cache.push(new_ele);
        }
    }

    pub fn hint_boxes(
        &self,
        screen_frame: &Frame,
        theme: &GlyphlowTheme,
        colored_frame_min_size: f64,
    ) -> (u32, Vec<HintBox>) {
        hint_boxes_from_frames(
            self.cache.len(),
            self.cache.iter().map(|it| it.frame),
            screen_frame,
            theme,
            colored_frame_min_size,
        )
    }

    pub fn select_range(
        &self,
        idx1: usize,
        idx2: usize,
        ref_role: Option<&RoleOfInterest>,
    ) -> Option<(String, Frame)> {
        let choices: Vec<(String, Frame, bool)> = self
            .cache
            .iter()
            .map(|eoi| {
                let is_valid = ref_role.is_none_or(|ref_role| *ref_role == eoi.role);
                (eoi.context.clone().unwrap_or_default(), eoi.frame, is_valid)
            })
            .collect();
        select_range_helper(&choices, idx1, idx2)
    }
}

pub trait GetAttribute {
    fn get_attribute(&self, attribute_name: &str) -> Option<CFType>;
    fn get_attribute_string(&self, attribute_name: &str) -> Option<String>;
    fn get_frame(&self) -> Option<Frame>;
    fn get_dom_classes(&self) -> Option<Vec<String>>;
    fn inspect(&self);
    fn is_clickable(&self) -> bool;
    fn has_children(&self) -> bool;
}

impl GetAttribute for AXUIElement {
    fn get_attribute(&self, attribute_name: &str) -> Option<CFType> {
        self.attribute(&AXAttribute::new(&CFString::new(attribute_name)))
            .ok()
    }

    fn get_attribute_string(&self, attribute_name: &str) -> Option<String> {
        self.get_attribute(attribute_name)
            .and_then(|val| val.downcast::<CFString>())
            .map(|cf| cf.to_string())
    }

    fn get_frame(&self) -> Option<Frame> {
        let cf_array_in = CFArray::from_CFTypes(&[
            CFString::new(kAXPositionAttribute),
            CFString::new(kAXSizeAttribute),
        ]);

        let mut values_ref: CFArrayRef = std::ptr::null();
        let err = unsafe {
            AXUIElementCopyMultipleAttributeValues(
                self.as_concrete_TypeRef(),
                cf_array_in.as_concrete_TypeRef(),
                // Don't stop on error
                0,
                &mut values_ref,
            )
        };

        if err != kAXErrorSuccess || values_ref.is_null() {
            None
        } else {
            let values_array: CFArray<CFType> =
                unsafe { CFArray::wrap_under_create_rule(values_ref) };
            let values = values_array.get_all_values();

            let pos = values
                .first()
                .and_then(|pos_ptr| cftype_to_rust_type::<CGPoint>(*pos_ptr, kAXValueTypeCGPoint));

            let size = values
                .last()
                .and_then(|size_ptr| cftype_to_rust_type::<CGSize>(*size_ptr, kAXValueTypeCGSize));

            match (pos, size) {
                (Some(p), Some(s)) => Some(Frame::new(p.x, p.y, p.x + s.width, p.y + s.height)),
                _ => None,
            }
        }
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

    fn is_clickable(&self) -> bool {
        self.action_names().is_ok_and(|actions| {
            actions
                .iter()
                .any(|action| action.to_string() == kAXPressAction)
        })
    }

    fn has_children(&self) -> bool {
        self.children()
            .ok()
            .is_some_and(|children| !children.is_empty())
    }

    fn get_dom_classes(&self) -> Option<Vec<String>> {
        let cf_vals = self
            .get_attribute("AXDOMClassList")?
            .downcast::<CFArray>()?;

        let mut classes: Vec<String> = Vec::new();
        for val in cf_vals.iter() {
            let s = unsafe { CFString::from_void(*val) };
            classes.push(s.to_string());
        }

        Some(classes)
    }
}

pub trait SetAttribute {
    fn set_attribute_by_name(&self, attribute_name: &str, value: CFType);
    fn set_selected_range(&self, location: isize, length: isize);
}

impl SetAttribute for AXUIElement {
    fn set_attribute_by_name(&self, attribute_name: &str, value: CFType) {
        let attr = AXAttribute::new(&CFString::new(attribute_name));
        if let Err(e) = self.set_attribute(&attr, value) {
            log::warn!("Failed to set attribute: {e}");
        };
    }

    fn set_selected_range(&self, location: isize, length: isize) {
        let range = CFRange::init(location, length);
        if let Some(wrapped_range) = rust_type_to_cftype(range, kAXValueTypeCFRange) {
            self.set_attribute_by_name(kAXSelectedTextRangeAttribute, wrapped_range);
        }
    }
}

/// A safe helper to extract C-structs from an AXValue stored inside a CFType.
fn cftype_to_rust_type<T: Default>(cf_type: CFTypeRef, value_type: u32) -> Option<T> {
    if cf_type.is_null() {
        return None;
    }

    unsafe {
        let value_ref = cf_type as AXValueRef;
        let mut result = T::default();

        AXValueGetValue(value_ref, value_type, &mut result as *mut T as *mut _).then_some(result)
    }
}

/// A helper for types have no impl Into<CFType>
fn rust_type_to_cftype<T>(value: T, value_type: u32) -> Option<CFType> {
    unsafe {
        let raw_value = AXValueCreate(value_type, &value as *const _ as *const std::ffi::c_void);
        if raw_value.is_null() {
            log::error!("Failed to create AXValue");
            return None;
        }

        Some(CFType::wrap_under_create_rule(raw_value as CFTypeRef))
    }
}

#[derive(Debug, Default, PartialEq, Clone)]
pub enum Target {
    #[default]
    Clickable,
    Image,
    ImageOCR,
    Editable,
    Edit,
    Text,
    MenuItem,
    ChildElement,
    Scrollable,
}

pub fn traverse_elements(
    element: &AXUIElement,
    parent_frame: &Frame,
    cache: &mut ElementCache,
    target: &Target,
    vis_level: VisibilityCheckingLevel,
) {
    let Some(ele_fp) = ElementBasicAttributes::from(element) else {
        return;
    };

    // Get child elements 1 level lower
    // for false negatives aggressively filtered by the visibility checker
    if *target == Target::ChildElement {
        cache.clear();
        if let Ok(children) = element.visible_children().or_else(|_| element.children()) {
            for child in &children {
                // NOTE: Some apps, like App Store, have circular referencing
                if *child == *element {
                    continue;
                }
                if let Some(child_fp) = ElementBasicAttributes::from(&child)
                    && child_fp.visible_frame(parent_frame).is_some()
                {
                    cache.add(&child, None, RoleOfInterest::GenericNode, child_fp.frame);
                }
            }
        }
        // Skip element levels where only 1 item available
        if cache.cache.len() == 1
            && let Some(ElementOfInterest {
                element: Some(element),
                frame,
                ..
            }) = cache.cache.first()
        {
            traverse_elements(&element.clone(), &frame.clone(), cache, target, vis_level);
        }

        return;
    }

    // If invisible, return early
    let Some(mut new_frame) = ele_fp.visible_frame(parent_frame) else {
        // element.inspect();
        return;
    };
    if vis_level == VisibilityCheckingLevel::Loose {
        new_frame = *parent_frame;
    }

    // TODO: Fine-grained control
    #[allow(non_upper_case_globals)]
    match ele_fp.role.as_str() {
        // TODO: DOM Class List based image searching for icon button
        kAXPopUpButtonRole | kAXButtonRole | "AXRadioButton" => match target {
            Target::Clickable => {
                cache.add(element, None, RoleOfInterest::Button, ele_fp.frame);
            }
            Target::Text => {
                if let Some(ctx) = element
                    .label_value()
                    .ok()
                    .or_else(|| element.title().ok())
                    .or_else(|| element.description().ok())
                    .map(|cf| cf.to_string())
                {
                    cache.add(element, Some(ctx), RoleOfInterest::Button, ele_fp.frame);
                }
            }
            _ => (),
        },
        kAXCellRole => {
            if *target == Target::Clickable {
                cache.add(element, None, RoleOfInterest::Cell, ele_fp.frame);
            }
        }
        // NOTE: first found in Discord app
        // hopefully won't cause too many false positives
        kAXRowRole => {
            if *target == Target::Clickable {
                let mut has_cell_child = false;
                if let Ok(children) = element.children() {
                    for child in &children {
                        if child.role().is_ok_and(|r| r == kAXCellRole) {
                            has_cell_child = true;
                            break;
                        }
                    }
                }
                if !has_cell_child {
                    cache.add(element, None, RoleOfInterest::Cell, ele_fp.frame);
                }
            }
        }
        kAXImageRole => match target {
            Target::Image | Target::ImageOCR => {
                cache.add(element, None, RoleOfInterest::Image, ele_fp.frame);
            }
            Target::Clickable if element.is_clickable() => {
                cache.add(element, None, RoleOfInterest::Button, ele_fp.frame);
            }
            _ => (),
        },
        kAXStaticTextRole => match target {
            Target::Clickable if element.is_clickable() => {
                cache.add(element, None, RoleOfInterest::Button, ele_fp.frame);
            }
            Target::Text => {
                if let Some(value) = element
                    .get_attribute_string(kAXValueAttribute)
                    .or_else(|| element.get_attribute_string(kAXDescriptionAttribute))
                {
                    cache.add(
                        element,
                        Some(value),
                        RoleOfInterest::StaticText,
                        ele_fp.frame,
                    );
                }
            }
            _ => (),
        },
        kAXWindowRole => {
            // NOTE: For AXApplication the frame is usually None, defaults to full screen.
            // Need to narrow down to window frame at this place.
            if let Some(win_frame) = ele_fp.frame {
                new_frame = win_frame;
            };
        }
        kAXComboBoxRole | kAXTextFieldRole | kAXTextAreaRole => match target {
            Target::Editable => {
                cache.add(
                    element,
                    element.get_attribute_string(kAXValueAttribute),
                    RoleOfInterest::TextField,
                    ele_fp.frame,
                );
            }
            Target::Text => {
                if let Some(value) = element.get_attribute_string(kAXValueAttribute) {
                    cache.add(
                        element,
                        Some(value),
                        RoleOfInterest::StaticText,
                        ele_fp.frame,
                    );
                }
            }
            // NOTE: Even if not clickable, still could be focused on click
            Target::Clickable => {
                cache.add(element, None, RoleOfInterest::TextField, ele_fp.frame);
            }
            _ => (),
        },
        kAXGroupRole => match target {
            Target::Clickable if element.is_clickable() => {
                cache.add(element, None, RoleOfInterest::Button, ele_fp.frame);
            }
            // NOTE: Potential texts in leaf AXGroup
            Target::ImageOCR if !element.has_children() => {
                cache.add(element, None, RoleOfInterest::Image, ele_fp.frame);
            }
            _ => (),
        },
        kAXMenuItemRole => match target {
            Target::Text => {
                if let Some(title) = element.get_attribute_string(kAXTitleAttribute) {
                    cache.add(element, Some(title), RoleOfInterest::MenuItem, ele_fp.frame);
                }
            }
            Target::MenuItem | Target::Clickable => {
                cache.add(element, None, RoleOfInterest::MenuItem, ele_fp.frame);
            }
            _ => (),
        },
        kAXScrollBarRole => {
            if *target == Target::Scrollable {
                cache.add(element, None, RoleOfInterest::ScrollBar, ele_fp.frame);
            }
        }
        _ => match target {
            Target::Clickable if element.is_clickable() => {
                cache.add(element, None, RoleOfInterest::Button, ele_fp.frame);
            }
            _ => (),
        },
    }

    if let Ok(children) = element.children() {
        for child in &children {
            // NOTE: Some apps, like App Store, have circular referencing
            if *child == *element {
                continue;
            }
            traverse_elements(&child, &new_frame, cache, target, vis_level);
        }
    }
}
